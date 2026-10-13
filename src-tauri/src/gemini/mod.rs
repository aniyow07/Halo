//! Google Gemini multimodal client (REST, `streamGenerateContent?alt=sse`).
//!
//! Endpoint, auth header and schema follow the official docs:
//! <https://ai.google.dev/api/generate-content>. The key travels only in the
//! `x-goog-api-key` header, never in the URL, so it cannot leak through
//! logged URLs or error messages.

pub mod sse;

use std::time::Duration;

use base64::Engine;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::errors::{ErrorKind, HaloError};
use sse::SseParser;

pub const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

const SYSTEM_PROMPT: &str = "You are Halo, a desktop assistant. The user selected a region of their \
screen; the attached image is exactly that region. Answer the user's question about it. Read text in \
the image carefully and quote it exactly when relevant. If something is unreadable or ambiguous, say so \
instead of guessing. Answer in concise GitHub-flavoured Markdown, using fenced code blocks for code. \
Follow-up questions refer to the same image.";

// ------------------------------------------------------------------ schema

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GenerateRequest {
    pub system_instruction: Content,
    pub contents: Vec<Content>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,
    /// Set by thinking models for thought summaries; never shown as answer text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InlineData {
    pub mime_type: String,
    pub data: String,
}

impl Part {
    fn text(t: impl Into<String>) -> Self {
        Self {
            text: Some(t.into()),
            inline_data: None,
            thought: None,
        }
    }
    fn image(mime_type: &str, data_b64: &str) -> Self {
        Self {
            text: None,
            inline_data: Some(InlineData {
                mime_type: mime_type.into(),
                data: data_b64.into(),
            }),
            thought: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerateResponseChunk {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    pub prompt_feedback: Option<PromptFeedback>,
    pub error: Option<ApiErrorBody>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub content: Option<Content>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptFeedback {
    pub block_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorEnvelope {
    pub error: ApiErrorBody,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub code: u16,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub details: Vec<Value>,
}

// ----------------------------------------------------- conversation context

/// The image of the active selection, base64-encoded once.
#[derive(Debug, Clone)]
pub struct VisualContext {
    pub mime_type: &'static str,
    pub data_b64: String,
}

impl VisualContext {
    pub fn new(mime_type: &'static str, bytes: &[u8]) -> Self {
        Self {
            mime_type,
            data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub question: String,
    pub answer: String,
}

/// Gemini is stateless: every request carries the image and the full
/// history explicitly. The image is attached to the first user turn.
pub fn build_request(visual: &VisualContext, history: &[Turn], question: &str) -> GenerateRequest {
    let mut contents = Vec::with_capacity(history.len() * 2 + 1);
    let image = Part::image(visual.mime_type, &visual.data_b64);
    let user = |parts| Content {
        role: Some("user".into()),
        parts,
    };
    let mut image_slot = Some(image);
    for turn in history {
        let mut parts = Vec::new();
        if let Some(img) = image_slot.take() {
            parts.push(img);
        }
        parts.push(Part::text(&turn.question));
        contents.push(user(parts));
        contents.push(Content {
            role: Some("model".into()),
            parts: vec![Part::text(&turn.answer)],
        });
    }
    let mut parts = Vec::new();
    if let Some(img) = image_slot.take() {
        parts.push(img);
    }
    parts.push(Part::text(question));
    contents.push(user(parts));

    GenerateRequest {
        system_instruction: Content {
            role: None,
            parts: vec![Part::text(SYSTEM_PROMPT)],
        },
        contents,
    }
}

// -------------------------------------------------------------- errors

fn retry_delay_from_details(details: &[Value]) -> Option<u64> {
    details.iter().find_map(|d| {
        let ty = d.get("@type")?.as_str()?;
        if !ty.ends_with("google.rpc.RetryInfo") {
            return None;
        }
        let delay = d.get("retryDelay")?.as_str()?;
        delay
            .trim_end_matches('s')
            .parse::<f64>()
            .ok()
            .map(|s| s.ceil() as u64)
    })
}

fn detail_reason(details: &[Value]) -> Option<String> {
    details
        .iter()
        .find_map(|d| d.get("reason").and_then(Value::as_str).map(str::to_string))
}

/// Map a non-2xx HTTP response to an actionable error.
pub fn map_http_error(
    status: u16,
    body: &str,
    retry_after_header: Option<u64>,
    model: &str,
) -> HaloError {
    let parsed = serde_json::from_str::<ApiErrorEnvelope>(body)
        .ok()
        .map(|e| e.error);
    let provider_msg = parsed
        .as_ref()
        .map(|e| e.message.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    let reason = parsed
        .as_ref()
        .and_then(|e| detail_reason(&e.details))
        .unwrap_or_default();
    let api_status = parsed
        .as_ref()
        .map(|e| e.status.clone())
        .unwrap_or_default();
    let retry = parsed
        .as_ref()
        .and_then(|e| retry_delay_from_details(&e.details))
        .or(retry_after_header);

    let key_problem =
        reason.starts_with("API_KEY") || provider_msg.to_ascii_lowercase().contains("api key");

    match status {
        400 if key_problem => HaloError::new(
            ErrorKind::Authentication,
            format!("Gemini rejected the API key: {provider_msg}"),
        ),
        401 | 403 => HaloError::new(
            ErrorKind::Authentication,
            format!("Gemini denied access ({api_status}): {provider_msg}"),
        ),
        404 => HaloError::new(
            ErrorKind::InvalidModel,
            format!(
                "Model \"{model}\" was not found or does not support streaming: {provider_msg}"
            ),
        ),
        429 => HaloError::new(
            ErrorKind::RateLimited,
            format!("Gemini rate limit or quota reached: {provider_msg}"),
        )
        .with_retry_after(retry),
        400 => HaloError::new(
            ErrorKind::Provider,
            format!("Gemini rejected the request: {provider_msg}"),
        ),
        500..=599 => HaloError::new(
            ErrorKind::Unavailable,
            format!("Gemini is temporarily unavailable (HTTP {status}): {provider_msg}"),
        )
        .with_retry_after(retry),
        _ => HaloError::new(
            ErrorKind::Provider,
            format!("Unexpected Gemini response (HTTP {status}): {provider_msg}"),
        ),
    }
}

fn map_transport_error(err: &reqwest::Error) -> HaloError {
    // The key is a header, so it never appears in reqwest errors; strip the
    // URL anyway to keep messages short.
    let mut msg = err.to_string();
    if let Some(url) = err.url() {
        msg = msg.replace(url.as_str(), "Gemini");
    }
    if err.is_timeout() {
        HaloError::new(ErrorKind::Timeout, "Timed out talking to Gemini.")
    } else if err.is_connect() {
        HaloError::new(
            ErrorKind::Network,
            format!("Could not connect to Gemini. Check your network connection. ({msg})"),
        )
    } else {
        HaloError::new(
            ErrorKind::Network,
            format!("Network error while talking to Gemini: {msg}"),
        )
    }
}

// ------------------------------------------------------------- streaming

#[derive(Debug, Clone, PartialEq)]
pub struct StreamOutcome {
    pub text: String,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamFailure {
    Cancelled { partial: String },
    Error(HaloError),
}

impl From<HaloError> for StreamFailure {
    fn from(e: HaloError) -> Self {
        StreamFailure::Error(e)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    /// Time allowed until response headers arrive.
    pub response: Duration,
    /// Max silence between two chunks of the body.
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            response: Duration::from_secs(90),
            idle: Duration::from_secs(90),
        }
    }
}

const BLOCKING_FINISH: &[&str] = &[
    "SAFETY",
    "RECITATION",
    "BLOCKLIST",
    "PROHIBITED_CONTENT",
    "SPII",
    "IMAGE_SAFETY",
    "IMAGE_PROHIBITED_CONTENT",
];

/// Consume an SSE byte stream from Gemini, forwarding answer text as it
/// arrives. Pure with respect to I/O so it can be tested with synthetic
/// streams.
pub async fn consume_stream<S, B, E>(
    mut body: S,
    cancel: &CancellationToken,
    idle: Duration,
    mut on_delta: impl FnMut(&str),
) -> Result<StreamOutcome, StreamFailure>
where
    S: Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut parser = SseParser::new();
    let mut text = String::new();
    let mut finish_reason: Option<String> = None;
    let mut block_reason: Option<String> = None;

    let mut handle = |data: String,
                      text: &mut String,
                      finish_reason: &mut Option<String>,
                      block_reason: &mut Option<String>|
     -> Result<(), StreamFailure> {
        let chunk: GenerateResponseChunk = serde_json::from_str(&data).map_err(|e| {
            HaloError::new(
                ErrorKind::Provider,
                format!("Gemini sent malformed stream data: {e}"),
            )
        })?;
        if let Some(err) = chunk.error {
            return Err(map_http_error(err.code, &serde_json::json!({ "error": {
                "code": err.code, "message": err.message, "status": err.status, "details": err.details
            }}).to_string(), None, "").into());
        }
        if let Some(reason) = chunk.prompt_feedback.and_then(|f| f.block_reason) {
            *block_reason = Some(reason);
        }
        for cand in chunk.candidates.into_iter().take(1) {
            if let Some(content) = cand.content {
                for part in content.parts {
                    if part.thought == Some(true) {
                        continue;
                    }
                    if let Some(t) = part.text.filter(|t| !t.is_empty()) {
                        text.push_str(&t);
                        on_delta(&t);
                    }
                }
            }
            if cand.finish_reason.is_some() {
                *finish_reason = cand.finish_reason;
            }
        }
        Ok(())
    };

    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(StreamFailure::Cancelled { partial: text }),
            n = tokio::time::timeout(idle, body.next()) => n,
        };
        match next {
            Err(_) => {
                return Err(HaloError::new(
                    ErrorKind::Timeout,
                    format!("Gemini stopped sending data for {}s.", idle.as_secs()),
                )
                .into());
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                return Err(HaloError::new(
                    ErrorKind::Interrupted,
                    format!("The connection to Gemini dropped mid-response: {e}"),
                )
                .into());
            }
            Ok(Some(Ok(bytes))) => {
                for data in parser.push(bytes.as_ref()) {
                    handle(data, &mut text, &mut finish_reason, &mut block_reason)?;
                }
            }
        }
    }
    let had_partial = parser.has_pending();
    for data in parser.finish() {
        // A truncated trailing event fails JSON parsing; report that as an
        // interruption rather than a provider error.
        if let Err(e) = handle(data, &mut text, &mut finish_reason, &mut block_reason) {
            if had_partial && finish_reason.is_none() {
                return Err(HaloError::new(
                    ErrorKind::Interrupted,
                    "The connection to Gemini closed in the middle of a message.",
                )
                .into());
            }
            return Err(e);
        }
    }

    if let Some(reason) = block_reason {
        return Err(HaloError::new(
            ErrorKind::Blocked,
            format!("Gemini blocked this request ({reason})."),
        )
        .into());
    }
    match finish_reason.as_deref() {
        None => Err(HaloError::new(
            ErrorKind::Interrupted,
            "The Gemini stream ended before the response was complete.",
        )
        .into()),
        Some(r) if text.trim().is_empty() && BLOCKING_FINISH.contains(&r) => Err(HaloError::new(
            ErrorKind::Blocked,
            format!("Gemini declined to answer ({r})."),
        )
        .into()),
        Some(r) if text.trim().is_empty() => Err(HaloError::new(
            ErrorKind::EmptyResponse,
            format!("Gemini returned an empty response (finish reason: {r})."),
        )
        .into()),
        Some(_) => Ok(StreamOutcome {
            text,
            finish_reason,
        }),
    }
}

pub fn build_client(timeouts: Timeouts) -> Result<reqwest::Client, HaloError> {
    reqwest::Client::builder()
        .connect_timeout(timeouts.connect)
        .user_agent(concat!("Halo/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| HaloError::internal(format!("Could not create HTTP client: {e}")))
}

fn retry_after_header(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Send a streaming request and forward deltas until completion, error or
/// cancellation. Dropping the response body closes the connection.
/// Where and as whom to send a request.
pub struct Endpoint<'a> {
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
}

pub async fn stream_generate(
    client: &reqwest::Client,
    endpoint: &Endpoint<'_>,
    request: &GenerateRequest,
    timeouts: Timeouts,
    cancel: &CancellationToken,
    on_delta: impl FnMut(&str),
) -> Result<StreamOutcome, StreamFailure> {
    let Endpoint {
        base_url,
        api_key,
        model,
    } = *endpoint;
    let model_path = model.strip_prefix("models/").unwrap_or(model);
    let url = format!("{base_url}/models/{model_path}:streamGenerateContent?alt=sse");
    let send = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(request)
        .send();

    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(StreamFailure::Cancelled { partial: String::new() }),
        r = tokio::time::timeout(timeouts.response, send) => match r {
            Err(_) => return Err(HaloError::new(ErrorKind::Timeout, "Gemini did not respond in time.").into()),
            Ok(Err(e)) => return Err(map_transport_error(&e).into()),
            Ok(Ok(resp)) => resp,
        },
    };

    let status = resp.status();
    if !status.is_success() {
        let retry = retry_after_header(&resp);
        let body = tokio::select! {
            _ = cancel.cancelled() => return Err(StreamFailure::Cancelled { partial: String::new() }),
            b = tokio::time::timeout(timeouts.idle, resp.text()) => b.ok().and_then(Result::ok).unwrap_or_default(),
        };
        return Err(map_http_error(status.as_u16(), &body, retry, model_path).into());
    }

    consume_stream(resp.bytes_stream(), cancel, timeouts.idle, on_delta).await
}

/// Attempts for requests that fail with a transient server error before
/// any answer text was produced.
pub const MAX_ATTEMPTS: u32 = 3;

/// `stream_generate`, retrying transient failures (HTTP 5xx) with backoff as
/// long as nothing has been streamed yet. Never retries once text was shown,
/// so the user never sees a duplicated answer.
pub async fn stream_generate_with_retry(
    client: &reqwest::Client,
    endpoint: &Endpoint<'_>,
    request: &GenerateRequest,
    timeouts: Timeouts,
    cancel: &CancellationToken,
    mut on_delta: impl FnMut(&str),
) -> Result<StreamOutcome, StreamFailure> {
    let mut attempt = 1;
    loop {
        let mut streamed = false;
        let result = stream_generate(client, endpoint, request, timeouts, cancel, |d| {
            streamed = true;
            on_delta(d);
        })
        .await;
        match result {
            Err(StreamFailure::Error(e))
                if e.kind == ErrorKind::Unavailable && !streamed && attempt < MAX_ATTEMPTS =>
            {
                let wait =
                    Duration::from_secs(e.retry_after_secs.unwrap_or(1u64 << attempt).min(10));
                log::info!("Gemini unavailable (attempt {attempt}), retrying in {wait:?}");
                tokio::select! {
                    _ = cancel.cancelled() => return Err(StreamFailure::Cancelled { partial: String::new() }),
                    _ = tokio::time::sleep(wait) => {}
                }
                attempt += 1;
            }
            other => return other,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCheck {
    pub model: String,
    pub display_name: Option<String>,
    pub supports_streaming: bool,
    pub input_token_limit: Option<u64>,
}

/// Validate a model id against the live `models.get` endpoint.
pub async fn check_model(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<ModelCheck, HaloError> {
    let model_path = model.strip_prefix("models/").unwrap_or(model);
    let resp = client
        .get(format!("{base_url}/models/{model_path}"))
        .header("x-goog-api-key", api_key)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| map_transport_error(&e))?;
    let status = resp.status();
    let retry = retry_after_header(&resp);
    let body = resp.text().await.map_err(|e| map_transport_error(&e))?;
    if !status.is_success() {
        return Err(map_http_error(status.as_u16(), &body, retry, model_path));
    }
    let v: Value = serde_json::from_str(&body).map_err(|e| {
        HaloError::new(
            ErrorKind::Provider,
            format!("Unexpected models.get response: {e}"),
        )
    })?;
    let methods: Vec<&str> = v["supportedGenerationMethods"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Ok(ModelCheck {
        model: model_path.to_string(),
        display_name: v["displayName"].as_str().map(str::to_string),
        supports_streaming: methods.contains(&"streamGenerateContent")
            || methods.contains(&"generateContent"),
        input_token_limit: v["inputTokenLimit"].as_u64(),
    })
}

#[cfg(test)]
mod tests;
