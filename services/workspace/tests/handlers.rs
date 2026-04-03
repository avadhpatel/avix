//! Tests for workspace.svc tool handlers (Phase 2: write operations)

#![allow(dead_code)]

use serde_json::json;

fn extract_caller_user(params: &serde_json::Value) -> String {
    params
        .get("_caller")
        .and_then(|c| c.get("user"))
        .and_then(|u| u.as_str())
        .unwrap_or("anonymous")
        .to_string()
}

#[test]
fn extract_caller_from_params_with_user() {
    let params = json!({
        "project": "myapp",
        "_caller": { "user": "alice", "pid": 42, "token": "tok" }
    });
    let user = extract_caller_user(&params);
    assert_eq!(user, "alice");
}

#[test]
fn extract_caller_defaults_to_anonymous() {
    let params = json!({ "project": "myapp" });
    let user = extract_caller_user(&params);
    assert_eq!(user, "anonymous");
}

#[test]
fn extract_caller_handles_missing_user_field() {
    let params = json!({
        "_caller": { "pid": 42 }
    });
    let user = extract_caller_user(&params);
    assert_eq!(user, "anonymous");
}
