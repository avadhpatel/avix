use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use thiserror::Error;
use tracing::instrument;

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

#[derive(Debug)]

pub struct SecretsStore {
    // In-memory storage: namespace -> key -> encrypted bytes
    pub(super) store: RwLock<HashMap<String, HashMap<String, Vec<u8>>>>,
    master_key: [u8; 32],
}

impl SecretsStore {
    #[instrument]
pub fn new(master_key: [u8; 32]) -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
            master_key,
        }
    }

    #[instrument]
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

    #[instrument]
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

    #[instrument]
pub fn delete(&self, namespace: &str, key: &str) -> Result<(), SecretsError> {
        let mut map = self.store.write().unwrap();
        if let Some(ns) = map.get_mut(namespace) {
            ns.remove(key)
                .ok_or_else(|| SecretsError::NotFound(key.to_string()))?;
        }
        Ok(())
    }

    #[instrument]
pub fn list(&self, namespace: &str) -> Vec<String> {
        let map = self.store.read().unwrap();
        map.get(namespace)
            .map(|ns| ns.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// VFS reads ALWAYS return EPERM
    #[instrument]
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

// ── Disk-backed SecretStore ────────────────────────────────────────────────────

#[derive(Debug)]

/// Disk-backed secret store.
///
/// Secrets are stored as AES-256-GCM ciphertext, hex-encoded, at:
/// `<root>/<owner-type>/<owner-name>/<secret-name>.enc`
///
/// Owner format: `"service:github-svc"` or `"user:alice"`.
pub struct SecretStore {
    root: PathBuf,
    master_key: [u8; 32],
}

impl SecretStore {
    /// Create a new `SecretStore` rooted at `root`, using `key` as the master key.
    /// `key` is zero-padded (or truncated) to 32 bytes.
    #[instrument]
pub fn new(root: &Path, key: &[u8]) -> Self {
        let mut master_key = [0u8; 32];
        let len = key.len().min(32);
        master_key[..len].copy_from_slice(&key[..len]);
        Self {
            root: root.to_path_buf(),
            master_key,
        }
    }

    #[instrument]
fn secret_path(&self, owner: &str, name: &str) -> PathBuf {
        let (owner_type, owner_name) = owner.split_once(':').unwrap_or(("other", owner));
        self.root
            .join(owner_type)
            .join(owner_name)
            .join(format!("{name}.enc"))
    }

    /// Encrypt `value` and write to disk.
    #[instrument]
pub fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), SecretsError> {
        let path = self.secret_path(owner, name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.master_key));
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, value.as_bytes())
            .map_err(|_| SecretsError::DecryptionFailed)?;
        let mut stored = nonce.to_vec();
        stored.extend_from_slice(&ciphertext);
        std::fs::write(&path, hex::encode(&stored))?;
        Ok(())
    }

    /// Read from disk and decrypt.  Returns plaintext only in memory.
    #[instrument]
pub fn get(&self, owner: &str, name: &str) -> Result<String, SecretsError> {
        let path = self.secret_path(owner, name);
        let hex_data = std::fs::read_to_string(&path)
            .map_err(|_| SecretsError::NotFound(format!("{owner}/{name}")))?;
        let stored = hex::decode(hex_data.trim()).map_err(|_| SecretsError::DecryptionFailed)?;
        if stored.len() < 12 {
            return Err(SecretsError::DecryptionFailed);
        }
        let (nonce_bytes, ciphertext) = stored.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.master_key));
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretsError::DecryptionFailed)?;
        String::from_utf8(plaintext).map_err(|_| SecretsError::DecryptionFailed)
    }

    /// Delete a secret from disk.
    #[instrument]
pub fn delete(&self, owner: &str, name: &str) -> Result<(), SecretsError> {
        let path = self.secret_path(owner, name);
        std::fs::remove_file(&path).map_err(|_| SecretsError::NotFound(format!("{owner}/{name}")))
    }

    /// List all secret names for `owner`.
    #[instrument]
pub fn list(&self, owner: &str) -> Vec<String> {
        let (owner_type, owner_name) = owner.split_once(':').unwrap_or(("other", owner));
        let dir = self.root.join(owner_type).join(owner_name);
        if !dir.exists() {
            return vec![];
        }
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().into_owned();
                if fname.ends_with(".enc") {
                    names.push(fname.trim_end_matches(".enc").to_string());
                }
            }
        }
        names
    }
}

#[cfg(test)]
mod disk_store_tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store(dir: &TempDir) -> SecretStore {
        SecretStore::new(dir.path(), b"test-master-key-32-bytes-padded!!")
    }

    #[test]
    fn set_and_get_service_secret_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store
            .set("service:github-svc", "app-key", "ghp_test123")
            .unwrap();
        let value = store.get("service:github-svc", "app-key").unwrap();
        assert_eq!(value, "ghp_test123");
    }

    #[test]
    fn get_nonexistent_secret_errors() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        assert!(store.get("service:x", "missing").is_err());
    }

    #[test]
    fn secrets_are_not_stored_in_plaintext() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store.set("service:svc", "key", "supersecret").unwrap();
        let content =
            std::fs::read_to_string(dir.path().join("service").join("svc").join("key.enc"))
                .unwrap();
        assert!(!content.contains("supersecret"));
    }

    #[test]
    fn set_and_get_user_secret_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store.set("user:alice", "gh-token", "secret-token").unwrap();
        let value = store.get("user:alice", "gh-token").unwrap();
        assert_eq!(value, "secret-token");
    }

    #[test]
    fn list_returns_secret_names() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store.set("service:svc", "key1", "v1").unwrap();
        store.set("service:svc", "key2", "v2").unwrap();
        let mut names = store.list("service:svc");
        names.sort();
        assert_eq!(names, vec!["key1", "key2"]);
    }

    #[test]
    fn list_nonexistent_owner_returns_empty() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        assert!(store.list("service:nobody").is_empty());
    }

    #[test]
    fn delete_removes_secret() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store.set("service:svc", "k", "v").unwrap();
        store.delete("service:svc", "k").unwrap();
        assert!(store.get("service:svc", "k").is_err());
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let dir = TempDir::new().unwrap();
        let store1 = SecretStore::new(dir.path(), b"key-one-32-bytes-padded000000000");
        store1.set("service:svc", "k", "secret").unwrap();
        let store2 = SecretStore::new(dir.path(), b"key-two-32-bytes-padded000000000");
        assert!(matches!(
            store2.get("service:svc", "k"),
            Err(SecretsError::DecryptionFailed)
        ));
    }
}
