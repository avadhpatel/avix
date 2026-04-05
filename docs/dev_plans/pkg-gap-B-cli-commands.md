# pkg-gap-B — CLI Commands & GitHub Integration

> **Status:** Done (incorporated into docs/architecture/15-packaging.md)
> **Priority:** High
> **Depends on:** pkg-gap-A (install syscalls)
> **Blocks:** pkg-gap-C (TUI/Web-UI need CLI plumbing done first)
> **Affects:**
> - `crates/avix-cli/src/` (`agent` and `service` subcommands)
> - `crates/avix-core/src/agent_manifest/installer.rs` (git clone fallback)

---

## Problem

There are no `avix agent install` or `avix service install` commands. Users cannot install
packages from the command line. GitHub API source resolution and `git clone` fallback are
unimplemented in the CLI layer.

---

## Scope

1. `avix agent install <source> [flags]` — sends `proc/package/install-agent` via ATP.
2. `avix service install <source> [flags]` — sends `proc/package/install-service` via ATP.
3. Both commands stream live kernel progress from the ATP `event` channel.
4. Git clone fallback for `git:` sources.
5. `--dry-run` flag (print resolved URL + checksum without installing).

No TUI screens (gap C). No GPG (gap D).

---

## What to Build

### 1. Shared install flags struct

Add to a shared location (e.g. `crates/avix-cli/src/install_flags.rs`):

```rust
use clap::Args;

#[derive(Args, Debug, Clone)]
pub struct InstallFlags {
    /// Package source: `github:owner/repo/name`, `https://…`, `git:https://…`, or local path.
    pub source: String,

    /// Install scope: `user` (default) or `system`.
    #[arg(long, default_value = "user")]
    pub scope: String,

    /// Specific version or tag (default: latest).
    #[arg(long)]
    pub version: Option<String>,

    /// Expected SHA-256 checksum in `sha256:<hex>` format.
    #[arg(long)]
    pub checksum: Option<String>,

    /// Skip checksum verification (trusted local dev only).
    #[arg(long)]
    pub no_verify: bool,

    /// Log this install under a specific session ID.
    #[arg(long)]
    pub session: Option<String>,

    /// Print what would happen without actually installing.
    #[arg(long)]
    pub dry_run: bool,
}
```

### 2. `avix agent install` command

Wire into the existing `avix agent` subcommand group
(file: `crates/avix-cli/src/commands/agent.rs` or wherever agent commands live).

```rust
pub async fn cmd_agent_install(flags: InstallFlags, client: &AtpClient) -> anyhow::Result<()> {
    if flags.dry_run {
        let resolved = PackageSource::resolve(&flags.source, flags.version.as_deref()).await?;
        println!("Resolved source: {:?}", resolved);
        return Ok(());
    }

    let body = serde_json::json!({
        "source":     flags.source,
        "scope":      flags.scope,
        "version":    flags.version.as_deref().unwrap_or("latest"),
        "checksum":   flags.checksum,
        "no_verify":  flags.no_verify,
        "session_id": flags.session,
    });

    let reply = client
        .cmd("proc/package/install-agent", body)
        .await
        .context("install-agent failed")?;

    println!(
        "Installed agent '{}' v{}",
        reply["name"].as_str().unwrap_or("?"),
        reply["version"].as_str().unwrap_or("?")
    );
    Ok(())
}
```

### 3. `avix service install` command

Mirror of the above but targets `proc/package/install-service`.
Scope defaults to `system` for services (admins typically install system-wide).

```rust
pub async fn cmd_service_install(flags: InstallFlags, client: &AtpClient) -> anyhow::Result<()> {
    if flags.dry_run {
        let resolved = PackageSource::resolve(&flags.source, flags.version.as_deref()).await?;
        println!("Resolved source: {:?}", resolved);
        return Ok(());
    }

    let scope = if flags.scope == "user" { "system" } else { &flags.scope };
    let body = serde_json::json!({
        "source":     flags.source,
        "scope":      scope,
        "version":    flags.version.as_deref().unwrap_or("latest"),
        "checksum":   flags.checksum,
        "no_verify":  flags.no_verify,
        "session_id": flags.session,
    });

    let reply = client
        .cmd("proc/package/install-service", body)
        .await
        .context("install-service failed")?;

    println!(
        "Installed service '{}' v{} → registered with router",
        reply["name"].as_str().unwrap_or("?"),
        reply["version"].as_str().unwrap_or("?")
    );
    Ok(())
}
```

### 4. Live progress streaming

Both commands should subscribe to ATP events while the install is in progress and print
kernel progress notifications to stdout. Use the existing `EventEmitter` / `AtpClient`
event subscription pattern.

```rust
// After sending cmd, subscribe to events filtered by the install op_id.
let mut events = client.subscribe_events().await?;
while let Some(event) = events.next().await {
    match event.kind.as_str() {
        "install.progress" => println!("  {}", event.body["message"].as_str().unwrap_or("")),
        "install.complete" => { println!("Done."); break; }
        "install.error"    => {
            eprintln!("Error: {}", event.body["message"].as_str().unwrap_or("unknown"));
            anyhow::bail!("install failed");
        }
        _ => {}
    }
}
```

The kernel emits these events from within the `install_agent` / `install_service` syscall
handlers. Add `tracing::info!` calls at download, verify, extract, and register stages;
map them to ATP `notification` events via the existing notification broadcast path.

### 5. Git clone fallback — `crates/avix-core/src/agent_manifest/git_fetch.rs`

For `PackageSource::GitClone(url)`:

```rust
use crate::error::AvixError;
use std::path::Path;

/// Clone a git repo into `dest` using the system `git` binary.
///
/// Uses `tokio::process::Command` to avoid blocking the async runtime.
pub async fn git_clone_to(url: &str, dest: &Path) -> Result<(), AvixError> {
    let status = tokio::process::Command::new("git")
        .args(["clone", "--depth=1", url, dest.to_str().unwrap_or("")])
        .status()
        .await
        .map_err(|e| AvixError::ConfigParse(format!("git clone failed: {e}")))?;

    if !status.success() {
        return Err(AvixError::ConfigParse(format!(
            "git clone exited with code: {:?}", status.code()
        )));
    }
    Ok(())
}
```

In `AgentInstaller::install`, detect `PackageSource::GitClone` and call `git_clone_to`
into a tempdir, then proceed to read `manifest.yaml` from that dir instead of extracting
a tarball.

### 6. Clap wiring

Update the CLI's top-level command enum to add:

```
avix agent install <source> [--scope user|system] [--version v0.1.0] [--checksum sha256:…]
                             [--no-verify] [--session <id>] [--dry-run]
avix service install <source> [--scope system|user] [--version v0.1.0] [--checksum sha256:…]
                               [--no-verify] [--session <id>] [--dry-run]
```

Keep `avix service install` consistent with the existing `avix service` subcommand group
(alongside `list`, `status`, `start`, `stop`, etc.). `avix agent` gets a new `install`
subcommand alongside `catalog`, `history`, `show`.

---

## Tests

### `install_flags.rs`
- `parse_source_only()` — `avix agent install https://…` parses correctly
- `parse_all_flags()` — all flags parse from args
- `scope_defaults_user()` — default scope for `agent install` is `user`

### `commands/agent.rs`
- `dry_run_prints_resolved_source()` — mock `PackageSource::resolve`, assert output
- `install_sends_correct_atp_body()` — mock `AtpClient`, assert `source`, `scope`, `version` in body
- `install_error_propagates()` — ATP returns error → `anyhow::bail!`

### `git_fetch.rs`
- `git_clone_nonexistent_repo_errors()` — bad URL → `Err` (requires git binary)
- `git_clone_success_creates_files()` — integration test, clone a small public repo (guard with `#[ignore]`)

---

## Success Criteria

- [ ] `avix agent install github:avadhpatel/avix/universal-tool-explorer` resolves, installs, and prints success
- [ ] `avix service install github:avadhpatel/avix/workspace` installs and registers with the router
- [ ] `--dry-run` prints resolved URL without modifying disk
- [ ] `--no-verify` skips checksum check
- [ ] `--session <id>` is forwarded in the ATP body
- [ ] `git:https://…` source triggers git clone fallback
- [ ] Live progress events are printed to stdout during install
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
