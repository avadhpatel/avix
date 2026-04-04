# pkg-gap-D — GPG Signing, Rollback & Polish

> **Status:** Pending
> **Priority:** Low (polish phase)
> **Depends on:** pkg-gap-A, pkg-gap-B, pkg-gap-C
> **Blocks:** nothing
> **Affects:**
> - `crates/avix-core/src/service/installer.rs` (rollback)
> - `crates/avix-core/src/agent_manifest/installer.rs` (rollback)
> - `crates/avix-core/src/syscall/domain/pkg_.rs` (GPG check, install quota)

---

## Problem

Installs are not cryptographically signed. A compromised download URL can deliver a
malicious package that passes SHA-256 verification if the checksum file is also compromised.
There is no rollback if an install partially succeeds. There are no quota limits to prevent
runaway installs.

---

## Scope

1. **GPG signature verification** — verify `.avix-signature.asc` against the official Avix
   public key for packages from `avadhpatel/avix`.
2. **Rollback on failure** — atomic install via temp dir; clean up on any error.
3. **Install quota** — per-user install rate limit (configurable, default 10/hour).
4. **Uninstall commands** — `avix agent uninstall <name>` and `avix service uninstall <name>`.

---

## What to Build

### 1. GPG verification — `crates/avix-core/src/service/gpg.rs`

```rust
use crate::error::AvixError;

/// Official Avix package signing public key (embedded at compile time).
const AVIX_PUBLIC_KEY: &str = include_str!("avix-signing-key.asc");

/// Verify `data` against a detached ASCII-armored signature `sig_asc`.
///
/// Uses the `pgp` crate (add to Cargo.toml: `pgp = "0.13"`).
pub fn verify_gpg_signature(data: &[u8], sig_asc: &str) -> Result<(), AvixError> {
    use pgp::{Deserializable, StandaloneSignature};

    let (pubkey, _) = pgp::SignedPublicKey::from_string(AVIX_PUBLIC_KEY)
        .map_err(|e| AvixError::ConfigParse(format!("parse public key: {e}")))?;

    let (sig, _) = StandaloneSignature::from_string(sig_asc)
        .map_err(|e| AvixError::ConfigParse(format!("parse signature: {e}")))?;

    sig.verify(&pubkey, data)
        .map_err(|e| AvixError::ConfigParse(format!("GPG verification failed: {e}")))?;

    Ok(())
}
```

Place `avix-signing-key.asc` (the official public key file) at
`crates/avix-core/src/service/avix-signing-key.asc`.

**Wire into installers:**
- In `ServiceInstaller::install` and `AgentInstaller::install`, after fetching bytes:
  - If source is official (`github:avadhpatel/avix`), fetch `<asset>.avix-signature.asc`
    (next to the release asset), and call `verify_gpg_signature(&bytes, &sig_asc)?`.
  - If signature file is absent for an official package: return `Err` (required for official).
  - For non-official packages: signature is optional; skip if absent.

### 2. Atomic install with rollback

Both `ServiceInstaller` and `AgentInstaller` currently copy files into the final `install_dir`
in one pass. Replace with an atomic two-phase pattern:

```
Phase 1: extract to temp dir (already done)
Phase 2: rename temp dir → final install_dir (atomic on same filesystem)
```

Use `std::fs::rename` (atomic on Linux/macOS when src and dst are on the same device).
If the rename fails (cross-device), fall back to `copy_dir_all` + `remove_dir_all(tmp)`.

On any error after phase 1 starts: call `cleanup_partial_install(&install_dir)`:

```rust
fn cleanup_partial_install(path: &Path) {
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
}
```

Wrap the install body in a guard:

```rust
struct InstallGuard { path: PathBuf, committed: bool }
impl Drop for InstallGuard {
    fn drop(&mut self) {
        if !self.committed {
            cleanup_partial_install(&self.path);
        }
    }
}
// …
let mut guard = InstallGuard { path: install_dir.clone(), committed: false };
// … all install steps …
guard.committed = true; // only set on full success
```

### 3. Install quota — `crates/avix-core/src/syscall/domain/pkg_.rs`

Add a simple in-memory rate limiter (per-user, counted against wall clock):

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct InstallQuota {
    window: Duration,
    limit: u32,
    counters: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}

impl InstallQuota {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self { window, limit, counters: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn check(&self, username: &str) -> Result<(), SyscallError> {
        let mut map = self.counters.lock().unwrap();
        let now = Instant::now();
        let entry = map.entry(username.to_owned()).or_insert((0, now));
        if now.duration_since(entry.1) > self.window {
            *entry = (0, now);
        }
        if entry.0 >= self.limit {
            return Err(SyscallError::Eperm(format!(
                "install quota exceeded: max {} installs per {:?}", self.limit, self.window
            )));
        }
        entry.0 += 1;
        Ok(())
    }
}
```

Default: 10 installs per hour per user. Make configurable via `auth.conf` field
`install_quota_per_hour: u32` (defaults to 10). Inject `InstallQuota` into the syscall
handler via the existing `SyscallContext` deps pattern.

### 4. Uninstall commands

#### 4a. Kernel syscalls

Add to `pkg_.rs`:

```rust
/// `proc/package/uninstall-agent`
///
/// Required capability: `install:agent` (same as install).
pub fn uninstall_agent(ctx: &SyscallContext, params: Value) -> SyscallResult {
    check_capability(ctx, "install:agent")?;
    let name = params["name"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing name".into()))?;
    let scope = parse_scope(&params, &ctx.username)?;
    let install_dir = match &scope {
        InstallScope::System => root.join("bin").join(name),
        InstallScope::User(u) => root.join("users").join(u).join("bin").join(name),
    };
    if !install_dir.exists() {
        return Err(SyscallError::Einval(format!("agent not installed: {name}")));
    }
    std::fs::remove_dir_all(&install_dir)
        .map_err(|e| SyscallError::Eio(e.to_string()))?;
    // Trigger ManifestScanner refresh.
    Ok(json!({ "uninstalled": name }))
}

/// `proc/package/uninstall-service`
pub async fn uninstall_service(ctx: &SyscallContext, params: Value) -> SyscallResult {
    check_capability(ctx, "install:service")?;
    let name = params["name"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing name".into()))?;
    // Stop running service first.
    service_manager.stop(name).await
        .map_err(|e| SyscallError::Eio(e.to_string()))?;
    let install_dir = root.join("services").join(name);
    if !install_dir.exists() {
        return Err(SyscallError::Einval(format!("service not installed: {name}")));
    }
    std::fs::remove_dir_all(&install_dir)
        .map_err(|e| SyscallError::Eio(e.to_string()))?;
    Ok(json!({ "uninstalled": name }))
}
```

Register both in `registry.rs` and `handler.rs`.

#### 4b. CLI commands

```
avix agent uninstall <name> [--scope user|system]
avix service uninstall <name>
```

Mirror the install command pattern: build ATP body, call `cmd()`, print result.

---

## Tests

### `gpg.rs`
- `verify_valid_signature_ok()` — sign test bytes with a test key, verify passes
- `verify_wrong_signature_errors()` — tampered bytes → `Err`
- `verify_wrong_key_errors()` — different key → `Err`

### `installer.rs` (rollback)
- `partial_install_cleaned_up_on_error()` — inject a failure mid-install, assert `install_dir` does not exist after
- `committed_install_not_cleaned_up()` — successful install, dir remains

### `pkg_.rs` (quota)
- `quota_allows_under_limit()` — 9 installs within window → all pass
- `quota_blocks_over_limit()` — 11th install → `Eperm`
- `quota_resets_after_window()` — advance time past window, counter resets

### `pkg_.rs` (uninstall)
- `uninstall_agent_removes_dir()` — install then uninstall, dir gone
- `uninstall_nonexistent_agent_errors()` — `Einval`
- `uninstall_service_stops_before_removing()` — mock `ServiceManager::stop` called before rm

---

## Success Criteria

- [ ] GPG signature verified for official Avix packages; failure blocks install
- [ ] Non-official packages work without a signature
- [ ] Partial installs are fully cleaned up on any error
- [ ] Install quota blocks more than N installs per hour per user
- [ ] `avix agent uninstall <name>` removes the agent and refreshes ManifestScanner
- [ ] `avix service uninstall <name>` stops the service then removes files
- [ ] Uninstall button in Web-UI Extensions tab calls the uninstall syscall
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
