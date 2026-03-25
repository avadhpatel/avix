# Svc Gap E — CLI Service Subcommands

> **Status:** Pending
> **Priority:** High
> **Depends on:** Svc gap D (installer), client gap F (ATP subcommands)
> **Blocks:** nothing (leaf)
> **Affects:** `crates/avix-cli/src/main.rs`

---

## Problem

There are no `avix service` subcommands. The spec (`service-authoring.md §9`) defines
`avix service install`. The architecture doc lists service lifecycle operations. Without
a CLI surface, operators have no way to manage services without writing ATP commands manually.

---

## Scope

Add the `avix service` subcommand group to `avix-cli`. All install/uninstall/lifecycle
operations go through ATP (`sys/install`, `proc/signal`) when the server is running.
For local-only operations (list installed from disk, show receipt), go through
`avix-core` types directly.

---

## New CLI Surface

```
avix service <subcommand>

avix service install <source> [--checksum sha256:...] [--no-start]
    Install a service from a local path (file://) or URL (https://).

avix service list
    List all installed services with name, version, state.

avix service status <name>
    Show full status of a service (version, PID, state, tools, restart count).

avix service start <name>
    Start a stopped/failed service (sends SIGSTART via ATP).

avix service stop <name>
    Gracefully stop a service (sends SIGTERM signal via ATP).

avix service restart <name>
    Stop then start a service.

avix service uninstall <name> [--force]
    Remove service files from disk. Refuses if service is running (use --force to kill first).

avix service logs <name> [--follow]
    Stream recent output from a service's stdout (reads /proc/services/<name>/log or
    subscribes to ATP events).
```

Global flags from client gap F (`--json`, `--server`, `--identity`, `--credential`)
apply to all subcommands.

---

## Implementation Sketch

### Clap subcommand definitions

```rust
#[derive(Subcommand)]
enum ServiceCmd {
    Install {
        source: String,
        #[arg(long)]
        checksum: Option<String>,
        #[arg(long = "no-start")]
        no_start: bool,
    },
    List,
    Status { name: String },
    Start  { name: String },
    Stop   { name: String },
    Restart { name: String },
    Uninstall {
        name: String,
        #[arg(long)]
        force: bool,
    },
    Logs {
        name: String,
        #[arg(long)]
        follow: bool,
    },
}
```

### `service install`

1. If source is a local path, prefix with `file://` if not already.
2. Build ATP `Cmd { domain: "sys", op: "install", body: { source, checksum, autostart } }`.
3. Call `dispatcher.call(cmd)`.
4. On success, print name + version + tools list.
5. If `--no-start`, pass `"autostart": false` in body.

```
avix service install ./github-svc-1.2.0.tar.gz
→ human: Installed github-svc v1.2.0 — tools: github/list-prs, github/create-issue
→ json:  {"name":"github-svc","version":"1.2.0","tools":["github/list-prs","github/create-issue"]}
```

### `service list`

Read installed services from disk (`ServiceManager::discover_installed(root)`) for
offline mode, or via ATP `proc/services/list` if connected.

```
avix service list
NAME          VERSION  STATE    TOOLS
github-svc    1.2.0    running  2
my-svc        0.1.0    stopped  0
```

JSON mode: `[{"name":"github-svc","version":"1.2.0","state":"running","tool_count":2}, ...]`

### `service status <name>`

Read `/proc/services/<name>/status.yaml` via VFS (or ATP `fs/read`).

```
avix service status github-svc
name:          github-svc
version:       1.2.0
pid:           42
state:         running
endpoint:      /run/avix/github-svc-42.sock
tools:         github/list-prs, github/create-issue
started_at:    2026-03-24T09:00:00Z
restart_count: 0
```

### `service start / stop / restart`

Send signals via ATP `signal/send` with `pid` from status file:
- `start` → `SIGSTART`
- `stop` → `SIGTERM` (maps to graceful shutdown)
- `restart` → `SIGSTOP` then `SIGSTART`

### `service uninstall <name>`

1. Check if running — error if so (unless `--force`).
2. If `--force`, send `SIGKILL` first.
3. Delete `AVIX_ROOT/services/<name>/` directory.
4. On success: `"Uninstalled github-svc"`.

### `service logs <name> [--follow]`

Without `--follow`: read buffered log from `/proc/services/<name>/log` (if it exists).
With `--follow`: subscribe to ATP events for `SysService` kind filtered to `name`.

---

## Tests (in `avix-cli`)

```rust
// Unit tests — clap parsing only (no real ATP connection)

#[test]
fn service_install_parses_source_and_checksum() {
    let cli = Cli::try_parse_from([
        "avix", "service", "install", "./pkg.tar.gz",
        "--checksum", "sha256:abc123",
    ]).unwrap();
    // verify subcommand fields
}

#[test]
fn service_install_no_start_flag() {
    let cli = Cli::try_parse_from([
        "avix", "service", "install", "./pkg.tar.gz", "--no-start",
    ]).unwrap();
    // verify no_start == true
}

#[test]
fn service_list_subcommand_parses() {
    let cli = Cli::try_parse_from(["avix", "service", "list"]).unwrap();
    // verify variant matches
}

#[test]
fn service_uninstall_force_flag() {
    let cli = Cli::try_parse_from([
        "avix", "service", "uninstall", "github-svc", "--force",
    ]).unwrap();
    // verify force == true
}

#[test]
fn service_logs_follow_parses() {
    let cli = Cli::try_parse_from([
        "avix", "service", "logs", "github-svc", "--follow",
    ]).unwrap();
}

#[test]
fn service_status_json_flag() {
    let cli = Cli::try_parse_from([
        "avix", "--json", "service", "status", "github-svc",
    ]).unwrap();
    assert!(cli.json);
}
```

---

## Success Criteria

- [ ] `avix service install <source>` sends `sys/install` ATP command
- [ ] `--checksum` is forwarded in the ATP body
- [ ] `--no-start` sets `autostart: false`
- [ ] `avix service list` reads installed services and prints table (human) or JSON
- [ ] `avix service status <name>` shows version, PID, state, tools
- [ ] `avix service start/stop/restart` sends correct signal via ATP
- [ ] `avix service uninstall --force` kills then removes
- [ ] All clap parsing tests pass
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
