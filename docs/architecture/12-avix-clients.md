# 12-avix-clients COMPLETE

## Impl Details

### avix-client-core Modules
- `atp/`: WS client (connect/send/next_frame, reconnect), Dispatcher (req/reply), EventEmitter (typed events: agent.status, hil.request,...), types (Cmd/Frame/Reply/Event)
- `commands.rs`: proc/spawn → pid, proc/list → Vec<AgentStatus>, signal/send (SIGRESUME hil, SIGPIPE text)
- `state.rs`: AppState (RwLock): config (server_url, auto_start), dispatcher, emitter, NotificationStore, agents Vec<ActiveAgent>, connection_status, server_handle, pending_hils hil_id→(pid,token), emit_callback
- `notification.rs`: NotificationStore (add/resolve/all)
- `persistence.rs`: save/load json (notifications.json, layout.json)
- `config.rs`: ClientConfig
- `server.rs`: ServerHandle (ensure_running daemon)

### Tauri Cmds/Events
See 11-tauri-backend.md

### Frontend
See 10-tauri-client.md

### Daemon Runtime::start_daemon Phases
From `crates/avix-core/src/bootstrap/mod.rs`:

**bootstrap_with_root()** (pre-daemon):
1. Phase 0: init
2. Phase 1: VFS ephemeral (/proc/, /kernel/) — phase1::run()
3. Phase 2: auth.conf load, AVIX_MASTER_KEY→memory+zero env, persistent mounts — phase2::mount_persistent_trees()
4. Phase 3: mock service PIDs (logger,memfs,auth,router,tool-registry,llm,exec,mcp-bridge,gateway)

**start_daemon(port)**:
1. Phase 2: kernel.agent PID1 spawn
2. Phase 3: services spawn (llm.svc, router.svc, ...)
3. Phase 4: ATP gateway WS/TLS on port
4. Loop: poll /run/avix/reload-pending → hot_reload()