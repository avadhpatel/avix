# GUI App Gap 2: Complete avix-client-core Shared

## Spec Reference
section 3 (config/server/atp/state/persistence/commands).

## Tasks
1. Extract GUI/CLI shared: config load/init, server spawn/monitor, ATP WS/reconnect, dispatcher, event_emitter, notification/HIL.
2. types.rs: all ATP (Cmd/Reply/Event/HilRequest/Notification).
3. persistence: ui-layout.json atomic save/load.
4. commands: spawn_agent, resolve_hil etc.
5. Tests: 95% cov (tempfile FS, mock dispatcher).

Verify: CLI uses it unchanged, lib test pass.

Est: 2h.