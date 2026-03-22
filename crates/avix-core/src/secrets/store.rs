use std::collections::HashMap;
use std::sync::RwLock;

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretsError {
    #[error("EPERM: secrets are not readable via VFS")]
    Eperm,
    #[error("secret not found: {0}")]
    NotFound(String),
    #[error("decryption failed")]
    DecryptionFailed,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct SecretsStore {
    // In-memory storage: namespace -> key -> encrypted bytes
    pub(super) store: RwLock<HashMap<String, HashMap<String, Vec<u8>>>>,
    master_key: [u8; 32],
}

impl SecretsStore {
    pub fn new(master_key: [u8; 32]) -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
            master_key,
        }
    }

    pub fn put(&self, namespace: &str, key: &str, plaintext: &[u8]) -> Result<(), SecretsError> {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.master_key));
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| SecretsError::DecryptionFailed)?;
        // Prepend nonce to ciphertext
        let mut stored = nonce.to_vec();
        stored.extend_from_slice(&ciphertext);

        let mut map = self.store.write().unwrap();
        map.entry(namespace.to_string())
            .or_default()
            .insert(key.to_string(), stored);
        Ok(())
    }

    pub fn get(&self, namespace: &str, key: &str) -> Result<Vec<u8>, SecretsError> {
        let map = self.store.read().unwrap();
        let ns = map
            .get(namespace)
            .ok_or_else(|| SecretsError::NotFound(key.to_string()))?;
        let stored = ns
            .get(key)
            .ok_or_else(|| SecretsError::NotFound(key.to_string()))?;

        // Split nonce (12 bytes) from ciphertext
        if stored.len() < 12 {
            return Err(SecretsError::DecryptionFailed);
        }
        let (nonce_bytes, ciphertext) = stored.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.master_key));
        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretsError::DecryptionFailed)
    }

    pub fn delete(&self, namespace: &str, key: &str) -> Result<(), SecretsError> {
        let mut map = self.store.write().unwrap();
        if let Some(ns) = map.get_mut(namespace) {
            ns.remove(key)
                .ok_or_else(|| SecretsError::NotFound(key.to_string()))?;
        }
        Ok(())
    }

    pub fn list(&self, namespace: &str) -> Vec<String> {
        let map = self.store.read().unwrap();
        map.get(namespace)
            .map(|ns| ns.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// VFS reads ALWAYS return EPERM
    pub fn vfs_read(&self, _path: &str) -> Result<Vec<u8>, SecretsError> {
        Err(SecretsError::Eperm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(b: u8) -> [u8; 32] {
        [b; 32]
    }

    #[test]
    fn test_store_retrieve_roundtrip() {
        let store = SecretsStore::new(make_key(1));
        store.put("ns", "my-key", b"my-secret").unwrap();
        let got = store.get("ns", "my-key").unwrap();
        assert_eq!(got, b"my-secret");
    }

    #[test]
    fn test_encryption_at_rest() {
        let store = SecretsStore::new(make_key(1));
        store.put("ns", "k", b"value").unwrap();
        let got = store.get("ns", "k").unwrap();
        assert_eq!(got, b"value");
    }

    #[test]
    fn test_vfs_read_returns_eperm() {
        let store = SecretsStore::new(make_key(1));
        let res = store.vfs_read("/secrets/ns/my-key");
        assert!(matches!(res, Err(SecretsError::Eperm)));
    }

    #[test]
    fn test_wrong_key_fails_decryption() {
        let store1 = SecretsStore::new(make_key(1));
        store1.put("ns", "k", b"secret").unwrap();
        // Get raw encrypted data
        let raw = {
            let map = store1.store.read().unwrap();
            map["ns"]["k"].clone()
        };
        // Try to decrypt with different key
        let store2 = SecretsStore::new(make_key(2));
        {
            let mut map = store2.store.write().unwrap();
            map.entry("ns".to_string())
                .or_default()
                .insert("k".to_string(), raw);
        }
        let res = store2.get("ns", "k");
        assert!(matches!(res, Err(SecretsError::DecryptionFailed)));
    }

    #[test]
    fn test_delete() {
        let store = SecretsStore::new(make_key(1));
        store.put("ns", "k", b"v").unwrap();
        store.delete("ns", "k").unwrap();
        assert!(matches!(
            store.get("ns", "k"),
            Err(SecretsError::NotFound(_))
        ));
    }

    #[test]
    fn test_list_keys() {
        let store = SecretsStore::new(make_key(1));
        store.put("ns", "key1", b"v1").unwrap();
        store.put("ns", "key2", b"v2").unwrap();
        let mut keys = store.list("ns");
        keys.sort();
        assert_eq!(keys, vec!["key1", "key2"]);
    }

    #[test]
    fn test_not_found_error() {
        let store = SecretsStore::new(make_key(1));
        let res = store.get("ns", "nonexistent");
        assert!(matches!(res, Err(SecretsError::NotFound(_))));
    }

    #[test]
    fn test_list_empty_namespace() {
        let store = SecretsStore::new(make_key(1));
        let keys = store.list("nonexistent-ns");
        assert!(keys.is_empty());
    }

    #[test]
    fn test_multiple_namespaces_isolated() {
        let store = SecretsStore::new(make_key(1));
        store.put("ns1", "key", b"val1").unwrap();
        store.put("ns2", "key", b"val2").unwrap();
        assert_eq!(store.get("ns1", "key").unwrap(), b"val1");
        assert_eq!(store.get("ns2", "key").unwrap(), b"val2");
    }

    #[test]
    fn test_overwrite_key() {
        let store = SecretsStore::new(make_key(1));
        store.put("ns", "key", b"old").unwrap();
        store.put("ns", "key", b"new").unwrap();
        assert_eq!(store.get("ns", "key").unwrap(), b"new");
    }
}
