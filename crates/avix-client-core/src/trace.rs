//! Client-side tracer for notification and HIL activity.
//!
//! Used by `NotificationStore` when `--trace notifications` is enabled.
//! Writes one JSONL line per event to `<log_dir>/trace-notifications.jsonl`.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::io::AsyncWriteExt;

/// Whether notification tracing is enabled.
#[derive(Debug, Clone, Default)]
pub struct ClientTraceFlags {
    pub notifications: bool,
}

impl ClientTraceFlags {
    /// Parse from a comma-separated string (`notifications`, `notif`, `all`).
    pub fn from_csv(s: &str) -> Self {
        let mut flags = Self::default();
        for part in s.split(',') {
            match part.trim() {
                "notifications" | "notif" | "all" => flags.notifications = true,
                _ => {}
            }
        }
        flags
    }

    pub fn is_any_enabled(&self) -> bool {
        self.notifications
    }
}

/// Lightweight async, fire-and-forget tracer for the client process.
pub struct ClientTracer {
    pub flags: ClientTraceFlags,
    log_dir: PathBuf,
}

impl ClientTracer {
    pub fn new(flags: ClientTraceFlags, log_dir: PathBuf) -> Arc<Self> {
        if flags.is_any_enabled() {
            let _ = std::fs::create_dir_all(&log_dir);
        }
        Arc::new(Self { flags, log_dir })
    }

    pub fn noop() -> Arc<Self> {
        Arc::new(Self {
            flags: ClientTraceFlags::default(),
            log_dir: PathBuf::new(),
        })
    }

    fn ts() -> String {
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn emit(&self, filename: &'static str, event: serde_json::Value) {
        let path = self.log_dir.join(filename);
        tokio::spawn(async move {
            let line = format!("{}\n", event);
            if let Ok(mut f) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
            {
                let _ = f.write_all(line.as_bytes()).await;
            }
        });
    }

    pub fn notification_added(&self, kind: &str, id: &str, message: &str, agent_pid: Option<u64>) {
        if !self.flags.notifications {
            return;
        }
        self.emit(
            "trace-notifications.jsonl",
            serde_json::json!({
                "ts": Self::ts(),
                "kind": "notification.added",
                "notif_kind": kind,
                "id": id,
                "message": message,
                "agent_pid": agent_pid,
            }),
        );
    }

    pub fn hil_request(&self, pid: u64, hil_id: &str, prompt: &str) {
        if !self.flags.notifications {
            return;
        }
        self.emit(
            "trace-notifications.jsonl",
            serde_json::json!({
                "ts": Self::ts(),
                "kind": "notification.hil_request",
                "pid": pid,
                "hil_id": hil_id,
                "prompt": prompt,
            }),
        );
    }

    pub fn hil_resolved(&self, hil_id: &str, outcome: &str) {
        if !self.flags.notifications {
            return;
        }
        self.emit(
            "trace-notifications.jsonl",
            serde_json::json!({
                "ts": Self::ts(),
                "kind": "notification.hil_resolved",
                "hil_id": hil_id,
                "outcome": outcome,
            }),
        );
    }
}
