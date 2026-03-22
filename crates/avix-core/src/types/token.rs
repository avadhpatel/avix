use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::Role;

/// Session-level token (used in auth::session)
#[derive(Debug, Clone)]
pub struct SessionToken {
    pub role: Role,
    pub session_id: String,
}

/// Who the token was issued to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedTo {
    pub pid: u32,
    pub agent_name: String,
    pub spawned_by: String,
}

/// HMAC-signed capability token issued by the kernel at agent spawn time.
///
/// `granted_tools` stores **individual tool names** (e.g. `"fs/read"`, `"agent/spawn"`).
/// Capability group names like `agent:spawn` are used only by token issuers to expand
/// into individual tools — they never appear in `granted_tools`.
///
/// The `signature` field is `sha256:<hex>` computed over a canonical payload using
/// HMAC-SHA256 with the kernel master key. Any modification invalidates the signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    pub granted_tools: Vec<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_to: Option<IssuedTo>,
    /// `sha256:<hex>` HMAC-SHA256 signature over canonical payload.
    pub signature: String,
}

impl CapabilityToken {
    /// Returns `true` if `tool` is in the granted tools list.
    pub fn has_tool(&self, tool: &str) -> bool {
        self.granted_tools.iter().any(|t| t == tool)
    }

    /// Returns `true` if the token's expiry time has passed.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Mint a new signed token.
    ///
    /// `ttl_secs` is how long the token is valid from now.
    /// `key` is the HMAC key (AVIX_MASTER_KEY in production).
    pub fn mint(
        granted_tools: Vec<String>,
        issued_to: Option<IssuedTo>,
        ttl_secs: i64,
        key: &[u8],
    ) -> Self {
        let issued_at = Utc::now();
        let expires_at = issued_at + chrono::Duration::seconds(ttl_secs);
        let mut token = Self {
            granted_tools,
            issued_at,
            expires_at,
            issued_to,
            signature: String::new(),
        };
        token.signature = token.compute_signature(key);
        token
    }

    /// Verify that the token's signature matches the given key.
    pub fn verify_signature(&self, key: &[u8]) -> bool {
        let expected = self.compute_signature(key);
        self.signature == expected
    }

    /// Compute HMAC-SHA256 over the canonical payload.
    ///
    /// Canonical form: `<issued_at_unix>|<expires_at_unix>|<sorted_tools_csv>`
    /// Sorted tools ensures the signature is deterministic regardless of insertion order.
    fn compute_signature(&self, key: &[u8]) -> String {
        let mut tools = self.granted_tools.clone();
        tools.sort();
        let payload = format!(
            "{}|{}|{}",
            self.issued_at.timestamp(),
            self.expires_at.timestamp(),
            tools.join(","),
        );
        let mut mac =
            Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
        mac.update(payload.as_bytes());
        let result = mac.finalize().into_bytes();
        format!("sha256:{}", hex::encode(result))
    }

    /// Convenience constructor for tests.
    /// Creates a token valid for 1 hour with an unsigned `"test-sig"` signature.
    /// Never use this in production code.
    pub fn test_token(caps: &[&str]) -> Self {
        Self {
            granted_tools: caps.iter().map(|s| s.to_string()).collect(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
            issued_to: None,
            signature: "test-sig".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &[u8] = b"test-master-key-32-bytes-padded!";

    fn fresh_token(tools: &[&str]) -> CapabilityToken {
        CapabilityToken::mint(
            tools.iter().map(|s| s.to_string()).collect(),
            None,
            3600,
            TEST_KEY,
        )
    }

    #[test]
    fn test_has_tool_present() {
        let token = fresh_token(&["fs/read", "fs/write"]);
        assert!(token.has_tool("fs/read"));
        assert!(token.has_tool("fs/write"));
    }

    #[test]
    fn test_has_tool_absent() {
        let token = fresh_token(&["fs/read"]);
        assert!(!token.has_tool("fs/write"));
    }

    #[test]
    fn test_fresh_token_is_not_expired() {
        let token = fresh_token(&[]);
        assert!(!token.is_expired(), "a freshly minted token should not be expired");
    }

    #[test]
    fn test_expired_token_is_detected() {
        let mut token = fresh_token(&[]);
        // Back-date the expiry
        token.expires_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(token.is_expired(), "a past-expiry token should be expired");
    }

    #[test]
    fn test_signature_verifies_with_correct_key() {
        let token = fresh_token(&["fs/read", "agent/spawn"]);
        assert!(
            token.verify_signature(TEST_KEY),
            "signature should verify with the same key"
        );
    }

    #[test]
    fn test_signature_fails_with_wrong_key() {
        let token = fresh_token(&["fs/read"]);
        assert!(
            !token.verify_signature(b"wrong-key"),
            "signature should not verify with a different key"
        );
    }

    #[test]
    fn test_tampered_tools_invalidates_signature() {
        let mut token = fresh_token(&["fs/read"]);
        token.granted_tools.push("agent/spawn".into()); // tamper
        assert!(
            !token.verify_signature(TEST_KEY),
            "adding a tool after signing should invalidate the signature"
        );
    }

    #[test]
    fn test_tampered_expiry_invalidates_signature() {
        let mut token = fresh_token(&["fs/read"]);
        token.expires_at = token.expires_at + chrono::Duration::hours(10); // tamper
        assert!(
            !token.verify_signature(TEST_KEY),
            "changing expiry after signing should invalidate the signature"
        );
    }

    #[test]
    fn test_signature_is_deterministic_regardless_of_tool_order() {
        // Tools in different order should produce the same signature (sorted internally)
        let token_a = CapabilityToken::mint(
            vec!["fs/read".into(), "agent/spawn".into()],
            None,
            3600,
            TEST_KEY,
        );
        let mut token_b = token_a.clone();
        token_b.granted_tools = vec!["agent/spawn".into(), "fs/read".into()];
        // Both should verify
        assert!(token_a.verify_signature(TEST_KEY));
        assert!(token_b.verify_signature(TEST_KEY));
    }

    #[test]
    fn test_mint_sets_issued_to() {
        let issued_to = IssuedTo {
            pid: 42,
            agent_name: "researcher".into(),
            spawned_by: "alice".into(),
        };
        let token = CapabilityToken::mint(vec![], Some(issued_to), 3600, TEST_KEY);
        let it = token.issued_to.as_ref().unwrap();
        assert_eq!(it.pid, 42);
        assert_eq!(it.agent_name, "researcher");
    }

    #[test]
    fn test_test_token_is_not_expired() {
        let token = CapabilityToken::test_token(&["cap/list"]);
        assert!(!token.is_expired());
    }

    #[test]
    fn test_serde_round_trip() {
        let token = fresh_token(&["fs/read", "llm/complete"]);
        let json = serde_json::to_string(&token).unwrap();
        let decoded: CapabilityToken = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.granted_tools, token.granted_tools);
        assert_eq!(decoded.signature, token.signature);
        assert_eq!(decoded.issued_at.timestamp(), token.issued_at.timestamp());
    }
}
