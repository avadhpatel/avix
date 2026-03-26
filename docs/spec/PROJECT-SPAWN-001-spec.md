# PROJECT-SPAWN-001-spec.md: Full Agent Spawn Workflow (TUI Form → Backend Spawn → Persistence)

## Version
1.0 (Initial) - 2024-10

## Motivation & Problem Statement
Current TUI supports spawning a test agent via 'a' key, but lacks a validated form for custom name/goal. Backend `commands::spawn_agent` is stubbed (returns Err). No ATP handler for spawn in gateway.svc. Agents do not appear in kernel/proc/list. No persistence: agents terminate on TUI disconnect; daemon restart loses all running agents. From testing-agent reports: gaps in end-to-end flow TUI connect → spawn → output → disconnect/reconnect → agent persists + listed.

Aligns with Avix OS goals (Unix-like processes persist independently of terminals) and CLAUDE.md invariants (kernel owns process table, /proc/ ephemeral but daemon state persistent).

## Goals
* Frontend: TUI form (name non-empty, goal valid JSON/quoted) → ATP spawn command.
* Backend: ATP \"spawn\" → gateway translates → IPC kernel/proc/spawn → fork/exec RuntimeExecutor binary + env (PID, CAP_TOKEN, GOAL) → IPC socket connected via router.svc.
* List: kernel/proc/list tool returns Vec&lt;AgentSummary&gt; (pid, name, state, goal snippet).
* Output: Agent stdout/stderr → ATP events → TUI output pane.
* Persist: 
  * TUI disconnect/reconnect: agents continue (no SIGKILL).
  * Daemon restart: daemon scans /run/avix/agents/*.pid + /proc/&lt;pid&gt;/status.yaml → re-adopts (write /proc/, SIGSTART if stopped).
* E2E: avix tui → connect → spawn \"researcher 'analyze logs'\" → see output/list → disconnect → reconnect → agent running + listed.

## Non-Goals
* GUI spawn (TUI-only).
* Docker mode persistence.
* Agent migration across machines.
* Snapshot restore (separate spec).
* Multi-user spawn (single-user daemon for now).

## Architecture Impact
* Minimal: Builds on 06-agents.md (kernel/proc/spawn stub → full impl), 00-overview.md (processes independent).
* New: ATP command \"spawn\" (04-atp.md extension), kernel/proc/list tool.
* Data: Daemon state /data/avix-daemon/agents.yaml (persistent PID map).
* No change to invariants: llm.svc mediation, fresh IPC/call, tool / naming, secrets env-injected.
* Crates: avix-core (kernel spawn logic), avix-cli (TUI form + ATP), gateway.svc handlers.
* Perf: Spawn &lt;500ms target (IPC + fork/exec).

## Detailed Design

### Data Structures
AgentSummary (for kernel/proc/list):
```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct AgentSummary {
    pid: u64,
    name: String,
    state: AgentState,  // running | paused | etc. (from 06-agents.md)
    goal: String,       // truncated
    uptime_secs: u64,
    tools_granted: Vec&lt;String&gt;,
}
```

DaemonState (new, persisted /data/avix-daemon/agents.yaml):
```yaml
apiVersion: avix/v1
kind: DaemonAgents
agents:
  - pid: 123
    name: researcher
    session_id: sess-abc
    spawned_at: 2024-10-01T12:00:00Z
    cap_token_hash: sha256:...
```

### Workflow State Machine
```
TUI Form Idle ── form open ──> Validating ── invalid ──> Error Notif
                    │
                 valid ── submit ──> ATP Spawn Cmd ──> Gateway ──> IPC kernel/proc/spawn
                                              │
                                        fork/exec RuntimeExecutor ──> SIGSTART ──> Running
                                                          │
TUI Disconnect ───────────────────────────────────────────┘
Daemon Restart ── read agents.yaml ── check ps -p pid ── alive? ──> re-adopt (write /proc/, SIGSTART)
```

### New ATP Command (04-atp.md)
```
Command::Spawn {
    name: String,     // validated non-empty alphanumeric
    goal: String,     // LLM prompt
}
→ Event::AgentSpawned { pid: u64, name: String }
→ Event::AgentOutput { pid: u64, chunk: String }
```

### IPC Syscalls (03-ipc.md)
* kernel/proc/spawn {name: str, goal: str} → Result&lt;u64 pid&gt;
* kernel/proc/list {} → Vec&lt;AgentSummary&gt;

Kernel impl:
1. Alloc PID, mint CapToken (user default tools + agent:*, pipe:*, cap:*).
2. Write /proc/&lt;pid&gt;/status.yaml + resolved.yaml (06-agents.md).
3. Exec RuntimeExecutor --pid &lt;pid&gt; --token &lt;token&gt; --goal &lt;goal&gt;.
4. Persist to daemon agents.yaml.
5. Return pid.

### Persistence
* On spawn: append to /data/avix-daemon/agents.yaml (atomic write).
* On daemon boot Phase 3: load agents.yaml → for each: if process alive (kill -0), re-write /proc/ files, queue SIGSTART.
* On agent exit: kernel removes from agents.yaml.
* Cleanup: cron-like kernel task gc stale pids (no /proc/&lt;pid&gt;/status.yaml).

### Security & Capabilities
* Spawn grants: fs:read/write (user tree), llm:complete, agent:spawn/kill/list (scoped to crew), pipe:*.
* HIL: agent/spawn requires human approval? No, user-initiated.
* Errors: EPERM if quota exceeded, ENOENT if binary missing.

### Error Handling
* Validation: ATP rejects invalid name/goal → Event::Error.
* Spawn fail (exec err) → Event::AgentFailed { pid, reason } + remove from list/agents.yaml.
* Orphaned: gc after 5min no heartbeat.

### Performance
* Spawn latency: fork/exec &lt;100ms, proc writes &lt;50us.
* List: O(n) scan process table &lt;10us/target.

## User/Dev Experience
* TUI: 'f' toggle form → name input → goal input → Tab/Enter → list updates + output streams.
* Reconnect: agents resume output.
* CLI: avix spawn researcher \"goal\" (ATP sync).
* Dev: avix tui e2e tests.

## Risks & Trade-offs
* Risk: Daemon restart race (pid reuse) → Mitigate: pid + cap_hash unique key.
* Risk: Fork/exec security → sandbox via cgroups? Future.
* Trade-off: Simple pid file vs DB → file for v1 (MemFS swap later).
* Risk: TUI form UX → usability-agent review.

## Dependencies & Prerequisites
* TUI form skeleton (PROJECT-TUI-001).
* kernel/proc/spawn stub (fs-gap-B).
* ATP Event::Agent* (ATP spec).

## Success Criteria
* E2E test: tui spawn → disconnect → reconnect → list shows running → output continues.
* Daemon restart mid-run → re-adopt success (manual test).
* Tests: unit (spawn/parser), integration (ATP→IPC), coverage &gt;95%.
* Usability: \"Seamless spawn/persist\".
* Perf: spawn &lt;500ms.

## References
* CLAUDE.md: Process model, persistence invariants.
* docs/architecture/00-overview.md, 06-agents.md (spawn/proc).
* docs/dev_plans/fs-gap-B-agent-spawn-vfs-writes.md.
* Recent testing-agent gaps.