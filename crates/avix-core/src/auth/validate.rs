use crate::config::CredentialType;

/// Validates a raw credential value against the stored credential config.
/// For now: api_key checks if value is non-empty and key_hash contains "test" (placeholder).
/// Full HMAC validation is on Day 11.
pub fn validate_credential(credential: &CredentialType, presented: &str) -> bool {
    match credential {
        CredentialType::ApiKey { .. } => !presented.is_empty(),
        CredentialType::Password { .. } => !presented.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_api_key_non_empty() {
        let cred = CredentialType::ApiKey {
            key_hash: "hmac-sha256:abcdef".into(),
            header: None,
        };
        assert!(validate_credential(&cred, "sk-test-123"));
    }

    #[test]
    fn test_validate_api_key_empty_presented_fails() {
        let cred = CredentialType::ApiKey {
            key_hash: "hmac-sha256:abcdef".into(),
            header: Some("x-api-key".into()),
        };
        assert!(!validate_credential(&cred, ""));
    }

    #[test]
    fn test_validate_password_non_empty() {
        let cred = CredentialType::Password {
            password_hash: "bcrypt:hash-here".into(),
        };
        assert!(validate_credential(&cred, "my-password"));
    }

    #[test]
    fn test_validate_password_empty_presented_fails() {
        let cred = CredentialType::Password {
            password_hash: "bcrypt:hash-here".into(),
        };
        assert!(!validate_credential(&cred, ""));
    }
}
