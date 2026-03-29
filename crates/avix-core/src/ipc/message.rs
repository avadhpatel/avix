use serde::{Deserialize, Serialize};

/// A JSON-RPC 2.0 notification — no `id` field, no response expected.
/// Used for signals delivered to agents and for jobs.emit / jobs.complete / jobs.fail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        }
    }
}

/// Either a full request (has `id`) or a notification (no `id`).
/// Used by IpcServer to distinguish the two wire shapes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
}

impl IpcMessage {
    /// Parse a raw JSON value into an IpcMessage.
    /// Presence of the `id` key determines which variant.
    pub fn from_value(v: serde_json::Value) -> Result<Self, serde_json::Error> {
        if v.get("id").is_some() {
            serde_json::from_value(v).map(IpcMessage::Request)
        } else {
            serde_json::from_value(v).map(IpcMessage::Notification)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: &str, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.to_string(),
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: &str, code: i32, message: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data,
            }),
        }
    }
}

#[repr(i32)]
pub enum JsonRpcErrorCode {
    Eauth = -32001,
    Eperm = -32002,
    Enoent = -32003,
    Ebusy = -32004,
    Etimeout = -32005,
    Eused = -32009,
    Eprovider = -32010,
    Eprovperm = -32018,
}
