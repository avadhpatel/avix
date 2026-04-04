# Packaging and Installation System

> **Spec status:** Implemented — reflects pkg-gaps A–F
> **Last updated:** April 2026

---

## Overview

Avix packages are `.tar.xz` archives containing agent packs or services. The packaging
system provides:

1. **Package authoring** — `PackageScaffold`, `PackageBuilder`, `PackageValidator`
2. **Package installation** — kernel syscalls for agent/service install, uninstall
3. **CLI/TUI/Web-UI** — user-facing commands for managing packages
4. **Trust & verification** — embedded official key + `TrustStore` for third-party keys

---

## Package Types

### Agent Packs

```
agents/<agent-name>/
├── manifest.yaml          # REQUIRED: agent metadata
├── system-prompt.md      # REQUIRED: default system prompt
├── examples/             # optional: example prompts
└── README.md            # optional
```

**manifest.yaml format:**

```yaml
name: universal-tool-explorer
version: 0.1.0
description: Explores available tools and their capabilities
system_prompt_path: system-prompt.md  # or embed directly
```

### Services

```
services/<service-name>/
├── Cargo.toml            # REQUIRED for Rust services
├── src/                 # source code
├── service.yaml         # REQUIRED: service configuration
├── tools/               # optional: *.tool.yaml descriptors
├── bin/                 # compiled binaries (CI-built)
└── README.md            # optional
```

**service.yaml format** — see `07-services.md`.

---

## Package Tooling (Offline)

All package tooling works without a running Avix server.

### CLI Commands

| Command | Description |
|---------|-------------|
| `avix client package validate <path>` | Validate package structure |
| `avix client package build <path> --version v0.1.0 [-o <dir>]` | Build `.tar.xz` + checksums |
| `avix client package new <name> --type agent\|service [-o <dir>]` | Scaffold new package |
| `avix client package trust add <key-url> --name <label> [--allow-source <pattern>]` | Add trusted key |
| `avix client package trust list` | List trusted keys |
| `avix client package trust remove <fingerprint>` | Remove trusted key |

### Trust Management

Avix uses GPG signature verification for package integrity:

1. **Official packages** — verified against embedded official Avix public key
2. **Third-party packages** — verified against keys in `TrustStore`

**TrustStore location:** `AVIX_ROOT/etc/avix/trusted-keys/`

```
etc/avix/trusted-keys/
├── <fingerprint>.asc           # ASCII-armored public key
└── <fingerprint>.meta.yaml     # label, added_at, allowed_sources
```

**Allowed sources:** Optional glob patterns that restrict which package sources a key can sign for. If empty, the key is trusted for all sources.

### Library API

```rust
use avix_core::packaging::{
    PackageType, PackageValidator, PackageBuilder, PackageScaffold,
    BuildRequest, ScaffoldRequest,
};

// Detect type
let pkg_type = PackageType::detect(&path)?;

// Validate
match PackageValidator::validate(&path) {
    Ok(_) => println!("valid"),
    Err(errors) => for e in errors { ... }
}

// Build archive
let req = BuildRequest {
    source_dir: path,
    output_dir: std::path::PathBuf::from("./dist"),
    version: "v0.1.0".into(),
    skip_validation: false,
};
let result = PackageBuilder::build(req)?;

// Scaffold new package
let req = ScaffoldRequest {
    name: "my-agent".into(),
    pkg_type: PackageType::Agent,
    version: "0.1.0".into(),
    output_dir: std::path::PathBuf::from("./agents"),
};
let path = PackageScaffold::create(req)?;

// TrustStore API
use avix_core::packaging::{TrustStore, TrustedKey};
let store = TrustStore::new(root_path);
let key = store.add(key_asc, "AcmeCorp", vec!["github:acmecorp/*".to_string()])?;
let keys = store.list()?;
store.remove(&fingerprint)?;
```

### Checksums File

Build creates `checksums.sha256` in the output directory:

```
abc123...  universal-tool-explorer-v0.1.0.tar.xz
def456...  workspace-v1.0.0-linux-x86_64.tar.xz
```

Format is compatible with `sha256sum -c`.

---

## Package Installation (Online)

### Syscalls

| Syscall | Capability | Description |
|---------|------------|--------------|
| `proc/package/install-agent` | `install:agent` | Install agent from URL/path |
| `proc/package/install-service` | `install:service` | Install service from URL/path |
| `proc/package/uninstall-agent` | `install:agent` | Remove installed agent |
| `proc/package/uninstall-service` | `install:service` | Stop and remove service |

### PackageSource Resolution

The kernel resolves package sources via `PackageSource`:

| Prefix | Example | Resolution |
|--------|---------|------------|
| `file://` | `file:///home/user/agents/my-agent` | Local path |
| `https://` | `https://github.com/user/repo/releases/v0.1.0.tar.xz` | Direct download |
| `github:` | `github:owner/repo/name` | GitHub Releases API |
| `git:` | `git:https://github.com/user/repo` | Git clone |
| (no prefix) | `/home/user/agents/my-agent` | Local path (file:// assumed) |

### Installation Flow

```
avix agent install github:acme/avix-plugins/my-agent
    │
    ├─ CLI → ATP connect → dispatcher.cmd("proc/package/install-agent", {...})
    │
    ├─ Kernel receives syscall
    │   ├─ Check capability: install:agent
    │   ├─ Resolve PackageSource (fetch to temp)
    │   ├─ Verify checksum (if provided)
    │   ├─ Extract .tar.xz to install dir
    │   ├─ InstallGuard ensures atomic rollback on failure
    │   └─ Write install receipt
    │
    ├─ ManifestScanner refresh (system + user bins)
    │
    └─ Return: { installed: "my-agent", version: "0.1.0" }
```

### Capabilities

| Capability | Description |
|------------|--------------|
| `install:agent` | Install/uninstall agent packages |
| `install:service` | Install/uninstall service packages |
| `auth:admin` | Add/remove trusted keys via `proc/package/trust-*` |
| `install:from-untrusted-source` | Required for unsigned packages or packages from sources not covered by trusted keys |

---

## Uninstall Flow

```
avix agent uninstall my-agent [--scope user|system]
    │
    ├─ CLI → ATP → proc/package/uninstall-agent
    │
    ├─ Kernel:
    │   ├─ Check install:agent capability
    │   ├─ Determine scope (user vs system)
    │   ├─ Remove /users/<u>/bin/<agent> or /bin/<agent>
    │   └─ Refresh ManifestScanner
    │
    └─ Return: { uninstalled: "my-agent" }
```

---

## Install Quota

Per-user rate limiting prevents runaway installs:

- **Default:** 10 installs per hour per user
- **Enforced via:** `InstallQuota` in `pkg_.rs`
- **Configurable:** via `auth.conf` field `install_quota_per_hour`

---

## GitHub Actions Packaging

The release workflow (`.github/workflows/release-packages.yml`) builds packages:

```yaml
# Triggered on GitHub Release
- uses: actions/checkout@v4
- run: |
    tar -cvJf universal-tool-explorer-${{ github.ref_name }}.tar.xz \
      agents/packs/universal-tool-explorer/
    echo "${{ hashFile(...) }}  universal-tool-explorer-${{ github.ref_name }}.tar.xz" >> checksums.sha256
- uses: softprops/action-gh-release@v1
  with:
    files: |
      *.tar.xz
      checksums.sha256
```

---

## Related Files

| File | Description |
|------|-------------|
| `crates/avix-core/src/packaging/` | Offline tooling module (trust, gpg, builder, validator, scaffold) |
| `crates/avix-core/official-pubkey.asc` | Embedded official Avix public key |
| `crates/avix-core/src/agent_manifest/` | Agent manifest + installer |
| `crates/avix-core/src/service/installer.rs` | Service installer |
| `crates/avix-core/src/service/package_source.rs` | Package source resolver |
| `crates/avix-core/src/syscall/domain/pkg_.rs` | Install/uninstall syscalls |
| `crates/avix-core/src/syscall/registry.rs` | Syscall registration |
| `crates/avix-cli/src/main.rs` | CLI package commands |
| `crates/avix-cli/src/tui/` | TUI install commands |
| `crates/avix-app/src-tauri/src/commands.rs` | Tauri install commands |
| `crates/avix-app/src-web/src/routes.rs` | Web-UI install handlers |

---

## Future Work

- Multi-language service scaffolding (Node.js, Python)
- Package repository/index server
