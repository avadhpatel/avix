use avix_core::auth::atp_token::ATPTokenStore;
use avix_core::auth::service::AuthService;
use avix_core::config::auth::{AuthConfig, AuthIdentity, AuthPolicy, CredentialType};
use avix_core::gateway::atp::error::{AtpError, AtpErrorCode};
use avix_core::gateway::atp::frame::{AtpCmd, AtpFrame};
use avix_core::gateway::handlers::dispatch;
use avix_core::gateway::handlers::{HandlerCtx, IpcRouter};
use avix_core::gateway::validator::ValidatedCmd;
use avix_core::types::Role;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A mock IPC router for unit tests.
/// Pre-load method → response pairs; anything not found returns Eunavail.
struct MockIpcRouter {
    responses: Mutex<HashMap<String, Result<Value, AtpError>>>,
}

impl MockIpcRouter {
    fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
        }
    }

    async fn set_ok(&self, method: &str, value: Value) {
        self.responses
            .lock()
            .await
            .insert(method.to_string(), Ok(value));
    }
}

#[async_trait::async_trait]
impl IpcRouter for MockIpcRouter {
    async fn call(&self, method: &str, _params: Value) -> Result<Value, AtpError> {
        self.responses
            .lock()
            .await
            .get(method)
            .cloned()
            .unwrap_or_else(|| {
                Err(AtpError::new(
                    AtpErrorCode::Eunavail,
                    format!("mock: no response for '{method}'"),
                ))
            })
    }
}

/// Build a minimal `HandlerCtx` with a mock IPC router pre-loaded with one response.
async fn make_test_ctx(method: &str, response: serde_json::Value) -> HandlerCtx {
    let mock = Arc::new(MockIpcRouter::new());
    mock.set_ok(method, response).await;
    HandlerCtx {
        ipc: mock,
        token_store: Arc::new(ATPTokenStore::new("test-secret".into())),
        hil_manager: None,
        auth_svc: Arc::new(AuthService::new(AuthConfig {
            api_version: "v1".into(),
            kind: "AuthConfig".into(),
            policy: AuthPolicy {
                session_ttl: "8h".into(),
                require_tls: false,
            },
            identities: vec![AuthIdentity {
                name: "alice".into(),
                uid: 1001,
                role: Role::Admin,
                credential: CredentialType::ApiKey {
                    key_hash: "key123".into(),
                    header: None,
                },
            }],
        })),
    }
}

#[tokio::test]
async fn dispatch_proc_spawn_returns_pid_reply() {
    let ctx = make_test_ctx("kernel/proc/spawn", json!({"pid": 123})).await;
    let cmd = AtpCmd {
        msg_type: "cmd".to_string(),
        id: "test-1".to_string(),
        token: "fake-token".to_string(),
        domain: avix_core::gateway::atp::types::AtpDomain::Proc,
        op: "spawn".to_string(),
        body: json!({"agent": "test-agent"}),
    };
    let validated = ValidatedCmd {
        cmd,
        caller_identity: "alice".to_string(),
        caller_role: Role::Admin,
        caller_session_id: "session-1".to_string(),
    };
    let reply = dispatch(validated, &ctx).await;
    assert!(reply.ok);
    assert_eq!(reply.id, "test-1");
    assert_eq!(reply.body.as_ref().unwrap()["pid"], 123);
}

#[tokio::test]
async fn dispatch_unknown_op_returns_eparse() {
    let ctx = make_test_ctx("kernel/proc/unknown", json!({})).await;
    let cmd = AtpCmd {
        msg_type: "cmd".to_string(),
        id: "test-2".to_string(),
        token: "fake-token".to_string(),
        domain: avix_core::gateway::atp::types::AtpDomain::Proc,
        op: "unknown".to_string(),
        body: json!({}),
    };
    let validated = ValidatedCmd {
        cmd,
        caller_identity: "alice".to_string(),
        caller_role: Role::Admin,
        caller_session_id: "session-2".to_string(),
    };
    let reply = dispatch(validated, &ctx).await;
    assert!(!reply.ok);
    assert_eq!(reply.id, "test-2");
    assert!(reply.error.as_ref().unwrap().message.contains("unknown op"));
}

#[tokio::test]
async fn atp_frame_parse_cmd() {
    let frame_str = r#"{"type":"cmd","id":"test","token":"tok","domain":"proc","op":"spawn","body":{"agent":"test"}}"#;
    let frame = AtpFrame::parse(frame_str).unwrap();
    match frame {
        AtpFrame::Cmd(cmd) => {
            assert_eq!(cmd.id, "test");
            assert_eq!(cmd.domain, avix_core::gateway::atp::types::AtpDomain::Proc);
            assert_eq!(cmd.op, "spawn");
        }
        _ => panic!("expected Cmd"),
    }
}

#[tokio::test]
async fn atp_frame_parse_subscribe() {
    let frame_str = r#"{"type":"subscribe","id":"sub-1","token":"tok","events":["agent.status","hil.request"]}"#;
    let frame = AtpFrame::parse(frame_str).unwrap();
    match frame {
        AtpFrame::Subscribe(sub) => {
            assert_eq!(sub.events, vec!["agent.status", "hil.request"]);
        }
        _ => panic!("expected Subscribe"),
    }
}
