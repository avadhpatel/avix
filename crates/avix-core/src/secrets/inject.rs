use std::collections::HashMap;

use super::store::{SecretsError, SecretsStore};
use tracing::instrument;

/// Inject secrets for an agent into an environment map
#[instrument]
pub fn inject_secrets(
    store: &SecretsStore,
    namespace: &str,
    env: &mut HashMap<String, String>,
) -> Result<(), SecretsError> {
    let keys = store.list(namespace);
    for key in keys {
        let plaintext = store.get(namespace, &key)?;
        let value = String::from_utf8_lossy(&plaintext).into_owned();
        let env_name = format!("AVIX_SECRET_{}", key.to_uppercase().replace('-', "_"));
        env.insert(env_name, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::store::SecretsStore;

    #[test]
    fn test_inject_secrets_env_format() {
        let store = SecretsStore::new([1u8; 32]);
        store.put("agent-42", "api-key", b"my-api-key").unwrap();
        let mut env = HashMap::new();
        inject_secrets(&store, "agent-42", &mut env).unwrap();
        assert_eq!(env.get("AVIX_SECRET_API_KEY").unwrap(), "my-api-key");
    }

    #[test]
    fn test_inject_multiple_secrets() {
        let store = SecretsStore::new([1u8; 32]);
        store.put("ns", "db-password", b"secret123").unwrap();
        store.put("ns", "api-token", b"tok-abc").unwrap();
        let mut env = HashMap::new();
        inject_secrets(&store, "ns", &mut env).unwrap();
        assert_eq!(env.get("AVIX_SECRET_DB_PASSWORD").unwrap(), "secret123");
        assert_eq!(env.get("AVIX_SECRET_API_TOKEN").unwrap(), "tok-abc");
    }

    #[test]
    fn test_inject_empty_namespace_no_env() {
        let store = SecretsStore::new([1u8; 32]);
        let mut env = HashMap::new();
        inject_secrets(&store, "empty-ns", &mut env).unwrap();
        assert!(env.is_empty());
    }
}
