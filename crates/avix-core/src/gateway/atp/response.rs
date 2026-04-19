use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::instrument;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ATPResponse {
    pub id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl ATPResponse {

    #[instrument(skip(id))]
    pub fn ok(id: impl Into<String>, result: Value) -> Self {
        Self {
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    #[instrument(skip_all)]
    pub fn err(id: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(msg.into()),
        }
    }
}
