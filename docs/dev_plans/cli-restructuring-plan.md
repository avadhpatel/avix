# CLI Command Restructuring Plan

## Overview

Restructure the `avix` CLI to move `service`, `secret`, and `session` subcommands under the `client` command, and add `--config` flag to each client subcommand for specifying custom client config path.

## Current Structure

```
avix server <subcommands>           # Server-side operations
avix client <subcommands>            # Client-side operations (limited)
avix session <subcommands>          # Standalone (should be under client)
avix service <subcommands>          # Standalone (should be under client)
avix secret <subcommands>           # Standalone (should be under client)
```

## Proposed Structure

```
avix server <subcommands>           # Server-side operations
avix client <subcommands>           # Client-side operations
  ├── connect [--config]           # Test connectivity
  ├── tui [--trace] [--config]      # Launch dashboard
  ├── atp shell                    # ATP REPL
  ├── agent spawn|list|kill|...   # Agent management
  ├── hil approve|deny            # HIL requests
  ├── logs [--follow] [--config]   # Tail server logs
  ├── service install|list|...     # Service management
  │   └── [--config]               # Per-command config override
  ├── secret set|list|delete       # Secret management
  │   └── [--config]               # Per-command config override
  └── session create|list|...      # Session management
      └── [--config]                # Per-command config override
```

## Rationale

1. **Consistency**: All client-side operations should be under `avix client`
2. **Config locality**: Each subcommand accepts its own `--config` flag (similar to `--log`), allowing per-command overrides without global flag
3. **Discovery**: Users can run `avix client --help` to see all client operations
4. **Cleaner top-level**: Only `server` and `client` at top level

## Completed Work

- [x] `ServerConfig` struct in `avix-client-core/src/server_config.rs`
  - Loads from `~/.config/avix/server.yaml` with defaults
  - Fields: `root`, `log_level`, `address`, `port`, `trace`
- [x] `ClientConfig::load_from(path: Option<PathBuf>)` in `avix-client-core/src/config.rs`
  - Supports custom config path override

## Implementation Steps

### Step 1: Modify ClientCmd enum

In `crates/avix-cli/src/main.rs`, add `--config` to each ClientCmd variant:

```rust
#[derive(Subcommand)]
enum ClientCmd {
    Connect {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Tui {
        #[arg(long)]
        trace: Option<String>,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    // ... etc
}
```

### Step 2: Simplify top-level Cmd enum

Remove Session/Service/Secret from Cmd, keep only Server and Client:

```rust
enum Cmd {
    Server { sub: ServerCmd },
    Client { sub: ClientCmd },
}
```

### Step 3: Update connect_config function

Accept config parameter:

```rust
async fn connect_config(config: Option<PathBuf>, server_url: Option<String>) -> Result<Dispatcher> {
    let mut config = ClientConfig::load_from(config).unwrap_or_else(|_| ClientConfig::default());
    // ...
}
```

### Step 4: Update all call sites

Update all 20+ `connect_config()` calls to pass `config` from the enclosing scope.

### Step 5: Update handlers

Convert:
- `Cmd::Session { sub }` → `ClientCmd::Session { sub, config }`
- `Cmd::Service { sub }` → `ClientCmd::Service { sub, config }`
- `Cmd::Secret { sub }` → `ClientCmd::Secret { sub, config }`

### Step 6: Update tests

Fix test assertions for new command paths (e.g., `avix client service install` instead of `avix service install`).

### Step 7: Verify

- `cargo clippy --package avix-cli -- -D warnings`
- `cargo test --package avix-cli`
- Manual testing of all subcommands

## Notes

- The existing code has inconsistent indentation (mix of 4-space and 8-space) in Session/Service/Secret handlers - manual editing recommended with verification after each change
- The `--config` flag approach mirrors `--log` placement (per-command, not global) for cleaner CLI ergonomics

## Related Files

- `crates/avix-cli/src/main.rs` - CLI entry point
- `crates/avix-client-core/src/config.rs` - ClientConfig with load_from
- `crates/avix-client-core/src/server_config.rs` - ServerConfig (future use)