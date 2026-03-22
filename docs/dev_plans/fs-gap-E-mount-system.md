# Filesystem Gap E — Mount System / fstab (Out of 31-Day Scope — Future Work)

> **Finding:** The filesystem spec (§7–10) defines a comprehensive mount system with 8 storage
> providers (`local`, `s3`, `gcs`, `azure-blob`, `nfs`, `sftp`, `encrypted-local`, `git`,
> `memory`), per-path provider delegation, mount lifecycle, and an `avix mount` CLI. None of
> this is implemented. There is no `FstabConfig` type, no provider trait, no mount registry.
>
> **Decision:** The mount system is **out of scope for the 31-day v0.1 plan**. `MemFs` with
> the `memory` provider covers 100% of the dev, test, and initial production surface. The mount
> system is a v0.2+ hardening concern.
>
> This file documents what needs to be built when the mount system is scheduled, so the design
> is not lost.

---

## What "not implemented" means in practice

- All VFS access during Days 1–31 goes through `MemFs` (in-memory, non-persistent).
- `fstab.yaml` is written by `config init` (Finding C) but **not read or parsed** by bootstrap.
- `MemFs` is the sole backing store. When the process exits, all VFS state is lost.
- Persistent user content (agent workspaces, snapshots) is deferred — agents write to MemFS
  during a session but nothing is flushed to disk on exit.
- This is acceptable for v0.1 because the full agent loop, capability system, and tool
  infrastructure are the primary deliverables.

---

## Scope for v0.2 — Mount System

### Phase 1: `FstabConfig` type + bootstrap parsing

**What to build:**
- `FstabConfig` struct in `src/config/fstab.rs` with `mounts: Vec<MountEntry>`.
- `MountEntry`: `path`, `provider`, `config` (provider-specific), `options`.
- Bootstrap Phase 1 reads `<root>/etc/fstab.yaml` and initialises mounts.
- Failed mount → affected paths return `EUNAVAIL`; agents requiring them held in `pending`.

**Tests:**
```rust
fn fstab_parses_local_mount() { ... }
fn fstab_parses_s3_mount() { ... }
fn bootstrap_registers_mounts_from_fstab() { ... }
fn bootstrap_holds_agent_when_required_mount_unavailable() { ... }
```

### Phase 2: `StorageProvider` trait + `LocalProvider`

**What to build:**

```rust
#[async_trait]
pub trait StorageProvider: Send + Sync {
    async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, AvixError>;
    async fn write(&self, path: &VfsPath, content: Vec<u8>) -> Result<(), AvixError>;
    async fn delete(&self, path: &VfsPath) -> Result<(), AvixError>;
    async fn exists(&self, path: &VfsPath) -> bool;
    async fn list(&self, dir: &VfsPath) -> Result<Vec<String>, AvixError>;
}

pub struct LocalProvider { root: PathBuf }
pub struct MemoryProvider { inner: MemFs }  // wraps existing MemFs
```

`MemFs` is refactored to implement `StorageProvider`. The mount registry holds a
`HashMap<String, Arc<dyn StorageProvider>>` keyed by mount path prefix, resolved
longest-prefix-first on each VFS call.

### Phase 3: `avix mount` CLI commands

```sh
avix mount add /users/alice --provider s3 --config bucket=avix-prod,region=us-east-1
avix mount remove /users/alice
avix mount status
```

### Phase 4: Cloud providers

Implement `S3Provider`, `GcsProvider` behind feature flags:
```toml
[features]
provider-s3 = ["aws-sdk-s3"]
provider-gcs = ["google-cloud-storage"]
```

### Phase 5: Encrypted local provider

`EncryptedLocalProvider` wraps `LocalProvider`, encrypting blobs at rest with AES-256-GCM
using the `AVIX_MASTER_KEY`. This is the production alternative to the separate `SecretsStore`.

---

## Dependencies before mount system can be scheduled

1. **Finding A** (Bootstrap Phase 1) must be complete — Phase 1 already calls `MemFs` directly;
   once the mount system exists, Phase 1 switches to initialising the `MountRegistry` instead.
2. **Finding C** (config init fstab.yaml) must be complete — the mount system reads from it.
3. Day-21 `kernel/fs/*` syscalls must be complete — they are the agent entry point to the VFS
   and will route through the `MountRegistry` rather than `MemFs` directly.

---

## Tracking

- **v0.1 deliverable:** `fstab.yaml` written by `config init` (skeleton only, not parsed at boot).
- **v0.2 deliverable:** `FstabConfig` parsed at bootstrap; `LocalProvider` + `MemoryProvider` implemented; mount registry routing in `kernel/fs/*` syscalls.
- **v0.3+ deliverable:** S3/GCS/Azure providers behind feature flags.

No code changes are required in Days 1–31 to unblock anything else. This file is reference
documentation only until v0.2 is scheduled.
