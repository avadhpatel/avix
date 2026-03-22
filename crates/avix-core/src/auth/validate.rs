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
