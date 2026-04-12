use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::AvixError;
use crate::invocation::conversation::ConversationEntry;

// ── Internal message type ────────────────────────────────────────────────────

/// A single turn in an agent's conversation history.
/// This is an internal type — it is **not** stored in the on-disk snapshot YAML.
/// `capture()` converts `Vec<SnapshotMessage>` into `spec.contextSummary` + `spec.contextTokenCount`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotMessage {
    pub role: String,
    pub content: String,
}

// ── SnapshotTrigger ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotTrigger {
    Auto,
    Crash,
    #[default]
    Manual,
    Sigsave,
}

// ── CapturedBy ───────────────────────────────────────────────────────────────

/// Who triggered the snapshot capture.
/// Serialises as `"kernel"`, `"user:1001"`, `"agent:57"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapturedBy {
    /// Kernel-triggered (e.g. SIGSAVE, auto-interval, crash).
    Kernel,
    /// A specific human user (UID).
    User(u32),
    /// An agent (PID) triggered via `snap/save` syscall.
    Agent(u32),
}

impl Serialize for CapturedBy {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            CapturedBy::Kernel => s.serialize_str("kernel"),
            CapturedBy::User(uid) => s.serialize_str(&format!("user:{uid}")),
            CapturedBy::Agent(pid) => s.serialize_str(&format!("agent:{pid}")),
        }
    }
}

impl<'de> Deserialize<'de> for CapturedBy {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "kernel" {
            return Ok(CapturedBy::Kernel);
        }
        if let Some(rest) = s.strip_prefix("user:") {
            let uid = rest.parse::<u32>().map_err(serde::de::Error::custom)?;
            return Ok(CapturedBy::User(uid));
        }
        if let Some(rest) = s.strip_prefix("agent:") {
            let pid = rest.parse::<u32>().map_err(serde::de::Error::custom)?;
            return Ok(CapturedBy::Agent(pid));
        }
        Err(serde::de::Error::custom(format!(
            "invalid CapturedBy value: '{s}'"
        )))
    }
}

// ── Metadata ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMetadata {
    /// Human-readable name: `<agentName>-<YYYYMMDD>-<HHMM>`.
    pub name: String,
    pub agent_name: String,
    /// PID of the agent at capture time.
    pub source_pid: u32,
    pub captured_at: DateTime<Utc>,
    pub captured_by: CapturedBy,
    pub trigger: SnapshotTrigger,
}

// ── Spec sub-types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMemory {
    pub episodic_events: u32,
    pub semantic_keys: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub request_id: String,
    pub resource: String,
    pub name: String,
    /// Always `"in-flight"` for requests captured mid-execution.
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPipe {
    pub pipe_id: String,
    /// Always `"open"` for pipes captured mid-execution.
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEnvironment {
    pub temperature: f32,
    /// SHA-256 fingerprint of the capability token at capture time.
    pub capability_token: String,
    /// Original tool list at capture time; used to issue a fresh token on restore.
    #[serde(default)]
    pub granted_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSpec {
    pub goal: String,
    pub context_summary: String,
    pub context_token_count: u32,
    #[serde(default)]
    pub memory: SnapshotMemory,
    #[serde(default)]
    pub pending_requests: Vec<PendingRequest>,
    #[serde(default)]
    pub pipes: Vec<SnapshotPipe>,
    pub environment: SnapshotEnvironment,
    /// SHA-256 integrity hash over the canonical YAML (with this field zeroed).
    pub checksum: String,
}

// ── SnapshotFile — the on-disk YAML envelope ─────────────────────────────────

/// The `kind: Snapshot` YAML envelope written to
/// `/users/<username>/snapshots/<agent>-<timestamp>.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: SnapshotMetadata,
    pub spec: SnapshotSpec,
}

impl SnapshotFile {
    pub fn new(metadata: SnapshotMetadata, spec: SnapshotSpec) -> Self {
        Self {
            api_version: "avix/v1".into(),
            kind: "Snapshot".into(),
            metadata,
            spec,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Generate the human-readable snapshot name from agent name and captured_at.
    pub fn make_name(agent_name: &str, captured_at: &DateTime<Utc>) -> String {
        format!("{}-{}", agent_name, captured_at.format("%Y%m%d-%H%M"))
    }

    /// VFS path where this snapshot is stored.
    pub fn vfs_path(&self, username: &str) -> String {
        format!("/users/{}/snapshots/{}.yaml", username, self.metadata.name)
    }
}

// ── capture() — build a SnapshotFile from live executor state ─────────────────

/// Parameters needed to capture a snapshot from a live executor.
pub struct CaptureParams<'a> {
    pub agent_name: &'a str,
    pub pid: u32,
    pub username: &'a str,
    pub goal: &'a str,
    pub message_history: &'a [ConversationEntry],
    pub temperature: f32,
    pub granted_tools: &'a [String],
    pub trigger: SnapshotTrigger,
    pub captured_by: CapturedBy,
    pub memory: SnapshotMemory,
    pub pending_requests: Vec<PendingRequest>,
    pub open_pipes: Vec<SnapshotPipe>,
}

/// Build a `SnapshotFile` from the current executor state.
/// The checksum is computed and embedded before returning.
pub fn capture(params: CaptureParams<'_>) -> SnapshotFile {
    use crate::snapshot::checksum::{compute_checksum, sha256_hex};

    let captured_at = Utc::now();
    let name = SnapshotFile::make_name(params.agent_name, &captured_at);

    // Rough token-count estimate: total chars / 4
    let context_chars: usize = params
        .message_history
        .iter()
        .map(|entry| entry.content.len())
        .sum();
    let context_token_count = (context_chars / 4).max(1) as u32;

    let context_summary = build_context_summary(params.message_history);

    // Capability token fingerprint = sha256 of joined tool names
    let capability_token = format!(
        "sha256:{}",
        sha256_hex(params.granted_tools.join(",").as_bytes())
    );

    let spec = SnapshotSpec {
        goal: params.goal.to_string(),
        context_summary,
        context_token_count,
        memory: params.memory,
        pending_requests: params.pending_requests,
        pipes: params.open_pipes,
        environment: SnapshotEnvironment {
            temperature: params.temperature,
            capability_token,
            granted_tools: params.granted_tools.to_vec(),
        },
        checksum: String::new(), // populated after computing checksum below
    };

    let mut file = SnapshotFile::new(
        SnapshotMetadata {
            name,
            agent_name: params.agent_name.to_string(),
            source_pid: params.pid,
            captured_at,
            captured_by: params.captured_by,
            trigger: params.trigger,
        },
        spec,
    );

    let checksum = compute_checksum(&file);
    file.spec.checksum = format!("sha256:{checksum}");
    file
}

fn build_context_summary(history: &[ConversationEntry]) -> String {
    use crate::invocation::conversation::Role;
    // Return the last assistant turn (up to 200 chars), or a generic placeholder.
    history
        .iter()
        .rev()
        .find(|entry| entry.role == Role::Assistant)
        .map(|entry| {
            let len = entry.content.len().min(200);
            entry.content[..len].to_string()
        })
        .unwrap_or_else(|| "No context available.".to_string())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_test_snapshot(agent_name: &str, source_pid: u32) -> SnapshotFile {
        let captured_at = Utc::now();
        let name = SnapshotFile::make_name(agent_name, &captured_at);
        SnapshotFile::new(
            SnapshotMetadata {
                name,
                agent_name: agent_name.to_string(),
                source_pid,
                captured_at,
                captured_by: CapturedBy::Kernel,
                trigger: SnapshotTrigger::Manual,
            },
            SnapshotSpec {
                goal: "Test goal".into(),
                context_summary: "Some context.".into(),
                context_token_count: 100,
                memory: SnapshotMemory::default(),
                pending_requests: vec![],
                pipes: vec![],
                environment: SnapshotEnvironment {
                    temperature: 0.7,
                    capability_token: "sha256:abc".into(),
                    granted_tools: vec!["fs/read".into()],
                },
                checksum: "sha256:placeholder".into(),
            },
        )
    }

    // T-SA-01: SnapshotFile round-trips through YAML
    #[test]
    fn snapshot_file_round_trips() {
        let meta = SnapshotMetadata {
            name: "researcher-20260315-0741".into(),
            agent_name: "researcher".into(),
            source_pid: 57,
            captured_at: Utc::now(),
            captured_by: CapturedBy::Kernel,
            trigger: SnapshotTrigger::Auto,
        };
        let spec = SnapshotSpec {
            goal: "Research quantum computing".into(),
            context_summary: "Found 12 sources. Synthesising.".into(),
            context_token_count: 64_000,
            memory: SnapshotMemory {
                episodic_events: 14,
                semantic_keys: 8,
            },
            pending_requests: vec![PendingRequest {
                request_id: "req-abc124".into(),
                resource: "tool".into(),
                name: "web".into(),
                status: "in-flight".into(),
            }],
            pipes: vec![SnapshotPipe {
                pipe_id: "pipe-001".into(),
                state: "open".into(),
            }],
            environment: SnapshotEnvironment {
                temperature: 0.7,
                capability_token: "sha256:tokenSig789".into(),
                granted_tools: vec!["fs/read".into()],
            },
            checksum: "sha256:snap001".into(),
        };
        let file = SnapshotFile::new(meta, spec);
        let yaml = file.to_yaml().unwrap();
        let parsed = SnapshotFile::from_str(&yaml).unwrap();
        assert_eq!(parsed.kind, "Snapshot");
        assert_eq!(parsed.metadata.agent_name, "researcher");
        assert_eq!(parsed.metadata.source_pid, 57);
        assert_eq!(parsed.spec.context_token_count, 64_000);
        assert_eq!(parsed.spec.pending_requests.len(), 1);
        assert_eq!(parsed.spec.pipes.len(), 1);
    }

    // T-SA-02: CapturedBy serialises and deserialises correctly
    #[test]
    fn captured_by_round_trips() {
        let cases = [
            (CapturedBy::Kernel, "kernel"),
            (CapturedBy::User(1001), "user:1001"),
            (CapturedBy::Agent(57), "agent:57"),
        ];
        for (variant, expected) in &cases {
            let yaml = serde_yaml::to_string(variant).unwrap();
            assert!(
                yaml.trim() == *expected,
                "serialise: got {yaml:?}, want {expected:?}"
            );
            let parsed: CapturedBy = serde_yaml::from_str(expected).unwrap();
            assert_eq!(parsed, *variant);
        }
    }

    // T-SA-03: SnapshotTrigger serialises to lowercase
    #[test]
    fn snapshot_trigger_serialises_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&SnapshotTrigger::Sigsave)
                .unwrap()
                .trim(),
            "sigsave"
        );
        assert_eq!(
            serde_yaml::to_string(&SnapshotTrigger::Auto)
                .unwrap()
                .trim(),
            "auto"
        );
    }

    // T-SA-04: vfs_path() generates correct path
    #[test]
    fn snapshot_file_vfs_path() {
        let file = make_test_snapshot("researcher", 57);
        let path = file.vfs_path("alice");
        assert!(
            path.starts_with("/users/alice/snapshots/researcher-"),
            "got: {path}"
        );
        assert!(path.ends_with(".yaml"), "got: {path}");
    }

    // T-SA-07: make_name() produces readable format
    #[test]
    fn snapshot_make_name_format() {
        let dt = Utc.with_ymd_and_hms(2026, 3, 15, 7, 41, 0).unwrap();
        assert_eq!(
            SnapshotFile::make_name("researcher", &dt),
            "researcher-20260315-0741"
        );
    }
}
