use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ATP error {code}: {message}")]
    Atp { code: String, message: String },
    #[error("Not connected")]
    NotConnected,
    #[error("Timeout")]
    Timeout,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
