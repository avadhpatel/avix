use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::AvixError;
use crate::types::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ATPTokenClaims {
    pub session_id: String,
    pub identity_name: String,
    pub role: Role,
    pub expires_at: DateTime<Utc>,
}

pub struct ATPToken;

impl ATPToken {
    pub fn issue(claims: ATPTokenClaims, secret: &str) -> Result<String, AvixError> {
        let json =
            serde_json::to_string(&claims).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let sig = Self::sign(json.as_bytes(), secret.as_bytes());
        let payload = base64_encode(json.as_bytes());
        Ok(format!("{payload}.{sig}"))
    }

    pub fn validate(token: &str, secret: &str) -> Result<ATPTokenClaims, AvixError> {
        let parts: Vec<&str> = token.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(AvixError::CapabilityDenied("invalid token format".into()));
        }
        let payload_bytes = base64_decode(parts[0]).map_err(AvixError::CapabilityDenied)?;
        let expected_sig = Self::sign(&payload_bytes, secret.as_bytes());
        if parts[1] != expected_sig {
            return Err(AvixError::CapabilityDenied(
                "invalid token signature".into(),
            ));
        }
        let claims: ATPTokenClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        if claims.expires_at < Utc::now() {
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
    use std::fmt::Write;
    let encoded = data.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{:02x}", b);
        acc
    });
    encoded
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("invalid hex length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
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
}
