# Client Gap F — `avix-cli` ATP Subcommands + `--json` Scripting Mode

> **Status:** Pending
> **Priority:** High — makes the CLI useful for scripting and CI before TUI is ready
> **Depends on:** Client gap E (AppState + commands)
> **Blocks:** Client gap G (TUI — same subcommand surface, different output mode)
> **Affects:** `crates/avix-cli/Cargo.toml`, `crates/avix-cli/src/main.rs`

---

## Problem

The existing `avix-cli` can run agents in-process via `RuntimeExecutor` directly but
cannot talk to a running Avix server over ATP. There is no `agent spawn`, `agent list`,
`hil approve/deny`, or machine-readable (`--json`) output mode. Scripting use-cases
(CI pipelines, shell scripts) need these before the TUI is built.

---

## Scope

Add ATP-aware subcommands to the existing `avix` CLI binary. Add a `--json` global flag
that switches all output to newline-delimited JSON. No TUI yet — plain terminal output
only. Reuse `avix-client-core` for all protocol work.

---

## New CLI Surface

```
avix [--json] [--quiet] <subcommand>

Existing subcommands (unchanged):
  config init    …
  config reload  …
  resolve        …
  run            …      (in-process executor, kept for local dev)

New subcommands:
  connect                  Test ATP connection and print session info
  agent list               List active agents on the server
  agent spawn              Spawn a new agent via ATP
  agent kill <pid>         Send SIGKILL to an agent
  agent pipe <pid>         Send SIGPIPE with text from stdin
  hil list                 List pending HIL requests
  hil approve <hil-id>     Approve a pending HIL request
  hil deny <hil-id>        Deny a pending HIL request
  logs <pid> [--follow]    Stream agent.output events to stdout
```

Global flags:
- `--json` / `-j`: emit newline-delimited JSON objects instead of human text
- `--quiet` / `-q`: suppress informational output; errors only
- `--server <url>`: override `server_url` from config (default: `http://127.0.0.1:7700`)
- `--identity <name>` / `--credential <val>`: override auth config

---

## `Cargo.toml` changes for `avix-cli`

```toml
[dependencies]
avix-client-core = { path = "../avix-client-core" }
# existing deps stay
```

---

## Implementation Sketch

### `main.rs` additions

Add a `GlobalOpts` struct parsed before the subcommand:

```rust
#[derive(Parser)]
#[command(name = "avix", about = "Avix agent OS", version)]
struct Cli {
    #[arg(long, short = 'j', global = true)]
    json: bool,
    #[arg(long, short = 'q', global = true)]
    quiet: bool,
    #[arg(long, global = true)]
    server: Option<String>,
    #[arg(long, global = true)]
    identity: Option<String>,
    #[arg(long, global = true)]
    credential: Option<String>,
    #[command(subcommand)]
    command: Cmd,
}
```

#### Output helper

```rust
fn emit<T: serde::Serialize>(json_mode: bool, human: impl FnOnce(), value: &T) {
    if json_mode {
        println!("{}", serde_json::to_string(value).unwrap());
    } else {
        human();
    }
}
```

#### ATP helper

```rust
async fn make_dispatcher(cli: &Cli) -> Result<(Dispatcher, String /* token */)> {
    let cfg = build_config(cli);
    let client = AtpClient::connect(&cfg.server_url, &cfg.identity, &cfg.credential).await?;
    let token = client.session.token.clone();
    let dispatcher = Dispatcher::new(client);
    Ok((dispatcher, token))
}
```

### New subcommand arms

#### `connect`

```
avix connect
→ human: "Connected to http://127.0.0.1:7700 — session sess-abc role=admin"
→ json:  {"session_id":"sess-abc","role":"admin","server":"http://127.0.0.1:7700"}
```

#### `agent list`

```
avix agent list
→ human: table of PID / NAME / STATUS / GOAL
→ json:  [{"pid":42,"name":"researcher","status":"running","goal":"…"}, …]
```

#### `agent spawn`

```
avix agent spawn --name researcher --goal "summarise /users/alice/q3.pdf" \
                 --cap fs/read --cap llm/complete
→ human: "Agent 'researcher' spawned — PID 42"
→ json:  {"pid":42,"name":"researcher"}
```

#### `agent kill <pid>`

```
avix agent kill 42
→ human: "SIGKILL sent to PID 42"
→ json:  {"ok":true}
```

#### `agent pipe <pid>`

```
echo "Here is the brief" | avix agent pipe 42
→ human: "SIGPIPE sent to PID 42"
→ json:  {"ok":true}
```

#### `hil list`

```
avix hil list
→ human: table of HIL-ID / PID / PROMPT / TIMEOUT
→ json:  [{"hil_id":"h1","pid":42,"prompt":"…","timeout_secs":600}, …]
```

#### `hil approve/deny <hil-id>`

Requires looking up `approval_token` from the pending HIL list first.

```
avix hil approve h1
→ human: "HIL h1 approved"
→ json:  {"ok":true}
```

#### `logs <pid> [--follow]`

Without `--follow`: print buffered output from `agent.output` events already in the
notification store (or empty if none buffered yet).

With `--follow`: subscribe to `EventEmitter::subscribe(EventKind::AgentOutput)` and
stream to stdout until Ctrl-C.

```
avix logs 42 --follow
→ human: raw text from each agent.output event (no JSON wrapping)
→ json:  {"pid":42,"turn":1,"text":"…"} (one line per event)
```

---

## Tests

Tests for the CLI layer live in `crates/avix-cli/tests/` (integration) or in `main.rs`
under `#[cfg(test)]` (unit). Use `assert_cmd` crate for integration tests.

```rust
// Unit — output helper
#[test]
fn emit_json_mode_prints_json() {
    // capture stdout, call emit(true, …, &value), assert valid JSON line
}

#[test]
fn emit_human_mode_calls_human_closure() {
    // capture stdout, call emit(false, || print!("hello"), &()), assert "hello"
}

// Integration — clap parsing
#[test]
fn agent_spawn_parses_multiple_caps() {
    use clap::Parser;
    let cli = Cli::try_parse_from([
        "avix", "--json", "agent", "spawn",
        "--name", "r", "--goal", "g",
        "--cap", "fs/read", "--cap", "llm/complete",
    ]).unwrap();
    assert!(cli.json);
    // verify subcommand fields
}

#[test]
fn logs_follow_flag_parses() {
    let cli = Cli::try_parse_from(["avix", "logs", "42", "--follow"]).unwrap();
    // verify pid == 42 and follow == true
}
```

---

## Dependencies to add to `avix-cli/Cargo.toml`

```toml
avix-client-core = { path = "../avix-client-core" }

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

---

## Success Criteria

- [ ] `avix connect` authenticates and prints session info (human and JSON mode)
- [ ] `avix --json agent list` outputs valid newline-delimited JSON
- [ ] `avix agent spawn` calls `commands::spawn_agent` and prints pid
- [ ] `avix hil approve <id>` calls `commands::resolve_hil` with `approved: true`
- [ ] `avix logs <pid> --follow` streams `AgentOutput` events to stdout
- [ ] All new clap subcommand args parse correctly in unit tests
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
