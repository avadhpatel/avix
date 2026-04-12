# PID Persistence Gap A — Design & Implementation Plan

> Root cause: sequential PID allocation resets to 1 on every kernel restart, causing new
> agents to collide with PIDs stored in persistent `SessionRecord.owner_pid` / `pids`
> fields, incorrectly attaching new sessions to old persisted sessions.

---

## Root Cause

`AgentManager::allocate_pid()` returns `max_pid_in_process_table + 1`. After a kernel
restart the `ProcessTable` is empty (in-memory only), so the first agent always gets PID 1
(or PID 2 if there is a kernel pseudo-process at PID 1).

`SessionRecord` and `InvocationRecord` are disk-persisted (redb + YAML). Their PID fields
survive restarts:

| Field | Type | Persisted | Problem |
|---|---|---|---|
| `SessionRecord.owner_pid` | `u32` | yes | stale PID collides with new agent's PID |
| `SessionRecord.pids` | `Vec<u32>` | yes | all PIDs in this list can collide |
| `InvocationRecord.pid` | `u32` | yes | informational only; already documented as unstable |

When the kernel restarts and a new agent is spawned with PID 1, `ProcHandler::spawn`
looks up `active_sessions` (which is *in-memory only*) and finds nothing for PID 1.
It creates a new session — but the new session's `owner_pid = 1` conflicts with the
previously-persisted session whose `owner_pid` was also 1.  Boot-time crash-recovery
clears `Running`/`Paused` sessions to `Idle`, but the stale `owner_pid` remains in the
YAML on disk. Clients that match on `ownerPid` (e.g. the GUI's `NewSessionModal` navigation)
then associate the new agent with the wrong session.

---

## Design Options Considered

### Option A — Persisted monotonic counter (smallest diff)

Store a `u64` high-water mark in `$AVIX_ROOT/kernel/state/pid_hwm.dat`. On boot, read it
and resume from that value. Each allocation atomically increments and fsyncs the file.

- **Pro:** No type changes to `Pid`, `SessionRecord`, `InvocationRecord`. Minimal diff.
- **Con:** If the counter file is lost (fresh install / data migration) collisions resume.
  Requires disk I/O on every allocation (or batching with flush risk).
- **Con:** `u32` eventually wraps (not practical in 4 billion spawns, but still).

### Option B — UUID v4 full (familiar)

Change `Pid` to wrap a `uuid::Uuid` (128 bits). Serialize as the standard hyphenated string.
Already the identifier style for sessions and invocations.

- **Pro:** Zero coordination, universally unique, familiar to the codebase.
- **Con:** Verbose in YAML/JSON (`/proc/550e8400-e29b-41d4-a716-446655440000/`) and
  in ATP wire events. Larger HashMap keys.
- **Con:** Large type-migration blast radius (every `u32` PID literal → UUID).

### Option C — UUID-seeded compact `u64` PID ⭐ Recommended

Generate a 64-bit PID as:

```
bit 63..22 (42 bits): milliseconds since 2025-01-01T00:00:00Z
             → supports ~139 years without overflow
bit 21..0  (22 bits): cryptographically random salt
             → ~4 million distinct values per millisecond,
               collision probability < 2⁻²² per millisecond
```

Generation is pure in-memory — no disk I/O, no coordination, no file to lose. The PID is
self-ordering within a session (earlier spawn = smaller `u64`). Display is an 11-character
base62 string (e.g. `3k9fPq2xmRs`) — human-readable, copy-pasteable, unambiguous.

Change `Pid(u32)` → `Pid(u64)`. The `HashMap<u64, …>` lookup is identical in cost to
`HashMap<u32, …>`. All persisted PID fields (`owner_pid`, `pids`) widen from `u32` to
`u64` — existing YAML/redb records stored as JSON numbers deserialise correctly (JSON
integers fit any width; serde widens automatically).

### Option D — Decouple sessions from PIDs entirely (architectural)

Keep `Pid(u32)` ephemeral. Replace `SessionRecord.owner_pid: u32` with
`owner_invocation_id: String` (UUID). Session finalization looks up the invocation by UUID
instead of PID. `pids: Vec<u32>` becomes a runtime-only field (not persisted).

- **Pro:** Recognises that PIDs are *already* documented as ephemeral; no PID type changes.
- **Con:** Significant session-finalization logic change. Requires coordination between
  `ProcHandler` and `InvocationStore` to resolve `pid → invocation_id` at finalize time.
- **Con:** Breaks `SessionRecord.ownerPid` in the ATP wire format and GUI — client migration.
- **Con:** Larger functional change for a conceptually simple bug fix.

---

## Recommendation: Option C

Option C fixes the root cause with a single principled change:

> **PIDs are globally unique across reboots because the timestamp component makes any
> PID generated after a restart statistically distinct from any PID generated before it.**

It requires no disk coordination, no new files, and the code change is mechanical:
`u32` → `u64` throughout, plus a new `PidAllocator::generate()` function.

Migration compatibility: existing `SessionRecord` YAML files have `ownerPid: 42`. When
deserialized into `u64`, `42u64` is still `42` — no data migration needed. Old persisted
records will never collide with new 64-bit time-seeded PIDs (which start at ~`2^42` ≈
4.4 trillion), so any stale `owner_pid: 42` in a persisted session cannot accidentally
match a new agent spawn.

---

## Scope of Change

### New code: `crates/avix-core/src/types/pid.rs`

```
// Before
pub struct Pid(u32);
impl Pid {
    pub fn new(n: u32) -> Self  { Self(n) }
    pub fn as_u32(&self) -> u32 { self.0 }
    pub fn is_kernel(&self) -> bool { self.0 == 0 }
}

// After
pub struct Pid(u64);
impl Pid {
    /// Generate a new globally-unique PID (call once per spawn).
    pub fn generate() -> Self { … }           // timestamp<<22 | rand22
    pub fn kernel() -> Self { Self(0) }
    pub fn from_u64(n: u64) -> Self { Self(n) }
    pub fn as_u64(&self) -> u64 { self.0 }
    pub fn is_kernel(&self) -> bool { self.0 == 0 }
    pub fn to_short(&self) -> String { … }    // 11-char base62 display
}
impl Display for Pid { … }   // writes to_short() for non-zero, "kernel" for 0
```

`rand` (or `getrandom`) is already a transitive dependency via `uuid`. Use
`rand::thread_rng().gen::<u32>() & 0x3FFFFF` for the 22-bit salt.

`Pid::kernel()` replaces `Pid::new(0)` — makes intent explicit.

### Modified: `crates/avix-core/src/types/pid.rs`
- `Pid(u32)` → `Pid(u64)`
- Remove `as_u32()` / `new(u32)` — add `as_u64()` / `from_u64(u64)` + `generate()`
- Keep `Hash`, `Eq`, `Copy` derives (all are `u64`-safe)

### Modified: `crates/avix-core/src/process/table.rs`
- `HashMap<u32, ProcessEntry>` → `HashMap<u64, ProcessEntry>`
- All `pid.as_u32()` call sites → `pid.as_u64()`

### Modified: `crates/avix-core/src/signal/channels.rs`
- `HashMap<u32, mpsc::Sender<Signal>>` → `HashMap<u64, mpsc::Sender<Signal>>`
- All `pid.as_u32()` call sites → `pid.as_u64()`

### Modified: `crates/avix-core/src/session/record.rs`
- `owner_pid: u32` → `owner_pid: u64`
- `pids: Vec<u32>` → `pids: Vec<u64>`
- `SessionRecord::new(…, owner_pid: u32)` → `owner_pid: u64`
- All methods: `add_pid(pid: u32)` → `add_pid(pid: u64)`, etc.

### Modified: `crates/avix-core/src/invocation/record.rs`
- `pid: u32` → `pid: u64`
- `InvocationRecord::new(…, pid: u32)` → `pid: u64`

### Modified: `crates/avix-core/src/kernel/proc/agent.rs`
- Remove `allocate_pid()` method
- Call `Pid::generate()` instead of `self.allocate_pid().await?`
- All `u32` PID variables from allocation forward → `u64` (via `pid.as_u64()`)

### Modified: `crates/avix-core/src/gateway/event_bus.rs`
- `pub fn agent_output(…, pid: u32, …)` → `pid: u64` internally; emitted as string on wire (see ATP wire format decision below)
- Same for `agent_exit`, `agent_status`, `agent_tool_call`, `agent_tool_result`

### Modified: `crates/avix-core/src/gateway/atp/command.rs`
- `AgentKill { pid: u64 }` — note: see ATP wire format decision below
- `AgentStatus { pid: u64 }` — note: see ATP wire format decision below

### Modified: `crates/avix-core/src/gateway/handlers/mod.rs`
- `next_pid: Arc<AtomicU32>` → remove entirely (PIDs now self-generated)
- Spawn handler uses `Pid::generate()` (or receives the PID returned from `AgentManager::spawn`)

### Cascading u32 → u64 fixups (mechanical, in same files as above)
- `InvocationRecord::new(…, pid: u32)` → `pid: u64`
- `SpawnParams.pid: Pid` (already `Pid`, so no change if table/signal types updated)
- Test literals: `Pid::new(10)` → `Pid::from_u64(10)` or `Pid::generate()` in test setup
- `service/lifecycle.rs` `pid_counter: AtomicU32` → keep as-is (services are not agents;
  their PIDs are local and ephemeral — no persistence concern)

### Not changed
- `InvocationStore` / `SessionStore` disk format — widening `u32 → u64` in JSON is
  backward-compatible (old records parse as `u64` with no data loss)
- VFS proc path `/proc/<pid>/` — Display impl writes `pid.to_short()` which is 11 chars;
  human-readable and unique
- `CapabilityToken` — already binds to `agent_name` and `spawned_by`, not raw PID
- Service PIDs — `service/lifecycle.rs` counter stays `AtomicU32` (services are ephemeral,
  not persisted by PID)

---

## Implementation Order

| Step | File | What changes |
|------|------|---|
| 1 | `crates/avix-core/src/types/pid.rs` | `Pid(u32)` → `Pid(u64)`, add `generate()`, `from_u64()`, `to_short()`, `kernel()` |
| 2 | `crates/avix-core/src/process/table.rs` | `HashMap<u32,…>` → `HashMap<u64,…>`, all `as_u32()` → `as_u64()` |
| 3 | `crates/avix-core/src/signal/channels.rs` | Same map key widening |
| 4 | `crates/avix-core/src/session/record.rs` | `owner_pid`/`pids` u32 → u64 |
| 5 | `crates/avix-core/src/invocation/record.rs` | `pid` u32 → u64 |
| 6 | `crates/avix-core/src/kernel/proc/agent.rs` | Remove `allocate_pid()`, use `Pid::generate()` |
| 7 | `crates/avix-core/src/gateway/event_bus.rs` | Internal pid u32 → u64; wire emission uses `pid.to_string()` |
| 8 | `crates/avix-core/src/gateway/atp/command.rs` | `pid: u32` → `pid: String`; parse back to u64 in handler |
| 9 | `crates/avix-core/src/gateway/handlers/mod.rs` | Remove `next_pid` AtomicU32; parse string pid from ATP commands |
| 10 | All remaining call sites | Mechanical `as_u32()` → `as_u64()`, `Pid::new(n)` → `Pid::from_u64(n)` in tests |
| 11 | `docs/architecture/06-agents.md` | Update PID format spec, example YAML, ATP wire spec |
| 12 | `docs/architecture/14-agent-persistence.md` | Update `owner_pid` type, note collision-safety |

---

## Targeted Tests

After each step, run only the tests for the crate touched:

```bash
cargo check --package avix-core
cargo test --package avix-core session::record::
cargo test --package avix-core invocation::
cargo test --package avix-core process::
cargo test --package avix-core signal::channels::
cargo test --package avix-core kernel::proc::
```

No full-workspace test runs.

---

## Migration Compatibility

Old on-disk YAML:
```yaml
ownerPid: 42
pids: [42]
```

Deserialized by serde into `u64` fields — `42u64` is exact. No data loss. These old
records will never collide with new time-seeded PIDs (which start at ~4.4 trillion).
Boot-time crash recovery already clears stale Running/Paused sessions to Idle and clears
`pids`, so even the stale `owner_pid: 42` becomes inert (it no longer drives any live
finalization logic).

No schema migration script required.

---

## Architecture Invariant Additions

After implementation, add to `docs/architecture/06-agents.md`:

> **PID Format (v0.2+):** PIDs are 64-bit time-seeded values generated by `Pid::generate()`.
> Upper 42 bits = milliseconds since 2025-01-01 UTC; lower 22 bits = random salt.
> PIDs are globally unique across kernel restarts with no coordination overhead.
> Display form: 11-character base62 string (e.g. `3k9fPq2xmRs`).
> `Pid(0)` is reserved for the kernel pseudo-process.
> PIDs are **ephemeral runtime handles** — use invocation UUIDs for durable cross-reboot
> references.

---

## Decisions

### D1 — Display format: decimal string
Use plain decimal (`to_string()` on the inner `u64`) for display and the ATP wire format.
Base62 is prettier but adds a codec with no practical benefit during alpha. `Pid::to_short()`
is an alias for `self.0.to_string()` — can be upgraded to base62 later without changing
wire format if agreed.

### D2 — `Pid::kernel()` sentinel: `Pid(0u64)`
`0u64` remains the kernel sentinel. `Pid::is_kernel()` checks `self.0 == 0`. No change
in semantics.

### D3 — ATP wire format: `pid` is a JSON **string** ✅ DECIDED
**Decision (2026-04-12):** ATP events and commands carry `pid` as a JSON string, not a
JSON number.

Rationale: A 64-bit time-seeded PID exceeds 2^53, which is the safe integer range for
IEEE-754 double-precision (used by all JavaScript JSON parsers). Emitting u64 as a JSON
number would silently corrupt PID values in the GUI and CLI clients. Since the system is
in alpha and breaking changes are acceptable, we switch the ATP wire type for `pid` to
string now rather than patching it later.

Concrete impact:
- `AtpEventBus` helper methods emit `"pid": "4398046511104"` (decimal string) instead of
  `"pid": 4398046511104` (number).
- `ATPCommand` variants that accept a pid from the client (`AgentKill`, `AgentStatus`, etc.)
  parse the field as `String` and convert to `u64` via `str::parse::<u64>()` in the handler.
- GUI/TUI/CLI clients that read pid from ATP events must treat the field as a string.
  Comparison and display are unchanged; arithmetic on PIDs is never needed in clients.

### D4 — Snapshot `sourcePid`: widen to `u64`
`SnapshotFile.spec.sourcePid` widens from `u32` to `u64`. Old snapshot YAML files with
small integer values deserialise into `u64` without data loss (serde widening). No
migration script needed.
