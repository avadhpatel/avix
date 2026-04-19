use crate::config::CredentialType;
use tracing::instrument;

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Validates a raw credential value against the stored credential config.
///
/// For `api_key`: the stored `key_hash` is `hmac-sha256:<hex>` where the hex is
/// HMAC-SHA256 of the raw key using the secret `b"config-init-secret"`. Verification
/// uses constant-time comparison via `hmac::Mac::verify_slice`.
///
/// For `password`: placeholder — accepts any non-empty value until bcrypt is wired up.
#[instrument(skip(credential, presented))]
pub fn validate_credential(credential: &CredentialType, presented: &str) -> bool {
    match credential {
        CredentialType::ApiKey { key_hash, .. } => verify_api_key(key_hash, presented),
        CredentialType::Password { .. } => !presented.is_empty(),
    }
}

#[instrument(skip(key_hash, presented))]
fn verify_api_key(key_hash: &str, presented: &str) -> bool {
    let hex_part = match key_hash.strip_prefix("hmac-sha256:") {
        Some(h) => h,
        None => return false,
    };
    let stored_bytes = match hex::decode(hex_part) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mut mac =
        HmacSha256::new_from_slice(b"config-init-secret").expect("HMAC accepts any key size");
    mac.update(presented.as_bytes());
    mac.verify_slice(&stored_bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key_hash(raw_key: &str) -> String {
        let mut mac =
            HmacSha256::new_from_slice(b"config-init-secret").expect("HMAC accepts any key size");
        mac.update(raw_key.as_bytes());
        format!("hmac-sha256:{}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn test_validate_api_key_correct() {
        let raw = "sk-avix-test-key-123";
        let cred = CredentialType::ApiKey {
            key_hash: make_key_hash(raw),
            header: None,
        };
        assert!(validate_credential(&cred, raw));
    }

    #[test]
    fn test_validate_api_key_wrong_key_fails() {
        let cred = CredentialType::ApiKey {
            key_hash: make_key_hash("sk-avix-correct"),
            header: None,
        };
        assert!(!validate_credential(&cred, "sk-avix-wrong"));
    }

    #[test]
    fn test_validate_api_key_empty_presented_fails() {
        let cred = CredentialType::ApiKey {
            key_hash: make_key_hash("sk-avix-something"),
            header: Some("x-api-key".into()),
        };
        assert!(!validate_credential(&cred, ""));
    }

    #[test]
    fn test_validate_api_key_bad_hash_prefix_fails() {
        let cred = CredentialType::ApiKey {
            key_hash: "sha256:abcdef".into(),
            header: None,
        };
        assert!(!validate_credential(&cred, "sk-avix-anything"));
    }

    #[test]
    fn test_validate_api_key_malformed_hex_fails() {
        let cred = CredentialType::ApiKey {
            key_hash: "hmac-sha256:not-valid-hex!!".into(),
            header: None,
        };
        assert!(!validate_credential(&cred, "sk-avix-anything"));
    }

    #[test]
    fn test_validate_password_non_empty() {
        let cred = CredentialType::Password {
            password_hash: "bcrypt:hash-here".into(),
        };
        assert!(validate_credential(&cred, "my-password"));
    }

    #[test]
    fn test_validate_password_empty_fails() {
        let cred = CredentialType::Password {
            password_hash: "bcrypt:hash-here".into(),
        };
        assert!(!validate_credential(&cred, ""));
    }
}
