use super::*;
use futures_util::stream;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn visual() -> VisualContext {
    VisualContext::new("image/png", b"\x89PNGfake")
}

fn sse(chunks: &[&str]) -> String {
    chunks
        .iter()
        .map(|c| format!("data: {c}\r\n\r\n"))
        .collect()
}

const HELLO: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hel"}]}}]}"#;
const WORLD: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"lo, world"}]},"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":5}}"#;

async fn run(chunks: Vec<Vec<u8>>) -> (Result<StreamOutcome, StreamFailure>, Vec<String>) {
    let mut deltas = Vec::new();
    let body = stream::iter(chunks.into_iter().map(Ok::<_, Infallible>));
    let r = consume_stream(
        body,
        &CancellationToken::new(),
        Duration::from_secs(5),
        |d| deltas.push(d.to_string()),
    )
    .await;
    (r, deltas)
}

// ------------------------------------------------------- request building

#[test]
fn first_request_has_image_then_question() {
    let req = build_request(&visual(), &[], "What is this?");
    assert_eq!(req.contents.len(), 1);
    let parts = &req.contents[0].parts;
    assert_eq!(req.contents[0].role.as_deref(), Some("user"));
    assert_eq!(
        parts[0].inline_data.as_ref().unwrap().mime_type,
        "image/png"
    );
    assert_eq!(parts[1].text.as_deref(), Some("What is this?"));
}

#[test]
fn follow_up_carries_image_once_and_full_history_in_order() {
    let history = vec![
        Turn {
            question: "q1".into(),
            answer: "a1".into(),
        },
        Turn {
            question: "q2".into(),
            answer: "a2".into(),
        },
    ];
    let req = build_request(&visual(), &history, "q3");
    let roles: Vec<_> = req
        .contents
        .iter()
        .map(|c| c.role.clone().unwrap())
        .collect();
    assert_eq!(roles, ["user", "model", "user", "model", "user"]);
    let images = req
        .contents
        .iter()
        .flat_map(|c| &c.parts)
        .filter(|p| p.inline_data.is_some())
        .count();
    assert_eq!(images, 1);
    assert!(req.contents[0].parts[0].inline_data.is_some());
    assert_eq!(req.contents[1].parts[0].text.as_deref(), Some("a1"));
    assert_eq!(
        req.contents[4].parts.last().unwrap().text.as_deref(),
        Some("q3")
    );
}

#[test]
fn request_serializes_to_documented_schema() {
    let req = build_request(&visual(), &[], "hi");
    let v = serde_json::to_value(&req).unwrap();
    assert!(
        v["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Halo")
    );
    assert!(v["systemInstruction"].get("role").is_none());
    let img = &v["contents"][0]["parts"][0]["inlineData"];
    assert_eq!(img["mimeType"], "image/png");
    assert_eq!(
        img["data"],
        base64::engine::general_purpose::STANDARD.encode(b"\x89PNGfake")
    );
    // No null fields leak into the payload.
    assert!(!v.to_string().contains("null"));
}

// -------------------------------------------------------- stream handling

#[tokio::test]
async fn streams_deltas_in_order_and_completes() {
    let (r, deltas) = run(vec![sse(&[HELLO, WORLD]).into_bytes()]).await;
    assert_eq!(deltas, ["Hel", "lo, world"]);
    let out = r.unwrap();
    assert_eq!(out.text, "Hello, world");
    assert_eq!(out.finish_reason.as_deref(), Some("STOP"));
}

#[tokio::test]
async fn tolerates_every_possible_chunk_split() {
    let full = sse(&[HELLO, WORLD]).into_bytes();
    for i in 1..full.len() {
        let (r, _) = run(vec![full[..i].to_vec(), full[i..].to_vec()]).await;
        assert_eq!(r.unwrap().text, "Hello, world", "split at {i}");
    }
}

#[tokio::test]
async fn skips_thought_parts() {
    let c = r#"{"candidates":[{"content":{"parts":[{"text":"thinking...","thought":true},{"text":"Answer"}]},"finishReason":"STOP"}]}"#;
    let (r, deltas) = run(vec![sse(&[c]).into_bytes()]).await;
    assert_eq!(deltas, ["Answer"]);
    assert_eq!(r.unwrap().text, "Answer");
}

#[tokio::test]
async fn premature_disconnect_is_an_interruption() {
    let (r, _) = run(vec![sse(&[HELLO]).into_bytes()]).await;
    match r {
        Err(StreamFailure::Error(e)) => assert_eq!(e.kind, ErrorKind::Interrupted),
        other => panic!("unexpected {other:?}"),
    }
    // Truncated mid-JSON.
    let mut bytes = sse(&[HELLO]).into_bytes();
    bytes.extend_from_slice(b"data: {\"candidates\":[{\"con");
    let (r, _) = run(vec![bytes]).await;
    match r {
        Err(StreamFailure::Error(e)) => assert_eq!(e.kind, ErrorKind::Interrupted),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn transport_error_mid_stream_is_an_interruption() {
    let items: Vec<Result<Vec<u8>, String>> = vec![
        Ok(sse(&[HELLO]).into_bytes()),
        Err("connection reset".into()),
    ];
    let r = consume_stream(
        stream::iter(items),
        &CancellationToken::new(),
        Duration::from_secs(5),
        |_| {},
    )
    .await;
    match r {
        Err(StreamFailure::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Interrupted);
            assert!(e.message.contains("connection reset"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn provider_error_event_in_stream() {
    let c = r#"{"error":{"code":503,"message":"The model is overloaded.","status":"UNAVAILABLE"}}"#;
    let (r, _) = run(vec![sse(&[HELLO, c]).into_bytes()]).await;
    match r {
        Err(StreamFailure::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Unavailable);
            assert!(e.message.contains("overloaded"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn empty_and_blocked_responses() {
    let empty = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#;
    let (r, _) = run(vec![sse(&[empty]).into_bytes()]).await;
    assert!(matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == ErrorKind::EmptyResponse));

    let safety = r#"{"candidates":[{"finishReason":"SAFETY"}]}"#;
    let (r, _) = run(vec![sse(&[safety]).into_bytes()]).await;
    assert!(matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == ErrorKind::Blocked));

    let prompt_blocked = r#"{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT"}}"#;
    let (r, _) = run(vec![sse(&[prompt_blocked]).into_bytes()]).await;
    assert!(matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == ErrorKind::Blocked));
}

#[tokio::test]
async fn max_tokens_with_text_is_success() {
    let c = r#"{"candidates":[{"content":{"parts":[{"text":"partial"}]},"finishReason":"MAX_TOKENS"}]}"#;
    let (r, _) = run(vec![sse(&[c]).into_bytes()]).await;
    assert_eq!(r.unwrap().finish_reason.as_deref(), Some("MAX_TOKENS"));
}

#[tokio::test]
async fn malformed_json_is_reported() {
    let (r, _) = run(vec![b"data: {nope}\n\n".to_vec()]).await;
    assert!(matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == ErrorKind::Provider));
}

#[tokio::test]
async fn cancellation_stops_a_stalled_stream_and_returns_partial_text() {
    let cancel = CancellationToken::new();
    let first = stream::iter(vec![Ok::<_, Infallible>(sse(&[HELLO]).into_bytes())]);
    let body = first.chain(stream::pending());
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.cancel();
    });
    let mut seen = Vec::new();
    let r = consume_stream(Box::pin(body), &cancel, Duration::from_secs(30), |d| {
        seen.push(d.to_string())
    })
    .await;
    assert_eq!(
        r,
        Err(StreamFailure::Cancelled {
            partial: "Hel".into()
        })
    );
    assert_eq!(seen, ["Hel"]);
}

#[tokio::test]
async fn idle_timeout_fires() {
    let body = stream::pending::<Result<Vec<u8>, Infallible>>();
    let r = consume_stream(
        body,
        &CancellationToken::new(),
        Duration::from_millis(30),
        |_| {},
    )
    .await;
    assert!(matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == ErrorKind::Timeout));
}

// ------------------------------------------------------------ HTTP errors

#[test]
fn maps_http_errors_to_actionable_kinds() {
    let bad_key = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID"}]}}"#;
    assert_eq!(
        map_http_error(400, bad_key, None, "m").kind,
        ErrorKind::Authentication
    );
    assert_eq!(
        map_http_error(403, "{}", None, "m").kind,
        ErrorKind::Authentication
    );
    let nf = map_http_error(
        404,
        r#"{"error":{"code":404,"message":"models/x is not found","status":"NOT_FOUND"}}"#,
        None,
        "x",
    );
    assert_eq!(nf.kind, ErrorKind::InvalidModel);
    assert!(nf.message.contains("\"x\""));
    let rl = r#"{"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"17.2s"}]}}"#;
    let e = map_http_error(429, rl, None, "m");
    assert_eq!(e.kind, ErrorKind::RateLimited);
    assert_eq!(e.retry_after_secs, Some(18));
    assert_eq!(
        map_http_error(429, "", Some(5), "m").retry_after_secs,
        Some(5)
    );
    assert_eq!(
        map_http_error(500, "<html>", None, "m").kind,
        ErrorKind::Unavailable
    );
    assert_eq!(
        map_http_error(400, r#"{"error":{"message":"bad image"}}"#, None, "m").kind,
        ErrorKind::Provider
    );
}

// -------------------------------------------- end-to-end over real HTTP

struct Captured {
    request_line: String,
    headers: String,
    body: String,
}

/// Minimal HTTP/1.1 server that records one request and replies with the
/// given status and body, written in small, oddly-sized pieces.
async fn serve_once(status: &'static str, body: String) -> (String, Arc<Mutex<Option<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(None));
    let cap = captured.clone();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let header_end = loop {
            let n = sock.read(&mut tmp).await.unwrap();
            buf.extend_from_slice(&tmp[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let len: usize = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap_or(0);
        while buf.len() < header_end + len {
            let n = sock.read(&mut tmp).await.unwrap();
            buf.extend_from_slice(&tmp[..n]);
        }
        let mut lines = head.lines();
        *cap.lock().unwrap() = Some(Captured {
            request_line: lines.next().unwrap().to_string(),
            headers: head.clone(),
            body: String::from_utf8_lossy(&buf[header_end..]).to_string(),
        });
        let header = format!(
            "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
        );
        sock.write_all(header.as_bytes()).await.unwrap();
        for piece in body.as_bytes().chunks(7) {
            sock.write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                .await
                .unwrap();
            sock.write_all(piece).await.unwrap();
            sock.write_all(b"\r\n").await.unwrap();
            sock.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        sock.write_all(b"0\r\n\r\n").await.unwrap();
    });
    (format!("http://{addr}/v1beta"), captured)
}

#[tokio::test]
async fn full_http_round_trip_streams_and_keeps_key_out_of_url() {
    let (base, captured) = serve_once("200 OK", sse(&[HELLO, WORLD])).await;
    let client = build_client(Timeouts::default()).unwrap();
    let req = build_request(&visual(), &[], "Summarize");
    let mut deltas = Vec::new();
    let out = stream_generate(
        &client,
        &Endpoint {
            base_url: &base,
            api_key: "test-key-123456789012345",
            model: "gemini-test",
        },
        &req,
        Timeouts::default(),
        &CancellationToken::new(),
        |d| deltas.push(d.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(out.text, "Hello, world");
    assert_eq!(deltas, ["Hel", "lo, world"]);

    let cap = captured.lock().unwrap().take().unwrap();
    assert_eq!(
        cap.request_line,
        "POST /v1beta/models/gemini-test:streamGenerateContent?alt=sse HTTP/1.1"
    );
    assert!(!cap.request_line.contains("test-key"));
    assert!(
        cap.headers
            .to_ascii_lowercase()
            .contains("x-goog-api-key: test-key-123456789012345")
    );
    let body: Value = serde_json::from_str(&cap.body).unwrap();
    assert_eq!(body["contents"][0]["parts"][1]["text"], "Summarize");
}

#[tokio::test]
async fn full_http_round_trip_maps_error_status() {
    let body =
        r#"{"error":{"code":404,"message":"models/nope is not found","status":"NOT_FOUND"}}"#
            .to_string();
    let (base, _) = serve_once("404 Not Found", body).await;
    let client = build_client(Timeouts::default()).unwrap();
    let req = build_request(&visual(), &[], "x");
    let r = stream_generate(
        &client,
        &Endpoint {
            base_url: &base,
            api_key: "k",
            model: "nope",
        },
        &req,
        Timeouts::default(),
        &CancellationToken::new(),
        |_| {},
    )
    .await;
    assert!(matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == ErrorKind::InvalidModel));
}

#[tokio::test]
async fn connection_refused_is_a_network_error() {
    // Bind then drop to get a port with nothing listening.
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client = build_client(Timeouts::default()).unwrap();
    let req = build_request(&visual(), &[], "x");
    let r = stream_generate(
        &client,
        &Endpoint {
            base_url: &format!("http://127.0.0.1:{port}"),
            api_key: "k",
            model: "m",
        },
        &req,
        Timeouts::default(),
        &CancellationToken::new(),
        |_| {},
    )
    .await;
    assert!(matches!(r, Err(StreamFailure::Error(ref e)) if e.kind == ErrorKind::Network));
}

#[tokio::test]
async fn retries_transient_errors_before_streaming() {
    let (base1, _) = serve_once(
        "503 Service Unavailable",
        r#"{"error":{"code":503,"message":"high demand","status":"UNAVAILABLE"}}"#.to_string(),
    )
    .await;
    // First attempt hits the 503 server; there is only one server per
    // address, so verify the retry decision via the error path instead.
    let client = build_client(Timeouts::default()).unwrap();
    let req = build_request(&visual(), &[], "x");
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.cancel();
    });
    // After the 503 the helper waits to retry; cancelling during the wait
    // ends it as Cancelled rather than surfacing the transient error.
    let r = stream_generate_with_retry(
        &client,
        &Endpoint {
            base_url: &base1,
            api_key: "k",
            model: "m",
        },
        &req,
        Timeouts::default(),
        &cancel,
        |_| {},
    )
    .await;
    assert!(matches!(r, Err(StreamFailure::Cancelled { .. })), "{r:?}");
}
