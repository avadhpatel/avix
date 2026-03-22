# Day 12 — Bootstrap Phases 0–2

> **Goal:** Implement the 4-phase bootstrap sequence: Phase 0 (panic ring buffer), Phase 1 (VFS mount), Phase 2 (config load + master key), Phase 3 (service start). Enforce the auth.conf-first invariant. Target: <700ms cold boot.

---

## Pre-flight: Verify Day 11

```bash
cargo test --workspace
grep -r "fn bootstrap_with_root" crates/avix-core/src/
grep -r "run_config_init"        crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Extend `src/bootstrap/`:

```
src/bootstrap/
├── mod.rs         ← Runtime struct, bootstrap phases
├── phase0.rs      ← panic ring buffer
├── phase1.rs      ← VFS mount
├── phase2.rs      ← config load, master key
└── phase3.rs      ← service startup order
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/bootstrap.rs`:

```rust
use avix_core::bootstrap::Runtime;
use tempfile::tempdir;
use std::time::Instant;

fn write_minimal_auth_conf(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("etc")).unwrap();
    std::fs::write(root.join("etc/auth.conf"), r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 8h
identities:
  - name: alice
    uid: 1001
    role: admin
    credential:
      type: api_key
      key_hash: "hmac-sha256:test"
"#).unwrap();
}

// ── Auth.conf requirement ─────────────────────────────────────────────────────

#[tokio::test]
async fn bootstrap_aborts_without_auth_conf() {
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("auth.conf"));
}

#[tokio::test]
async fn bootstrap_succeeds_with_valid_auth_conf() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    // Set required env var for master key
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    std::env::remove_var("AVIX_MASTER_KEY");
    assert!(result.is_ok());
}

// ── Master key from env ───────────────────────────────────────────────────────

#[tokio::test]
async fn phase2_loads_master_key_from_env() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");

    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    assert!(runtime.has_master_key());

    // Env var should be cleared after load
    assert!(std::env::var("AVIX_MASTER_KEY").is_err());
}

#[tokio::test]
async fn phase2_fails_without_master_key_env_var() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::remove_var("AVIX_MASTER_KEY");

    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("AVIX_MASTER_KEY"));
}

// ── Phase sequencing ──────────────────────────────────────────────────────────

#[tokio::test]
async fn bootstrap_phases_complete_in_order() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");

    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let log = runtime.boot_log();
    let phases: Vec<_> = log.iter().map(|e| e.phase).collect();
    assert!(phases.windows(2).all(|w| w[0] < w[1])); // monotonically increasing
}

// ── Boot time ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bootstrap_completes_within_700ms() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");

    let start = Instant::now();
    Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 700,
        "bootstrap took {}ms, expected <700ms", elapsed.as_millis()
    );
}

// ── Installed services start after built-ins ──────────────────────────────────

#[tokio::test]
async fn built_in_services_get_low_pids() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");

    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    // Built-in services (router, auth, memfs, logger) should have low PIDs
    let router_pid = runtime.service_pid("router").await.unwrap();
    assert!(router_pid.as_u32() <= 9, "router should have PID ≤ 9, got {}", router_pid);
}
```

---

## Step 3 — Implement

`Runtime` struct with `process_table: ProcessTable`, `signal_bus: SignalBus`, `memfs: MemFs`, `master_key: Option<[u8; 32]>`, `boot_log: Vec<BootLogEntry>`.

Bootstrap phases:
- **Phase 0**: initialise panic ring buffer
- **Phase 1**: mount MemFS, create `/proc/`, `/kernel/` trees  
- **Phase 2**: read `auth.conf` (abort if missing), load `AVIX_MASTER_KEY` from env and zero it
- **Phase 3**: start built-in services in dependency order (logger → memfs → auth → router) assigning PIDs 1–9

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 20+ bootstrap tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-12: bootstrap phases 0-3, auth.conf-first, master key from env, <700ms"
```

## Success Criteria

- [ ] Bootstrap aborts without `auth.conf` (checked first)
- [ ] `AVIX_MASTER_KEY` loaded and env var zeroed immediately
- [ ] Bootstrap fails if `AVIX_MASTER_KEY` not set
- [ ] Phase ordering is monotonically increasing in boot log
- [ ] Cold boot <700ms (in test environment)
- [ ] Built-in services get PIDs ≤ 9
- [ ] 20+ tests pass, 0 clippy warnings

---
---

