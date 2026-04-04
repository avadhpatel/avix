# pkg-gap-F — Third-Party Trust Keyring

> **Status:** Pending
> **Priority:** Medium
> **Depends on:** pkg-gap-D (GPG verification infrastructure)
> **Blocks:** nothing
> **Affects:**
> - `crates/avix-core/src/packaging/trust.rs` (new)
> - `crates/avix-core/src/syscall/domain/pkg_.rs` (updated GPG verification)
> - `crates/avix-cli/src/main.rs` (`avix package trust` subcommands)

---

## Problem

pkg-gap-D embeds a single hardcoded official Avix public key. There is no way for an
admin to add keys from third-party publishers (companies, community developers) so their
signed packages are verified automatically. Without this, every non-official package either
requires `install:from-untrusted-source` capability or skips signature verification entirely.

---

## Scope

1. **`TrustStore`** — disk-backed keyring at `AVIX_ROOT/etc/avix/trusted-keys/`.
2. **`TrustedKey`** — metadata struct: fingerprint, label, allowed source patterns, added date.
3. **Updated GPG verification** — check official embedded key first, then `TrustStore`.
4. **Kernel syscalls** — `proc/package/trust-add`, `proc/package/trust-list`, `proc/package/trust-remove`.
5. **CLI** — `avix package trust add/list/remove`.

---

## Keyring Directory Layout

```
AVIX_ROOT/
└── etc/avix/trusted-keys/
    ├── <fingerprint>.asc           # ASCII-armored public key (e.g. DEADBEEF…CAFE.asc)
    └── <fingerprint>.meta.yaml     # label, added_at, allowed_sources
```

Example `.meta.yaml`:
```yaml
fingerprint: DEADBEEF1234CAFE
label: "AcmeCorp"
added_at: "2026-04-04T10:00:00Z"
allowed_sources:
  - "github:acmecorp/*"
  - "https://packages.acmecorp.com/*"
```

`allowed_sources` is an optional list of glob patterns. If absent or empty, the key is
trusted for packages from **any** source. Patterns are matched against the resolved
`PackageSource` string before verification.

---

## What to Build

### 1. `TrustedKey` and `TrustStore` — `crates/avix-core/src/packaging/trust.rs`

```rust
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::error::AvixError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedKey {
    pub fingerprint: String,
    pub label: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    /// Glob patterns for allowed package sources.
    /// Empty = trusted for all sources.
    #[serde(default)]
    pub allowed_sources: Vec<String>,
}

impl TrustedKey {
    /// Returns true if this key is allowed to sign packages from `source`.
    pub fn allows_source(&self, source: &str) -> bool {
        if self.allowed_sources.is_empty() {
            return true;
        }
        self.allowed_sources.iter().any(|pattern| glob_match(pattern, source))
    }
}

pub struct TrustStore {
    dir: PathBuf,
}

impl TrustStore {
    pub fn new(root: &Path) -> Self {
        Self { dir: root.join("etc/avix/trusted-keys") }
    }

    /// Add a trusted key from ASCII-armored key data.
    ///
    /// Extracts the fingerprint from the key, writes `<fingerprint>.asc`
    /// and `<fingerprint>.meta.yaml` into the keyring directory.
    pub fn add(
        &self,
        key_asc: &str,
        label: &str,
        allowed_sources: Vec<String>,
    ) -> Result<TrustedKey, AvixError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let fingerprint = extract_fingerprint(key_asc)?;

        let key_path = self.dir.join(format!("{fingerprint}.asc"));
        let meta_path = self.dir.join(format!("{fingerprint}.meta.yaml"));

        if key_path.exists() {
            return Err(AvixError::ConfigParse(format!(
                "key already trusted: {fingerprint}"
            )));
        }

        let trusted = TrustedKey {
            fingerprint: fingerprint.clone(),
            label: label.to_owned(),
            added_at: chrono::Utc::now(),
            allowed_sources,
        };

        std::fs::write(&key_path, key_asc)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let meta_yaml = serde_yaml::to_string(&trusted)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(&meta_path, meta_yaml)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Ok(trusted)
    }

    /// List all trusted keys in the keyring.
    pub fn list(&self) -> Result<Vec<TrustedKey>, AvixError> {
        if !self.dir.exists() {
            return Ok(vec![]);
        }
        let mut keys = Vec::new();
        for entry in std::fs::read_dir(&self.dir)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let key: TrustedKey = serde_yaml::from_str(&content)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                keys.push(key);
            }
        }
        keys.sort_by(|a, b| a.added_at.cmp(&b.added_at));
        Ok(keys)
    }

    /// Remove a key by fingerprint.
    pub fn remove(&self, fingerprint: &str) -> Result<(), AvixError> {
        let key_path = self.dir.join(format!("{fingerprint}.asc"));
        let meta_path = self.dir.join(format!("{fingerprint}.meta.yaml"));
        if !key_path.exists() {
            return Err(AvixError::ConfigParse(format!(
                "key not found: {fingerprint}"
            )));
        }
        std::fs::remove_file(&key_path)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::remove_file(&meta_path)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    /// Look up a key by fingerprint and return its ASCII-armored data + metadata.
    pub fn get(&self, fingerprint: &str) -> Result<Option<(String, TrustedKey)>, AvixError> {
        let key_path = self.dir.join(format!("{fingerprint}.asc"));
        let meta_path = self.dir.join(format!("{fingerprint}.meta.yaml"));
        if !key_path.exists() {
            return Ok(None);
        }
        let key_asc = std::fs::read_to_string(&key_path)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let meta: TrustedKey = serde_yaml::from_str(
            &std::fs::read_to_string(&meta_path)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
        ).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(Some((key_asc, meta)))
    }
}

/// Extract the primary key fingerprint from ASCII-armored key data.
fn extract_fingerprint(key_asc: &str) -> Result<String, AvixError> {
    use pgp::Deserializable;
    let (pubkey, _) = pgp::SignedPublicKey::from_string(key_asc)
        .map_err(|e| AvixError::ConfigParse(format!("invalid key: {e}")))?;
    Ok(hex::encode(pubkey.fingerprint()).to_uppercase())
}

/// Simple glob pattern match supporting `*` as a wildcard segment.
fn glob_match(pattern: &str, value: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return value.starts_with(&format!("{prefix}/")) || value == prefix;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    pattern == value
}
```

Wire into `packaging/mod.rs`:
```rust
pub mod trust;
pub use trust::{TrustStore, TrustedKey};
```

---

### 2. Updated GPG verification in `packaging/gpg.rs`

Replace the single-key check from pkg-gap-D with a two-stage lookup:

```rust
/// Verify `data` against `sig_asc`, checking the official embedded key first,
/// then falling back to the `TrustStore`.
///
/// Returns `Ok(VerifiedBy::Official)` or `Ok(VerifiedBy::Trusted(key))`.
/// Returns `Err` if the signature is invalid or the signing key is not trusted.
pub fn verify_signature(
    data: &[u8],
    sig_asc: &str,
    source: &str,
    trust_store: &TrustStore,
) -> Result<VerifiedBy, AvixError> {
    use pgp::{Deserializable, StandaloneSignature};

    let (sig, _) = StandaloneSignature::from_string(sig_asc)
        .map_err(|e| AvixError::ConfigParse(format!("parse signature: {e}")))?;

    // Stage 1: official embedded key.
    let (official_key, _) = pgp::SignedPublicKey::from_string(AVIX_PUBLIC_KEY)
        .map_err(|e| AvixError::ConfigParse(format!("parse official key: {e}")))?;
    if sig.verify(&official_key, data).is_ok() {
        return Ok(VerifiedBy::Official);
    }

    // Stage 2: TrustStore — find key by fingerprint from the signature issuer.
    let issuer = sig_issuer_fingerprint(&sig);
    if let Some(fingerprint) = issuer {
        if let Some((key_asc, meta)) = trust_store.get(&fingerprint)? {
            if !meta.allows_source(source) {
                return Err(AvixError::ConfigParse(format!(
                    "key '{label}' ({fingerprint}) is not trusted for source '{source}'",
                    label = meta.label,
                )));
            }
            let (pubkey, _) = pgp::SignedPublicKey::from_string(&key_asc)
                .map_err(|e| AvixError::ConfigParse(format!("load trusted key: {e}")))?;
            sig.verify(&pubkey, data)
                .map_err(|e| AvixError::ConfigParse(format!("signature invalid: {e}")))?;
            return Ok(VerifiedBy::Trusted(meta));
        }
    }

    Err(AvixError::ConfigParse(
        "signing key is not in the official keyring or trust store — \
         add it with `avix package trust add`".into(),
    ))
}

#[derive(Debug)]
pub enum VerifiedBy {
    Official,
    Trusted(TrustedKey),
}

fn sig_issuer_fingerprint(sig: &pgp::StandaloneSignature) -> Option<String> {
    // Extract the issuer fingerprint subpacket from the signature.
    // Returns uppercase hex fingerprint or None if not present.
    sig.signature.issuer_fingerprint()
        .map(|fp| hex::encode(fp).to_uppercase())
}
```

Update callers in `pkg_.rs` (`install_agent`, `install_service`) to pass `trust_store`
and match on `VerifiedBy` for logging:

```rust
match verify_signature(&bytes, &sig_asc, source, &trust_store)? {
    VerifiedBy::Official => tracing::info!("package verified by official Avix key"),
    VerifiedBy::Trusted(key) => tracing::info!("package verified by trusted key: {}", key.label),
}
```

---

### 3. Kernel syscalls — `proc/package/trust-*`

Add to `crates/avix-core/src/syscall/domain/pkg_.rs`:

```rust
/// `proc/package/trust-add`
///
/// Required capability: `auth:admin`.
/// Body: { key_asc: string, label: string, allowed_sources?: [string] }
pub fn trust_add(ctx: &SyscallContext, params: Value, trust_store: &TrustStore) -> SyscallResult {
    check_capability(ctx, "auth:admin")?;

    let key_asc = params["key_asc"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing key_asc".into()))?;
    let label = params["label"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing label".into()))?;
    let allowed_sources = params["allowed_sources"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_owned())).collect())
        .unwrap_or_default();

    let key = trust_store.add(key_asc, label, allowed_sources)
        .map_err(|e| SyscallError::Einval(e.to_string()))?;

    Ok(json!({
        "fingerprint": key.fingerprint,
        "label":       key.label,
        "added_at":    key.added_at.to_rfc3339(),
    }))
}

/// `proc/package/trust-list`
///
/// No capability required — any authenticated user can see the keyring.
pub fn trust_list(_ctx: &SyscallContext, _params: Value, trust_store: &TrustStore) -> SyscallResult {
    let keys = trust_store.list()
        .map_err(|e| SyscallError::Eio(e.to_string()))?;
    let entries: Vec<_> = keys.iter().map(|k| json!({
        "fingerprint":    k.fingerprint,
        "label":          k.label,
        "added_at":       k.added_at.to_rfc3339(),
        "allowed_sources": k.allowed_sources,
    })).collect();
    Ok(json!({ "keys": entries }))
}

/// `proc/package/trust-remove`
///
/// Required capability: `auth:admin`.
pub fn trust_remove(ctx: &SyscallContext, params: Value, trust_store: &TrustStore) -> SyscallResult {
    check_capability(ctx, "auth:admin")?;
    let fingerprint = params["fingerprint"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing fingerprint".into()))?;
    trust_store.remove(fingerprint)
        .map_err(|e| SyscallError::Einval(e.to_string()))?;
    Ok(json!({ "removed": fingerprint }))
}
```

Register all three in `registry.rs` and `handler.rs`.

---

### 4. CLI — `avix package trust` subcommands

Add to `PackageCmd` in `main.rs`:

```rust
/// Manage trusted third-party signing keys
Trust {
    #[command(subcommand)]
    sub: TrustCmd,
},
```

```rust
#[derive(Subcommand)]
enum TrustCmd {
    /// Add a trusted signing key
    Add {
        /// Path to a local .asc key file, or https:// URL to fetch it from
        key: String,
        /// Human-readable label for this key (e.g. "AcmeCorp")
        #[arg(long)]
        name: String,
        /// Restrict this key to specific source patterns (e.g. "github:acmecorp/*")
        /// May be specified multiple times. Omit to trust for all sources.
        #[arg(long = "allow-source")]
        allow_sources: Vec<String>,
    },
    /// List all trusted keys
    List,
    /// Remove a trusted key by fingerprint
    Remove {
        fingerprint: String,
    },
}
```

Handler logic — `Add` fetches the key (local file or HTTPS URL), sends
`proc/package/trust-add` via ATP. `List` and `Remove` follow the same pattern.

```rust
TrustCmd::Add { key, name, allow_sources } => {
    let key_asc = if key.starts_with("https://") || key.starts_with("http://") {
        reqwest::get(&key).await?.text().await
            .context("fetch key")?
    } else {
        std::fs::read_to_string(&key)
            .context("read key file")?
    };
    let body = serde_json::json!({
        "key_asc":         key_asc,
        "label":           name,
        "allowed_sources": allow_sources,
    });
    let result = client.cmd("proc/package/trust-add", body).await?;
    println!("Trusted key added: {} ({})", result["label"], result["fingerprint"]);
}
TrustCmd::List => {
    let result = client.cmd("proc/package/trust-list", json!({})).await?;
    let keys = result["keys"].as_array().cloned().unwrap_or_default();
    if keys.is_empty() {
        println!("No third-party keys trusted (official Avix key always active).");
        return Ok(());
    }
    for k in &keys {
        println!("{} — {} (added {})",
            k["fingerprint"].as_str().unwrap_or("?"),
            k["label"].as_str().unwrap_or("?"),
            k["added_at"].as_str().unwrap_or("?"),
        );
        let sources = k["allowed_sources"].as_array().cloned().unwrap_or_default();
        if sources.is_empty() {
            println!("  allowed sources: all");
        } else {
            for s in sources { println!("  allowed source: {}", s.as_str().unwrap_or("")); }
        }
    }
}
TrustCmd::Remove { fingerprint } => {
    client.cmd("proc/package/trust-remove", json!({ "fingerprint": fingerprint })).await?;
    println!("Removed key: {fingerprint}");
}
```

---

## Typical Third-Party Workflow

**Publisher side** (third-party developer):
```bash
# Sign their release archive
gpg --detach-sign --armor workspace-v1.0.0-linux-x86_64.tar.xz
# Produces workspace-v1.0.0-linux-x86_64.tar.xz.asc

# Distribute their public key (e.g. committed to their repo)
gpg --armor --export their@email.com > signing-key.asc
```

**Admin side** (Avix system admin, once per publisher):
```bash
avix client package trust add https://github.com/acmecorp/avix-plugins/raw/main/signing-key.asc \
  --name "AcmeCorp" \
  --allow-source "github:acmecorp/*"
```

**User side** (no extra steps — install works transparently):
```bash
avix client agent install github:acmecorp/avix-plugins/my-agent
# → kernel fetches archive + .asc, verifies against AcmeCorp key, installs
```

---

## Tests

### `packaging/trust.rs`
- `add_key_writes_asc_and_meta()` — after `add()`, both files exist in keyring dir
- `add_duplicate_fingerprint_errors()` — `add()` same key twice → `Err`
- `list_returns_all_keys_sorted_by_date()` — add 3 keys, list returns them in insertion order
- `remove_deletes_both_files()` — after `remove()`, neither `.asc` nor `.meta.yaml` exists
- `remove_nonexistent_errors()` — `Err`
- `get_returns_key_and_meta()` — `get()` after `add()` returns correct data
- `allows_source_empty_patterns_allows_all()` — `allowed_sources: []` → `allows_source("anything")` = true
- `allows_source_glob_prefix()` — `github:acmecorp/*` matches `github:acmecorp/my-agent`
- `allows_source_glob_no_match()` — `github:acmecorp/*` does not match `github:other/repo`
- `allows_source_exact_match()` — exact pattern matches exact string

### `packaging/gpg.rs`
- `verify_official_key_ok()` — data signed with embedded test official key → `VerifiedBy::Official`
- `verify_trusted_key_ok()` — data signed with third-party key present in store → `VerifiedBy::Trusted`
- `verify_trusted_key_wrong_source_errors()` — key present but source not in allowed_sources → `Err`
- `verify_unknown_key_errors()` — key not in store, not official → `Err` with hint message
- `verify_tampered_data_errors()` — valid signature, modified data → `Err`

### `syscall/domain/pkg_.rs`
- `trust_add_requires_admin()` — non-admin token → `Eperm`
- `trust_add_persists_key()` — admin adds key → `trust_list` returns it
- `trust_remove_requires_admin()` — non-admin → `Eperm`
- `trust_remove_deletes_key()` — add then remove → `trust_list` empty

---

## Success Criteria

- [ ] `avix client package trust add <key-url> --name "Vendor" --allow-source "github:vendor/*"` adds the key to the keyring
- [ ] `avix client package trust list` shows all trusted keys with fingerprint, label, allowed sources
- [ ] `avix client package trust remove <fingerprint>` removes the key
- [ ] Packages signed by a trusted key install without `install:from-untrusted-source` capability
- [ ] Trusted key with `--allow-source "github:vendor/*"` is rejected for packages from other sources
- [ ] Key with no `--allow-source` restriction is trusted for any source
- [ ] Official Avix packages continue to verify against the embedded key regardless of keyring state
- [ ] Unknown signing key → `Err` with message directing user to `avix package trust add`
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
