# Day 23 — Secrets Store

> **Goal:** Implement the AES-256-GCM encrypted secrets store under `/secrets/`. Kernel-mediated injection only — secrets are never readable via VFS. Implement local encrypted store (default) with extensibility hooks for AWS KMS, GCP Cloud KMS, HashiCorp Vault.

---

## Pre-flight: Verify Day 22

```bash
cargo test --workspace
grep -r "PipeRegistry" crates/avix-core/src/
grep -r "SIGPIPE"      crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Add Crypto Dependencies

In `crates/avix-core/Cargo.toml`:

```toml
[dependencies]
aes-gcm    = "0.10"
rand       = "0.8"
base64     = "0.21"
```

Add to `src/lib.rs`: `pub mod secrets;`

```
src/secrets/
├── mod.rs
├── store.rs       ← SecretsStore (local AES-256-GCM)
├── inject.rs      ← kernel injection logic
└── backend.rs     ← SecretsBackend trait for extensibility
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/secrets.rs`:

```rust
use avix_core::secrets::{SecretsStore, SecretsBackend};
use avix_core::types::Pid;
use tempfile::tempdir;

fn master_key() -> [u8; 32] { [0x42u8; 32] }

// ── Store and retrieve ────────────────────────────────────────────────────────

#[tokio::test]
async fn store_and_retrieve_secret() {
    let tmp = tempdir().unwrap();
    let store = SecretsStore::open(tmp.path(), master_key()).await.unwrap();
    store.put("alice", "api-key", b"sk-real-key-here").await.unwrap();
    let value = store.kernel_get("alice", "api-key").await.unwrap();
    assert_eq!(value, b"sk-real-key-here");
}

// ── Encryption at rest ────────────────────────────────────────────────────────

#[tokio::test]
async fn secret_is_encrypted_at_rest() {
    let tmp = tempdir().unwrap();
    let store = SecretsStore::open(tmp.path(), master_key()).await.unwrap();
    store.put("alice", "my-key", b"plaintext-secret").await.unwrap();

    // Read raw bytes from disk
    let raw = std::fs::read(tmp.path().join("alice/my-key.enc")).unwrap();
    // Must not contain the plaintext
    assert!(!raw.windows(16).any(|w| w == b"plaintext-secret"));
}

// ── VFS read is blocked ───────────────────────────────────────────────────────

#[tokio::test]
async fn vfs_read_of_secrets_path_is_forbidden() {
    let tmp = tempdir().unwrap();
    let store = SecretsStore::open(tmp.path(), master_key()).await.unwrap();
    store.put("alice", "my-key", b"val").await.unwrap();

    // Any attempt to read via VFS must fail
    let result = store.vfs_read("/secrets/alice/my-key.enc").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("forbidden") ||
            result.unwrap_err().to_string().contains("EPERM"));
}

// ── Kernel injection ──────────────────────────────────────────────────────────

#[tokio::test]
async fn inject_secret_into_env_for_agent() {
    let tmp = tempdir().unwrap();
    let store = SecretsStore::open(tmp.path(), master_key()).await.unwrap();
    store.put("alice", "api-key", b"sk-injected").await.unwrap();

    let env = store.inject_into_env(Pid::new(57), "alice", &["api-key"]).await.unwrap();
    assert_eq!(env.get("AVIX_SECRET_API_KEY").map(|s| s.as_bytes()), Some(b"sk-injected" as &[u8]));
}

#[tokio::test]
async fn inject_missing_secret_returns_error() {
    let tmp = tempdir().unwrap();
    let store = SecretsStore::open(tmp.path(), master_key()).await.unwrap();
    let result = store.inject_into_env(Pid::new(57), "alice", &["nonexistent"]).await;
    assert!(result.is_err());
}

// ── Delete ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_secret_removes_enc_file() {
    let tmp = tempdir().unwrap();
    let store = SecretsStore::open(tmp.path(), master_key()).await.unwrap();
    store.put("alice", "del-key", b"value").await.unwrap();
    store.delete("alice", "del-key").await.unwrap();
    assert!(store.kernel_get("alice", "del-key").await.is_err());
}

// ── Wrong master key ──────────────────────────────────────────────────────────

#[tokio::test]
async fn wrong_master_key_fails_decryption() {
    let tmp = tempdir().unwrap();
    let store1 = SecretsStore::open(tmp.path(), [0x42u8; 32]).await.unwrap();
    store1.put("alice", "key", b"secret").await.unwrap();

    let store2 = SecretsStore::open(tmp.path(), [0x99u8; 32]).await.unwrap();
    let result = store2.kernel_get("alice", "key").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("decrypt") ||
            result.unwrap_err().to_string().contains("decrypt failure"));
}

// ── SecretsBackend trait ──────────────────────────────────────────────────────

#[tokio::test]
async fn local_backend_implements_trait() {
    let tmp = tempdir().unwrap();
    let backend: Box<dyn SecretsBackend> = Box::new(
        avix_core::secrets::LocalBackend::open(tmp.path(), master_key()).await.unwrap()
    );
    backend.put("svc", "key", b"val").await.unwrap();
    let v = backend.get("svc", "key").await.unwrap();
    assert_eq!(v, b"val");
}
```

---

## Step 3 — Implement

`SecretsStore` stores AES-256-GCM encrypted blobs at `<root>/<namespace>/<key>.enc`. The nonce is prepended to the ciphertext. `vfs_read` always returns `EPERM`. `inject_into_env` decrypts and places in a `HashMap<String, String>` with key `AVIX_SECRET_<NAME_UPPER>`.

`SecretsBackend` trait: `async fn put(&self, namespace, key, value)` + `async fn get(...)` + `async fn delete(...)`.

---

## Step 4 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-23: SecretsStore — AES-256-GCM, VFS block, kernel injection, extensible backend"
```

## Success Criteria

- [ ] Store/retrieve preserves exact value
- [ ] `.enc` file on disk doesn't contain plaintext
- [ ] VFS read of `/secrets/` always returns EPERM
- [ ] Kernel injection produces correct env var names
- [ ] Wrong master key fails decryption
- [ ] Delete removes encrypted file
- [ ] `SecretsBackend` trait implemented by `LocalBackend`
- [ ] 15+ tests pass, 0 clippy warnings

---
---

