# 15 — Packaging System

> **Audience:** Developers integrating the packaging system, security auditors, third-party package publishers  
> **Assumes:** Familiar with Avix kernel, IPC, and capability system

---

## Overview

The Avix packaging system handles distribution and installation of two package types: **Agents** (LLM-driven conversational processes) and **Services** (deterministic background processes). Both use a tar.xz archive format with embedded metadata.

Key design decisions:
- Package type is detected by file presence: `manifest.yaml` → Agent, `service.yaml` → Service
- Signature verification uses GPG with a two-stage trust model (official embedded key + admin-managed third-party keyring)
- Install operations are async; uninstall is sync
- `--no-verify` flag bypasses signature verification for air-gapped or development scenarios

---

## Package Types

### Agent Packages

An agent is an LLM-driven process with a system prompt, metadata, and optional example conversations.

**Directory structure (unpacked):**
```
universal-tool-explorer-v0.1/
├── manifest.yaml              # Required: agent metadata
├── system-prompt.md           # Required: LLM system prompt
├── README.md                  # Optional: documentation
└── examples/                  # Optional: example conversation files
    └── example-goal.md
```

**manifest.yaml schema:**
```yaml
name: universal-tool-explorer
version: 0.1.0
description: Explore tool capabilities across all available tools
system_prompt_path: system-prompt.md
```

Required fields: `name`, `version`, `description`, `system_prompt_path`. The referenced prompt file must exist in the package root.

### Service Packages

A service is a deterministic process exposing tools via the Avix tool registry.

**Directory structure (unpacked):**
```
my-service-v1.0.0/
├── service.yaml               # Required: service definition
└── bin/                       # Required: contains executable
    └── my-service
```

**service.yaml schema:** See `docs/architecture/07-services.md` — uses the same `ServiceUnit` format as disk-installed services.

---

## Package Format

Packages are distributed as tar.xz archives. The installer extracts to a versioned directory:

- **Agents:** `AVIX_ROOT/bin/<agent-name>-<version>/` (system) or `AVIX_ROOT/users/<username>/bin/<agent-name>-<version>/` (user)
- **Services:** `AVIX_ROOT/services/<service-name>-<version>/`

The versioned directory prevents conflicts when installing multiple versions. The `avix agent catalog` command shows all installed versions.

---

## Package Detection

Detection happens at the filesystem level before parsing:

```rust
// crates/avix-core/src/packaging/mod.rs
impl PackageType {
    pub fn detect(dir: &Path) -> Result<Self, AvixError> {
        if dir.join("manifest.yaml").exists() {
            return Ok(Self::Agent);
        }
        if dir.join("service.yaml").exists() {
            return Ok(Self::Service);
        }
        Err(AvixError::ConfigParse(
            "cannot detect package type: no manifest.yaml or service.yaml found".into(),
        ))
    }
}
```

**Detection order:** Agent (manifest.yaml) is checked first, then Service (service.yaml). This means a package with both files is treated as an Agent.

---

## Signature Verification

### GPG Verification Flow

All signed packages go through verification before installation:

1. **Fetch** the archive (`.tar.xz`) and signature (`.tar.xz.asc`)
2. **Parse** the detached GPG signature
3. **Verify** against the official embedded key first
4. **Fall back** to the third-party trust keyring if official verification fails
5. **Check** source restrictions on third-party keys

### Two-Stage Trust Model

**Stage 1 — Official Key:**  
Avix ships a single embedded public key in `crates/avix-core/official-pubkey.asc`. Packages signed with this key are always trusted, regardless of source.

**Stage 2 — Third-Party Keyring:**  
Administrators can add keys for trusted publishers. Each key has optional `allowed_sources` glob patterns.

### Keyring Directory Layout

```
AVIX_ROOT/
└── etc/avix/trusted-keys/
    ├── DEADBEEF1234CAFE.asc           # ASCII-armored public key
    └── DEADBEEF1234CAFE.meta.yaml     # label, added_at, allowed_sources
```

**meta.yaml example:**
```yaml
fingerprint: DEADBEEF1234CAFE
label: "AcmeCorp"
added_at: "2026-04-04T10:00:00Z"
allowed_sources:
  - "github:acmecorp/*"
  - "https://packages.acmecorp.com/*"
```

An empty `allowed_sources` list means the key is trusted for packages from **any** source.

### Source Pattern Matching

Patterns support:
- `*` as a trailing wildcard: `github:acmecorp/*` matches `github:acmecorp/my-agent`
- Exact string matching: `https://packages.acmecorp.com/foo` matches exactly

### VerifiedBy Enum

Verification returns one of:
```rust
pub enum VerifiedBy {
    Official,           // Signed with embedded Avix key
    Trusted(TrustedKey), // Signed with a third-party key
}
```

Callers log which key verified the package for audit purposes.

---

## Trust Management

### CLI Commands

```bash
# Add a trusted third-party key
avix package trust add https://github.com/acmecorp/keys/signing-key.asc \
  --name "AcmeCorp" \
  --allow-source "github:acmecorp/*"

# List all trusted keys
avix package trust list

# Remove a trusted key
avix package trust remove DEADBEEF1234CAFE
```

### Kernel Syscalls

| Syscall | Capability | Description |
|---------|-----------|-------------|
| `proc/package/trust-add` | `auth:admin` | Add a key to the keyring |
| `proc/package/trust-list` | none | List all trusted keys |
| `proc/package/trust-remove` | `auth:admin` | Remove a key by fingerprint |

All three are synchronous (no async I/O needed).

---

## Install Operations

### CLI: `avix package install-agent`

```bash
avix package install ./universal-tool-explorer-v0.1.tar.xz
avix package install github:avadhpatel/avix/universal-tool-explorer
avix package install ./universal-tool-explorer-v0.1.tar.xz --no-verify
```

**Parameters:**
- `source`: Local path (file://) or remote (github:, https://)
- `scope`: `user` (default) or `system`
- `version`: Specific version (default: latest)
- `checksum`: SHA256 checksum for additional verification
- `no_verify`: Skip GPG signature verification (requires `install:from-untrusted-source` for non-official sources)

### Async Functions

Install operations are async because they may:
- Fetch packages from the network (reqwest)
- Run GPG verification (CPU-bound, run in spawn_blocking)
- Extract archives and write files to disk

The IPC server awaits these directly, while the sync syscall handler uses `block_on` inside a spawned blocking task to avoid runtime conflicts.

---

## Uninstall Operations

### CLI: `avix package uninstall-agent`

```bash
avix package uninstall universal-tool-explorer
```

Uninstall is synchronous — it only removes directories via `std::fs::remove_dir_all`.

---

## Install Quota

To prevent runaway installation, a quota limits installs per time window:

- **Limit:** 10 installs per hour per user
- **Implementation:** In-memory `Arc<Mutex<HashMap>>` in `pkg_.rs`

```rust
lazy_static::lazy_static! {
    static ref INSTALL_QUOTA: InstallQuota = InstallQuota::new(10, Duration::from_secs(3600));
}
```

The quota is checked before any install operation begins.

---

## Validation

`PackageValidator` validates packages before installation:

**Agent validation:**
- `manifest.yaml` exists and parses
- `name` is non-empty
- `version` is non-empty
- `system_prompt_path` file exists

**Service validation:**
- `service.yaml` exists and parses as `ServiceUnit`
- `bin/` directory exists and is non-empty

Validation errors are collected and returned as a `Vec<ValidationError>`.

---

## Kernel Syscalls Reference

| Syscall | Capability | Async | Description |
|---------|-----------|-------|-------------|
| `proc/package/install-agent` | `proc/package/install-agent` | Yes | Install an agent package |
| `proc/package/install-service` | `proc/package/install-service` | Yes | Install a service package |
| `proc/package/uninstall-agent` | `proc/package/install-agent` | No | Uninstall an agent |
| `proc/package/uninstall-service` | `proc/package/install-service` | No | Uninstall a service |
| `proc/package/trust-add` | `auth:admin` | No | Add a trusted key |
| `proc/package/trust-list` | none | No | List trusted keys |
| `proc/package/trust-remove` | `auth:admin` | No | Remove a trusted key |

---

## Error Handling

| Error | Cause | Resolution |
|-------|-------|------------|
| `cannot detect package type` | No manifest.yaml or service.yaml | Add the required file to the package |
| `signing key is not trusted` | Unknown signing key | Run `avix package trust add` to add the publisher's key |
| `key not trusted for source` | Third-party key's allowed_sources doesn't match | Add matching source pattern or use key without restrictions |
| `install quota exceeded` | More than 10 installs per hour | Wait until the hour resets |
| `install:from-untrusted-source required` | Non-official source without `--no-verify` | Add the capability to the user's token, or use `--no-verify` |

---

## File Reference

| File | Purpose |
|------|---------|
| `crates/avix-core/src/packaging/mod.rs` | PackageType enum, exports |
| `crates/avix-core/src/packaging/validator.rs` | PackageValidator, validation logic |
| `crates/avix-core/src/packaging/gpg.rs` | GPG verification, VerifiedBy |
| `crates/avix-core/src/packaging/trust.rs` | TrustStore, TrustedKey |
| `crates/avix-core/src/packaging/builder.rs` | PackageBuilder for creating packages |
| `crates/avix-core/src/packaging/scaffold.rs` | PackageScaffold for template generation |
| `crates/avix-core/src/syscall/domain/pkg_.rs` | Install/uninstall syscall handlers |
| `crates/avix-core/official-pubkey.asc` | Embedded Avix public key |