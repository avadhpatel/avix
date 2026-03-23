use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::AvixError;
use crate::types::Role;

/// Claims embedded in an ATPToken (§3.1).
/// Serialised to JSON then base64url-encoded before HMAC signing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ATPTokenClaims {
    /// Username (subject).
    pub sub: String,
    /// Numeric user ID.
    pub uid: u32,
    pub role: Role,
    /// Crew memberships.
    pub crews: Vec<String>,
    /// Session identifier.
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// Issued-at timestamp.
    pub iat: DateTime<Utc>,
    /// Expiry timestamp.
    pub exp: DateTime<Utc>,
    /// Permitted ATP domains, e.g. `["proc", "fs", "signal"]`.
    pub scope: Vec<String>,
}

impl ATPTokenClaims {
    pub fn is_expired(&self) -> bool {
        self.exp < Utc::now()
    }

    /// True when fewer than 5 minutes remain before expiry.
    pub fn is_expiring_soon(&self) -> bool {
        let remaining = self.exp.signed_duration_since(Utc::now());
        remaining < chrono::Duration::minutes(5) && remaining > chrono::Duration::zero()
    }
}

pub struct ATPToken;

impl ATPToken {
    pub fn issue(claims: ATPTokenClaims, secret: &str) -> Result<String, AvixError> {
        let json =
            serde_json::to_string(&claims).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let payload = base64_encode(json.as_bytes());
        let sig = Self::sign(payload.as_bytes(), secret.as_bytes());
        Ok(format!("{payload}.{sig}"))
    }

    pub fn validate(token: &str, secret: &str) -> Result<ATPTokenClaims, AvixError> {
        let parts: Vec<&str> = token.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(AvixError::CapabilityDenied("invalid token format".into()));
        }
        let expected_sig = Self::sign(parts[0].as_bytes(), secret.as_bytes());
        if parts[1] != expected_sig {
            return Err(AvixError::CapabilityDenied(
                "invalid token signature".into(),
            ));
        }
        let payload_bytes = base64_decode(parts[0]).map_err(AvixError::CapabilityDenied)?;
        let claims: ATPTokenClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        if claims.is_expired() {
            return Err(AvixError::CapabilityDenied("token expired".into()));
        }
        Ok(claims)
    }

    fn sign(data: &[u8], secret: &[u8]) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key size");
        mac.update(data);
        hex::encode(mac.finalize().into_bytes())
    }
}

fn base64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| e.to_string())
}

#[derive(Default)]
pub struct ATPTokenStore {
    secret: String,
    revoked: Arc<RwLock<HashSet<String>>>,
}

impl ATPTokenStore {
    pub fn new(secret: String) -> Self {
        Self {
            secret,
            revoked: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub async fn issue(&self, claims: ATPTokenClaims) -> Result<String, AvixError> {
        ATPToken::issue(claims, &self.secret)
    }

    pub async fn validate(&self, token: &str) -> Result<ATPTokenClaims, AvixError> {
        let claims = ATPToken::validate(token, &self.secret)?;
        if self.revoked.read().await.contains(&claims.session_id) {
            return Err(AvixError::CapabilityDenied("token revoked".into()));
        }
        Ok(claims)
    }

    pub async fn revoke(&self, session_id: &str) {
        self.revoked.write().await.insert(session_id.to_string());
    }

    /// Returns `true` when the token is valid and expires within 5 minutes.
    pub async fn is_expiring_soon(&self, token: &str) -> Result<bool, AvixError> {
        let claims = self.validate(token).await?;
        Ok(claims.is_expiring_soon())
    }
}
