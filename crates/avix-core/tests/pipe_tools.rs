/// Integration tests for Pipe IPC tool handlers (Gap E).
use avix_core::{
    error::AvixError,
    memfs::VfsRouter,
    pipe::{BackpressurePolicy, PipeConfig, PipeDirection, PipeEncoding, PipeManager, ReadResult},
    signal::{SignalChannelRegistry, kind::SignalKind},
    types::Pid,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn pid(n: u32) -> Pid {
    Pid::new(n)
}

fn out_config(src: u32, tgt: u32) -> PipeConfig {
    PipeConfig::new(pid(src), pid(tgt))
}

// ── T-E-01: pipe/open creates pipe and returns pipe_id ────────────────────────

#[tokio::test]
async fn pipe_open_creates_pipe() {
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();
    assert!(id.starts_with("pipe-"));
    assert_eq!(mgr.pipe_count().await, 1);
}

// ── T-E-02: pipe/write succeeds for source agent ──────────────────────────────

#[tokio::test]
async fn pipe_write_by_source_succeeds() {
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();
    mgr.write(&id, pid(10), "hello".into()).await.unwrap();
}

// ── T-E-03: pipe/write fails for non-source agent ────────────────────────────

#[tokio::test]
async fn pipe_write_by_non_source_fails() {
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();
    let result = mgr.write(&id, pid(30), "msg".into()).await;
    assert!(
        matches!(result, Err(AvixError::CapabilityDenied(_))),
        "expected CapabilityDenied, got: {result:?}"
    );
}

// ── T-E-04: pipe/read returns message for target agent ────────────────────────

#[tokio::test]
async fn pipe_read_by_target_succeeds() {
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();
    mgr.write(&id, pid(10), "hello".into()).await.unwrap();
    let result = mgr
        .read(&id, pid(20), Some(Duration::from_millis(200)))
        .await
        .unwrap();
    assert!(
        matches!(result, ReadResult::Message(ref m) if m == "hello"),
        "expected Message(hello), got: {result:?}"
    );
}

// ── T-E-05: pipe/read fails for non-target agent ─────────────────────────────

#[tokio::test]
async fn pipe_read_by_non_target_fails() {
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();
    let result = mgr
        .read(&id, pid(10), Some(Duration::from_millis(50)))
        .await;
    assert!(
        matches!(result, Err(AvixError::CapabilityDenied(_))),
        "expected CapabilityDenied, got: {result:?}"
    );
}

// ── T-E-06: pipe/read times out when no message ──────────────────────────────

#[tokio::test]
async fn pipe_read_times_out() {
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();
    let result = mgr
        .read(&id, pid(20), Some(Duration::from_millis(50)))
        .await
        .unwrap();
    assert!(
        matches!(result, ReadResult::Timeout),
        "expected Timeout, got: {result:?}"
    );
}

// ── T-E-07: pipe/read returns Closed after close ─────────────────────────────

#[tokio::test]
async fn pipe_read_returns_closed_after_close() {
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();
    mgr.close(&id, pid(10), None, None, "test").await.unwrap();
    // After close the pipe is removed from the registry.
    let result = mgr
        .read(&id, pid(20), Some(Duration::from_millis(50)))
        .await;
    assert!(
        matches!(result, Err(AvixError::NotFound(_))),
        "expected NotFound after close, got: {result:?}"
    );
}

// ── T-E-08: pipe/close delivers SIGPIPE to partner ───────────────────────────

#[tokio::test]
async fn pipe_close_delivers_sigpipe_to_partner() {
    use tokio::sync::mpsc;

    // Register pid=20 (target) in the channel registry.
    let channels = SignalChannelRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    channels.register(pid(20), tx).await;

    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), None).await.unwrap();

    // pid=10 (source) closes the pipe → SIGPIPE to pid=20
    mgr.close(&id, pid(10), Some(&channels), None, "source_closed")
        .await
        .unwrap();

    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("timed out waiting for SIGPIPE")
        .expect("channel closed unexpectedly");
    assert_eq!(sig.kind, SignalKind::Pipe);
}

// ── T-E-09: pipe/open writes VFS manifest ────────────────────────────────────

#[tokio::test]
async fn pipe_open_writes_vfs_manifest() {
    use avix_core::memfs::VfsPath;

    let vfs = Arc::new(VfsRouter::new());
    let mgr = PipeManager::new();
    let id = mgr.open(out_config(10, 20), Some(&vfs)).await.unwrap();

    let path = VfsPath::parse(&format!("/proc/10/pipes/{id}.yaml")).unwrap();
    let content = vfs.read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();

    assert!(text.contains("sourcePid: 10"), "missing sourcePid: {text}");
    assert!(text.contains("targetPid: 20"), "missing targetPid: {text}");
    assert!(text.contains("state: open"), "missing state: {text}");
}

// ── T-E-10: Backpressure=Drop silently drops on full ─────────────────────────

#[tokio::test]
async fn backpressure_drop_discards_on_full() {
    let mut config = out_config(10, 20);
    config.buffer_tokens = 2;
    config.backpressure = BackpressurePolicy::Drop;

    let mgr = PipeManager::new();
    let id = mgr.open(config, None).await.unwrap();

    mgr.write(&id, pid(10), "msg1".into()).await.unwrap();
    mgr.write(&id, pid(10), "msg2".into()).await.unwrap();
    // Third write: buffer is full → dropped silently.
    mgr.write(&id, pid(10), "msg3-dropped".into())
        .await
        .unwrap();

    // Only 2 messages should be readable.
    let r1 = mgr
        .read(&id, pid(20), Some(Duration::from_millis(50)))
        .await
        .unwrap();
    let r2 = mgr
        .read(&id, pid(20), Some(Duration::from_millis(50)))
        .await
        .unwrap();
    let r3 = mgr
        .read(&id, pid(20), Some(Duration::from_millis(50)))
        .await
        .unwrap();

    assert!(matches!(r1, ReadResult::Message(_)));
    assert!(matches!(r2, ReadResult::Message(_)));
    assert!(
        matches!(r3, ReadResult::Timeout),
        "third read should timeout (msg dropped)"
    );
}

// ── T-E-11: Backpressure=Error returns error on full ─────────────────────────

#[tokio::test]
async fn backpressure_error_returns_error_on_full() {
    let mut config = out_config(10, 20);
    config.buffer_tokens = 1;
    config.backpressure = BackpressurePolicy::Error;

    let mgr = PipeManager::new();
    let id = mgr.open(config, None).await.unwrap();

    mgr.write(&id, pid(10), "msg1".into()).await.unwrap();
    let result = mgr.write(&id, pid(10), "msg2".into()).await;
    assert!(result.is_err(), "expected error on full buffer");
}

// ── T-E-12: Agent exit closes all owned pipes ────────────────────────────────

#[tokio::test]
async fn agent_exit_closes_owned_pipes() {
    let mgr = PipeManager::new();
    // Two pipes both owned (as source) by pid=10.
    let _id1 = mgr.open(out_config(10, 20), None).await.unwrap();
    let _id2 = mgr.open(out_config(10, 30), None).await.unwrap();
    assert_eq!(mgr.pipe_count().await, 2);

    mgr.close_pipes_for_pid(pid(10), None, None).await.unwrap();
    assert_eq!(mgr.pipe_count().await, 0, "both pipes should be closed");
}

// ── T-E-13: Bidirectional pipe allows both agents to write ───────────────────

#[tokio::test]
async fn bidirectional_pipe_both_agents_write() {
    let mut config = out_config(10, 20);
    config.direction = PipeDirection::Bidirectional;

    let mgr = PipeManager::new();
    let id = mgr.open(config, None).await.unwrap();

    // Both agents can write.
    mgr.write(&id, pid(10), "from-src".into()).await.unwrap();
    mgr.write(&id, pid(20), "from-tgt".into()).await.unwrap();

    // Both agents can read.
    let r1 = mgr
        .read(&id, pid(10), Some(Duration::from_millis(100)))
        .await
        .unwrap();
    let r2 = mgr
        .read(&id, pid(20), Some(Duration::from_millis(100)))
        .await
        .unwrap();

    // Order depends on FIFO; both messages should arrive.
    let msgs: Vec<_> = [r1, r2]
        .into_iter()
        .filter_map(|r| {
            if let ReadResult::Message(m) = r {
                Some(m)
            } else {
                None
            }
        })
        .collect();

    assert!(msgs.contains(&"from-src".to_string()));
    assert!(msgs.contains(&"from-tgt".to_string()));
}

// ── T-E-14: JSON encoding validation rejects non-JSON message ────────────────

#[tokio::test]
async fn json_encoding_rejects_invalid_message() {
    let mut config = out_config(10, 20);
    config.encoding = PipeEncoding::Json;

    let mgr = PipeManager::new();
    let id = mgr.open(config, None).await.unwrap();

    // Valid JSON should succeed.
    mgr.write(&id, pid(10), json!({"key": "value"}).to_string())
        .await
        .unwrap();

    // Invalid JSON should fail.
    let result = mgr.write(&id, pid(10), "not-json!!!".into()).await;
    assert!(result.is_err(), "expected encoding error for invalid JSON");
}
