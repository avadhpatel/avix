use serde::{Deserialize, Serialize};
use thiserror::Error;

/// ATP error codes (§9). Serialise as SCREAMING_SNAKE_CASE, e.g. `"EAUTH"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AtpErrorCode {
    /// 401 — invalid or missing token
    Eauth,
    /// 401 — token expired
    Eexpired,
    /// 401 — session ID mismatch
    Esession,
    /// 403 — insufficient role
    Eperm,
    /// 404 — target doesn't exist
    Enotfound,
    /// 409 — operation conflicts with current state
    Econflict,
    /// 409 — ApprovalToken already consumed
    Eused,
    /// 429 — quota exceeded
    Elimit,
    /// 400 — malformed message
    Eparse,
    /// 500 — kernel-side error
    Einternal,
    /// 503 — target service not running
    Eunavail,
}

/// Structured error body carried in an `AtpReply` when `ok: false`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpError {
    pub code: AtpErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl AtpError {
    pub fn new(code: AtpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

/// Error returned when an inbound WebSocket frame cannot be parsed.
#[derive(Debug, Error)]
pub enum AtpFrameError {
    #[error("malformed frame: {0}")]
    Parse(String),
    #[error("unknown message type: {0}")]
    UnknownType(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_serialize_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Eauth).unwrap(),
            "\"EAUTH\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Eexpired).unwrap(),
            "\"EEXPIRED\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Esession).unwrap(),
            "\"ESESSION\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Eperm).unwrap(),
            "\"EPERM\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Enotfound).unwrap(),
            "\"ENOTFOUND\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Econflict).unwrap(),
            "\"ECONFLICT\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Eused).unwrap(),
            "\"EUSED\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Elimit).unwrap(),
            "\"ELIMIT\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Eparse).unwrap(),
            "\"EPARSE\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Einternal).unwrap(),
            "\"EINTERNAL\""
        );
        assert_eq!(
            serde_json::to_string(&AtpErrorCode::Eunavail).unwrap(),
            "\"EUNAVAIL\""
        );
    }

    #[test]
    fn error_code_round_trips() {
        let code = AtpErrorCode::Eused;
        let s = serde_json::to_string(&code).unwrap();
        let back: AtpErrorCode = serde_json::from_str(&s).unwrap();
        assert_eq!(back, code);
    }

    #[test]
    fn atp_error_new_has_no_detail() {
        let e = AtpError::new(AtpErrorCode::Eperm, "not allowed");
        assert_eq!(e.code, AtpErrorCode::Eperm);
        assert_eq!(e.message, "not allowed");
        assert!(e.detail.is_none());
    }

    #[test]
    fn atp_error_with_detail_serializes_detail_field() {
        let e = AtpError::new(AtpErrorCode::Eperm, "not allowed")
            .with_detail(serde_json::json!({ "required_role": "admin" }));
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("required_role"));
    }

    #[test]
    fn atp_error_without_detail_omits_detail_field() {
        let e = AtpError::new(AtpErrorCode::Eparse, "bad input");
        let s = serde_json::to_string(&e).unwrap();
        assert!(!s.contains("detail"));
    }
}
