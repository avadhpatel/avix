//! ResourceRequest / ResourceResponse — kernel-side handler.
//!
//! Agents send a `ResourceRequest` when they need resources not granted at spawn:
//! additional context tokens, a new tool, a pipe to another agent, or token renewal.
//! The kernel validates the capability token's signature, then routes each sub-request
//! to the appropriate subsystem.
//!
//! Spec: `docs/spec/resource-request.md` and `docs/spec/resource-response.md`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use crate::error::AvixError;
use crate::types::token::CapabilityToken;

// ── Request types ──────────────────────────────────────────────────────────

/// One item in `ResourceRequest.spec.requests`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case")]
pub enum ResourceItem {
    /// Request additional context window tokens.
    ContextTokens {
        amount: u32,
        #[serde(default)]
        reason: String,
    },
    /// Request access to a tool not currently in the agent's CapabilityToken.
    Tool {
        name: String,
        #[serde(default = "default_urgency")]
        urgency: Urgency,
        #[serde(default)]
        reason: String,
    },
    /// Request a pipe to another agent.
    Pipe {
        target_pid: u64,
        #[serde(default = "default_direction")]
        direction: PipeDirection,
        #[serde(default = "default_buffer_tokens")]
        buffer_tokens: u32,
        #[serde(default)]
        reason: String,
    },
    /// Request renewal of the current CapabilityToken before it expires.
    TokenRenewal {
        #[serde(default)]
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PipeDirection {
    In,
    Out,
    Bidirectional,
}

fn default_urgency() -> Urgency {
    Urgency::Normal
}
fn default_direction() -> PipeDirection {
    PipeDirection::Out
}
fn default_buffer_tokens() -> u32 {
    8192
}

/// The full ResourceRequest envelope from agent → kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub agent_pid: u64,
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    /// The HMAC signature from the agent's current CapabilityToken.
    /// The kernel validates this before processing any sub-requests.
    pub capability_token_signature: String,
    pub requests: Vec<ResourceItem>,
}

impl ResourceRequest {
    pub fn new(
        agent_pid: u64,
        capability_token_signature: String,
        requests: Vec<ResourceItem>,
    ) -> Self {
        Self {
            agent_pid,
            request_id: format!("req-{}", Uuid::new_v4()),
            timestamp: Utc::now(),
            capability_token_signature,
            requests,
        }
    }
}

// ── Response types ─────────────────────────────────────────────────────────

/// One grant item in `ResourceResponse.spec.grants` — ordered to match requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case")]
pub enum ResourceGrant {
    ContextTokens {
        granted: bool,
        amount: u32,
        new_total: u32,
        expires_at: Option<DateTime<Utc>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Tool {
        granted: bool,
        name: String,
        /// Present when `granted: true`. The updated token includes the new tool.
        #[serde(skip_serializing_if = "Option::is_none")]
        new_token: Option<CapabilityToken>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        suggestion: Option<String>,
    },
    Pipe {
        granted: bool,
        target_pid: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        pipe_id: Option<String>,
        direction: PipeDirection,
        buffer_tokens: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    TokenRenewal {
        granted: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_token: Option<CapabilityToken>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<DateTime<Utc>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// The full ResourceResponse envelope from kernel → agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceResponse {
    pub request_id: String,
    pub responded_at: DateTime<Utc>,
    pub grants: Vec<ResourceGrant>,
}

// ── Kernel handler ─────────────────────────────────────────────────────────

/// Kernel-side handler for ResourceRequests.
///
/// In production the handler is injected with the HMAC key (AVIX_MASTER_KEY)
/// and processes each sub-request against the live kernel state.
/// For now, `token_renewal` re-signs tokens in-memory; `tool` grants require
/// external HIL approval (returned as denied with a suggestion), and `pipe` /
/// `context_tokens` return canned responses.
pub struct KernelResourceHandler {
    hmac_key: Vec<u8>,
    /// Default TTL for renewed tokens (seconds).
    renewal_ttl_secs: i64,
}

impl KernelResourceHandler {
    pub fn new(hmac_key: Vec<u8>) -> Self {
        Self {
            hmac_key,
            renewal_ttl_secs: 3600,
        }
    }

    /// Process a `ResourceRequest`, returning a `ResourceResponse` with one grant per request.
    ///
    /// First validates that `capability_token_signature` matches the token presented by the
    /// agent — returns `AvixError::CapabilityDenied` if the signature is invalid.
    #[instrument(skip(self, req, current_token))]
    pub fn handle(
        &self,
        req: &ResourceRequest,
        current_token: &CapabilityToken,
    ) -> Result<ResourceResponse, AvixError> {
        // Validate token signature before processing any sub-requests
        if !current_token.verify_signature(&self.hmac_key) {
            return Err(AvixError::CapabilityDenied(
                "capability token signature is invalid".into(),
            ));
        }
        if req.capability_token_signature != current_token.signature {
            return Err(AvixError::CapabilityDenied(
                "request token signature does not match current token".into(),
            ));
        }

        let grants = req
            .requests
            .iter()
            .map(|item| self.dispatch_item(item, current_token))
            .collect();

        Ok(ResourceResponse {
            request_id: req.request_id.clone(),
            responded_at: Utc::now(),
            grants,
        })
    }

    fn dispatch_item(&self, item: &ResourceItem, token: &CapabilityToken) -> ResourceGrant {
        match item {
            ResourceItem::ContextTokens { amount, .. } => {
                // Grant context tokens up to a fixed ceiling (stub: always grant)
                ResourceGrant::ContextTokens {
                    granted: true,
                    amount: *amount,
                    new_total: 64_000 + amount,
                    expires_at: None,
                    reason: None,
                }
            }

            ResourceItem::Tool { name, urgency, reason } => {
                // Tool grants require HIL approval. This handler signals that HIL is
                // needed; the caller (dispatch_manager) is responsible for orchestrating
                // the full SIGPAUSE → HilManager::open → ATP event → SIGRESUME flow.
                let _ = (urgency, reason);
                ResourceGrant::Tool {
                    granted: false,
                    name: name.clone(),
                    new_token: None,
                    reason: Some("HIL approval required".into()),
                    suggestion: None,
                }
            }

            ResourceItem::Pipe {
                target_pid,
                direction,
                buffer_tokens,
                ..
            } => {
                // Grant the pipe — in production this calls pipe::registry::open()
                ResourceGrant::Pipe {
                    granted: true,
                    target_pid: *target_pid,
                    pipe_id: Some(format!("pipe-{}", Uuid::new_v4())),
                    direction: direction.clone(),
                    buffer_tokens: *buffer_tokens,
                    reason: None,
                }
            }

            ResourceItem::TokenRenewal { .. } => {
                // Re-mint a new token with the same grants, extending expiry
                let new_token = CapabilityToken::mint(
                    token.granted_tools.clone(),
                    token.issued_to.clone(),
                    self.renewal_ttl_secs,
                    &self.hmac_key,
                );
                let expires_at = new_token.expires_at;
                ResourceGrant::TokenRenewal {
                    granted: true,
                    new_token: Some(new_token),
                    expires_at: Some(expires_at),
                    reason: None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::token::IssuedTo;

    const TEST_KEY: &[u8] = b"test-master-key-32-bytes-padded!";

    fn signed_token(tools: &[&str]) -> CapabilityToken {
        CapabilityToken::mint(
            tools.iter().map(|s| s.to_string()).collect(),
            None,
            3600,
            TEST_KEY,
        )
    }

    fn make_req(token: &CapabilityToken, items: Vec<ResourceItem>) -> ResourceRequest {
        ResourceRequest::new(42, token.signature.clone(), items)
    }

    fn handler() -> KernelResourceHandler {
        KernelResourceHandler::new(TEST_KEY.to_vec())
    }

    // ── Signature validation ───────────────────────────────────────────

    #[test]
    fn handle_rejects_invalid_token_signature() {
        let mut token = signed_token(&["fs/read"]);
        token.granted_tools.push("extra".into()); // tamper — invalidates sig
        let req = make_req(&token, vec![]);
        let err = handler().handle(&req, &token).unwrap_err();
        assert!(
            err.to_string().contains("invalid"),
            "should reject invalid signature: {err}"
        );
    }

    #[test]
    fn handle_rejects_mismatched_request_signature() {
        let token = signed_token(&["fs/read"]);
        let mut req = make_req(&token, vec![]);
        req.capability_token_signature = "sha256:wrong".into();
        let err = handler().handle(&req, &token).unwrap_err();
        assert!(err.to_string().contains("does not match"), "{err}");
    }

    // ── context_tokens ─────────────────────────────────────────────────

    #[test]
    fn context_tokens_granted() {
        let token = signed_token(&["fs/read"]);
        let req = make_req(
            &token,
            vec![ResourceItem::ContextTokens {
                amount: 10_000,
                reason: "need more".into(),
            }],
        );
        let resp = handler().handle(&req, &token).unwrap();
        assert_eq!(resp.grants.len(), 1);
        match &resp.grants[0] {
            ResourceGrant::ContextTokens {
                granted, amount, ..
            } => {
                assert!(granted);
                assert_eq!(*amount, 10_000);
            }
            _ => panic!("expected ContextTokens grant"),
        }
    }

    // ── tool ───────────────────────────────────────────────────────────

    #[test]
    fn tool_request_returns_denied_hil_required() {
        let token = signed_token(&["fs/read"]);
        let req = make_req(
            &token,
            vec![ResourceItem::Tool {
                name: "send_email".into(),
                urgency: Urgency::Normal,
                reason: "need to notify user".into(),
            }],
        );
        let resp = handler().handle(&req, &token).unwrap();
        match &resp.grants[0] {
            ResourceGrant::Tool {
                granted,
                name,
                reason,
                suggestion,
                ..
            } => {
                assert!(!granted, "tool grants require HIL");
                assert_eq!(name, "send_email");
                assert_eq!(reason.as_deref(), Some("HIL approval required"));
                assert!(suggestion.is_none(), "HIL orchestration is caller's responsibility");
            }
            _ => panic!("expected Tool grant"),
        }
    }

    // ── pipe ───────────────────────────────────────────────────────────

    #[test]
    fn pipe_request_granted_with_pipe_id() {
        let token = signed_token(&["pipe/open"]);
        let req = make_req(
            &token,
            vec![ResourceItem::Pipe {
                target_pid: 58,
                direction: PipeDirection::Out,
                buffer_tokens: 16_384,
                reason: "stream to writer".into(),
            }],
        );
        let resp = handler().handle(&req, &token).unwrap();
        match &resp.grants[0] {
            ResourceGrant::Pipe {
                granted,
                pipe_id,
                target_pid,
                ..
            } => {
                assert!(granted);
                assert_eq!(*target_pid, 58);
                assert!(pipe_id.is_some(), "granted pipe must have a pipe_id");
            }
            _ => panic!("expected Pipe grant"),
        }
    }

    // ── token_renewal ──────────────────────────────────────────────────

    #[test]
    fn token_renewal_returns_new_signed_token() {
        let token = signed_token(&["fs/read", "llm/complete"]);
        let req = make_req(
            &token,
            vec![ResourceItem::TokenRenewal {
                reason: "expiring soon".into(),
            }],
        );
        let resp = handler().handle(&req, &token).unwrap();
        match &resp.grants[0] {
            ResourceGrant::TokenRenewal {
                granted,
                new_token,
                expires_at,
                ..
            } => {
                assert!(granted);
                let new_tok = new_token.as_ref().unwrap();
                assert!(
                    new_tok.verify_signature(TEST_KEY),
                    "renewed token must be signed"
                );
                assert_eq!(
                    new_tok.granted_tools, token.granted_tools,
                    "tools must be preserved"
                );
                assert!(expires_at.is_some());
                assert!(
                    expires_at.unwrap() > token.expires_at,
                    "new expiry must be later"
                );
            }
            _ => panic!("expected TokenRenewal grant"),
        }
    }

    #[test]
    fn token_renewal_preserves_issued_to() {
        let token = CapabilityToken::mint(
            vec!["fs/read".into()],
            Some(IssuedTo {
                pid: 57,
                agent_name: "researcher".into(),
                spawned_by: "alice".into(),
            }),
            3600,
            TEST_KEY,
        );
        let req = make_req(
            &token,
            vec![ResourceItem::TokenRenewal { reason: "".into() }],
        );
        let resp = handler().handle(&req, &token).unwrap();
        match &resp.grants[0] {
            ResourceGrant::TokenRenewal { new_token, .. } => {
                let it = new_token.as_ref().unwrap().issued_to.as_ref().unwrap();
                assert_eq!(it.agent_name, "researcher");
            }
            _ => panic!(),
        }
    }

    // ── batched requests ───────────────────────────────────────────────

    #[test]
    fn batched_requests_return_grants_in_order() {
        let token = signed_token(&["fs/read"]);
        let req = make_req(
            &token,
            vec![
                ResourceItem::ContextTokens {
                    amount: 5_000,
                    reason: "".into(),
                },
                ResourceItem::Tool {
                    name: "email".into(),
                    urgency: Urgency::Low,
                    reason: "".into(),
                },
                ResourceItem::TokenRenewal { reason: "".into() },
            ],
        );
        let resp = handler().handle(&req, &token).unwrap();
        assert_eq!(resp.grants.len(), 3);
        assert!(matches!(
            &resp.grants[0],
            ResourceGrant::ContextTokens { .. }
        ));
        assert!(matches!(&resp.grants[1], ResourceGrant::Tool { .. }));
        assert!(matches!(
            &resp.grants[2],
            ResourceGrant::TokenRenewal { .. }
        ));
    }

    #[test]
    fn response_request_id_matches_request() {
        let token = signed_token(&[]);
        let req = make_req(&token, vec![]);
        let resp = handler().handle(&req, &token).unwrap();
        assert_eq!(resp.request_id, req.request_id);
    }
}
