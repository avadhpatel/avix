# Avix Daemon Gap: Implement \`avix start\` Command

## Spec Reference
CLAUDE.md (boot invariants, ADRs), docs/architecture/02-bootstrap.md (phases), docs/spec/gui-cli-via-atp.md s3 server.rs (spawn/monitor daemon), s6 Flows (config-first → server start → ATP connect).

## Goals
* Add \`avix start --root ~/avix-data\` daemon: kernel.agent PID1 + services (llm/router/fs) + ATP WS localhost:9142/atp.
* Graceful shutdown (SIGTERM → save state).
* Hot-reload config (poll /run/avix/reload-pending).
* client-core auto-spawn/probe/kill if auto_start_server=true.
* Works daemonized (nohup/Docker), logs JSON to file.

## Dependencies
* avix-core: Runtime::start_daemon async (full boot Phase1-4).
* avix-cli: Cmd::Start {root, port=9142}.
* tokio full, tracing json subscriber.

## Files to Create/Edit
* crates/avix-cli/src/main.rs: Cmd::Start subcommand.
* crates/avix-core/src/bootstrap.rs: impl Runtime {async fn start_daemon(self, port: u16) -> Result<()>}
* crates/avix-client-core/src/server.rs: complete spawn_child(\"avix\", [\"start\", \"--root\", path]), probe_ws, kill_child.
* crates/avix-core/Cargo.toml: [bin] avix-daemon? or use avix-cli.
* tests/integration/daemon_boot.rs

## Detailed Tasks
1. avix-cli/main.rs Cmd::Start:
```
rust
#[derive(Subcommand)]
Start {
  /// Runtime root
  #[arg(long)]
  root: PathBuf,
  /// ATP port (default 9142)
  #[arg(long, default=9142)]
  port: u16,
}
```
Handler: let mut runtime = Runtime::bootstrap_with_root(&root).await?; runtime.start_daemon(port).await?;

2. avix-core/bootstrap.rs Runtime::start_daemon:
```
rust
impl Runtime {
  pub async fn start_daemon(mut self, port: u16) -> Result<()> {
    // Phase1 done (auth/master_key)
    self.phase2_kernel().await?;  // spawn kernel.agent PID1
    self.phase3_services().await?;  // llm.svc IPC llm/complete etc.
    self.phase4_atp_gateway(port).await?;  // axum WS /atp + gateway.svc IPC<->ATP
    // Poll reload-pending every 5s
    loop {
      tokio::time::sleep(Duration::from_secs(5)).await;
      if fs::metadata(\"/run/avix/reload-pending\").is_ok() {
        self.hot_reload().await?;
      }
    }
  }
}
```
* kernel: RuntimeExecutor::spawn(kernel_manifest, full tools).
* services: spawn llm.svc (IpcLlmClient multi-provider), router.svc dispatch, fs.svc MemFS.
* ATP: axum::Server ws /atp (auth ATPToken → IPC dispatch → reply).

3. client-core/server.rs ServerHandle:
```
rust
pub struct ServerHandle { child: Child, root: PathBuf }
impl ServerHandle {
  pub async fn ensure_running(&self) -> Result<()> {
    loop {
      if self.probe_ws(\"ws://localhost:9142/atp/health\").await.is_ok() { break Ok(()); }
      if self.child.kill().await.is_err() { /* already dead */ }
      let mut child = tokio::process::Command::new(\"avix\")
        .arg(\"start\").arg(&format!(\"--root={}\", self.root.display()))
        .spawn()?;
      self.child = child;
      tokio::time::sleep(Duration::from_secs(60)).await;  // reconnect grace
    }
  }
}
```

4. Tests: #[tokio::test] daemon_spawn_probes_ok, hot_reload_writes_pending.

5. Docker: avix-docker/main.rs = avix start --root /avix-data.

## Verify
* \`avix config init --root ~/test-avix\`
* \`AVIX_MASTER_KEY=xx avix start --root ~/test-avix\` → daemon PID, ws://localhost:9142/atp/health ok.
* client-core ensure_running auto-spawns.
* \`curl -H 'Authorization: Bearer token' ws://localhost:9142/atp\` connects.
* SIGTERM graceful (save state).

Est: 4h