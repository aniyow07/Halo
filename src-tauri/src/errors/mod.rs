//! Structured application errors.
//!
//! Every error that crosses the IPC boundary is serialized as
//! `{ kind, message, retryAfterSecs? }` so the frontend can show an
//! actionable message without parsing strings.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    InvalidInput,
    InvalidSelection,
    Capture,
    ImageProcessing,
    MissingApiKey,
    Authentication,
    InvalidModel,
    RateLimited,
    Network,
    Timeout,
    Provider,
    /// Transient server-side failure (HTTP 5xx, e.g. "model overloaded").
    Unavailable,
    Blocked,
    EmptyResponse,
    Interrupted,
    StaleSession,
    Busy,
    Configuration,
    Credentials,
    Shortcut,
    Window,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct HaloError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl HaloError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    pub fn with_retry_after(mut self, secs: Option<u64>) -> Self {
        self.retry_after_secs = secs;
        self
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
}

pub type HaloResult<T> = Result<T, HaloError>;

impl From<tauri::Error> for HaloError {
    fn from(err: tauri::Error) -> Self {
        Self::new(ErrorKind::Window, format!("Window system error: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_kind_and_message_in_camel_case() {
        let err = HaloError::new(ErrorKind::RateLimited, "slow down").with_retry_after(Some(7));
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "rateLimited");
        assert_eq!(json["message"], "slow down");
        assert_eq!(json["retryAfterSecs"], 7);
    }

    #[test]
    fn omits_retry_after_when_absent() {
        let json = serde_json::to_value(HaloError::internal("x")).unwrap();
        assert!(json.get("retryAfterSecs").is_none());
    }
}
