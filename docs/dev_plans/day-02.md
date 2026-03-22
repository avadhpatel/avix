# Day 2 — Core Types & Validation

> **Goal:** Define and test every foundational type used across `avix-core`: PIDs, IPC addresses, roles, tokens, tool categories, modalities, and the capability→tool mapping. No services, no I/O — pure data types with full validation logic.

---

## Pre-flight: Verify Day 1

```bash
# Run from repo root
cargo build --workspace       # must exit 0
cargo test --workspace        # must exit 0
cargo clippy --workspace -- -D warnings   # must exit 0

# Confirm structure
ls crates/avix-core/src/lib.rs
ls CLAUDE.md
ls .github/workflows/ci.yml
ls docs/architecture/00-overview.md
```

All checks must pass before writing any new code.

---

## Step 1 — Create the Module Tree

Replace `crates/avix-core/src/lib.rs` with the module declarations. Create one file per module:

```
crates/avix-core/src/
├── lib.rs
├── types/
│   ├── mod.rs
│   ├── pid.rs
│   ├── ipc_addr.rs
│   ├── role.rs
│   ├── token.rs
│   ├── modality.rs
│   ├── tool.rs
│   └── capability_map.rs
└── error.rs
```

**`src/lib.rs`**

```rust
pub mod error;
pub mod types;
```

**`src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AvixError {
    #[error("invalid PID: {0}")]
    InvalidPid(String),

    #[error("invalid IPC address: {0}")]
    InvalidIpcAddr(String),

    #[error("invalid tool name '{name}': {reason}")]
    InvalidToolName { name: String, reason: String },

    #[error("unknown credential type: {0}")]
    UnknownCredentialType(String),

    #[error("config parse error: {0}")]
    ConfigParse(String),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}
```

**`src/types/mod.rs`**

```rust
pub mod capability_map;
pub mod ipc_addr;
pub mod modality;
pub mod pid;
pub mod role;
pub mod token;
pub mod tool;

pub use capability_map::CapabilityToolMap;
pub use ipc_addr::IpcAddr;
pub use modality::Modality;
pub use pid::Pid;
pub use role::Role;
pub use token::{ATPToken, CapabilityToken};
pub use tool::{ToolCategory, ToolName, ToolState};
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/types.rs`:

```rust
//! Day 2 — Core type tests. All tests must be written before implementation.

use avix_core::types::*;
use avix_core::error::AvixError;

// ── Pid ───────────────────────────────────────────────────────────────────────

#[test]
fn pid_zero_is_kernel() {
    let pid = Pid::new(0);
    assert!(pid.is_kernel());
}

#[test]
fn pid_nonzero_is_not_kernel() {
    assert!(!Pid::new(57).is_kernel());
}

#[test]
fn pid_display() {
    assert_eq!(Pid::new(42).to_string(), "42");
}

#[test]
fn pid_ordering() {
    assert!(Pid::new(1) < Pid::new(2));
}

// ── IpcAddr ───────────────────────────────────────────────────────────────────

#[test]
fn ipc_addr_kernel_unix() {
    let addr = IpcAddr::from_name("kernel");
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/kernel.sock");
}

#[test]
fn ipc_addr_kernel_windows() {
    let addr = IpcAddr::from_name("kernel");
    #[cfg(windows)]
    assert_eq!(addr.os_path(), r"\\.\pipe\avix-kernel");
}

#[test]
fn ipc_addr_agent_unix() {
    let addr = IpcAddr::for_agent(Pid::new(57));
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/agents/57.sock");
}

#[test]
fn ipc_addr_service_unix() {
    let addr = IpcAddr::for_service("github-svc");
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/services/github-svc.sock");
}

#[test]
fn ipc_addr_router_unix() {
    let addr = IpcAddr::router();
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/router.sock");
}

// ── Role ──────────────────────────────────────────────────────────────────────

#[test]
fn role_ordering_admin_highest() {
    assert!(Role::Admin > Role::Operator);
    assert!(Role::Operator > Role::User);
    assert!(Role::User > Role::Guest);
}

#[test]
fn role_can_access_proc_domain() {
    assert!(Role::Guest.can_access_domain("proc"));
    assert!(Role::User.can_access_domain("proc"));
}

#[test]
fn role_sys_requires_admin() {
    assert!(!Role::User.can_access_domain("sys"));
    assert!(!Role::Operator.can_access_domain("sys"));
    assert!(Role::Admin.can_access_domain("sys"));
}

#[test]
fn role_cap_requires_admin() {
    assert!(!Role::Operator.can_access_domain("cap"));
    assert!(Role::Admin.can_access_domain("cap"));
}

#[test]
fn role_from_str() {
    assert_eq!("admin".parse::<Role>().unwrap(), Role::Admin);
    assert_eq!("operator".parse::<Role>().unwrap(), Role::Operator);
    assert_eq!("user".parse::<Role>().unwrap(), Role::User);
    assert_eq!("guest".parse::<Role>().unwrap(), Role::Guest);
    assert!("unknown".parse::<Role>().is_err());
}

// ── Modality ──────────────────────────────────────────────────────────────────

#[test]
fn modality_as_str() {
    assert_eq!(Modality::Text.as_str(),          "text");
    assert_eq!(Modality::Image.as_str(),         "image");
    assert_eq!(Modality::Speech.as_str(),        "speech");
    assert_eq!(Modality::Transcription.as_str(), "transcription");
    assert_eq!(Modality::Embedding.as_str(),     "embedding");
}

#[test]
fn modality_from_str() {
    assert_eq!("text".parse::<Modality>().unwrap(), Modality::Text);
    assert_eq!("image".parse::<Modality>().unwrap(), Modality::Image);
    assert!("video".parse::<Modality>().is_err());
}

#[test]
fn modality_round_trip() {
    for m in Modality::all() {
        assert_eq!(m.as_str().parse::<Modality>().unwrap(), m);
    }
}

// ── ToolName ──────────────────────────────────────────────────────────────────

#[test]
fn tool_name_valid() {
    assert!(ToolName::parse("fs/read").is_ok());
    assert!(ToolName::parse("mcp/github/list-prs").is_ok());
    assert!(ToolName::parse("llm/generate-image").is_ok());
}

#[test]
fn tool_name_rejects_double_underscore() {
    let err = ToolName::parse("bad__name").unwrap_err();
    assert!(matches!(err, AvixError::InvalidToolName { .. }));
}

#[test]
fn tool_name_rejects_empty() {
    assert!(ToolName::parse("").is_err());
}

#[test]
fn tool_name_mangle() {
    assert_eq!(ToolName::parse("fs/write").unwrap().mangled(), "fs__write");
    assert_eq!(
        ToolName::parse("mcp/github/list-prs").unwrap().mangled(),
        "mcp__github__list-prs"
    );
}

#[test]
fn tool_name_unmangle() {
    assert_eq!(
        ToolName::unmangle("mcp__github__list-prs").unwrap().as_str(),
        "mcp/github/list-prs"
    );
}

#[test]
fn tool_name_mangle_round_trip() {
    let original = "llm/generate-image";
    let mangled  = ToolName::parse(original).unwrap().mangled();
    let back     = ToolName::unmangle(&mangled).unwrap();
    assert_eq!(back.as_str(), original);
}

// ── ToolState ─────────────────────────────────────────────────────────────────

#[test]
fn tool_state_available_can_transition() {
    assert!(ToolState::Available.can_transition_to(&ToolState::Degraded));
    assert!(ToolState::Available.can_transition_to(&ToolState::Unavailable));
}

#[test]
fn tool_state_unavailable_can_recover() {
    assert!(ToolState::Unavailable.can_transition_to(&ToolState::Available));
    assert!(ToolState::Unavailable.can_transition_to(&ToolState::Degraded));
}

// ── ToolCategory ──────────────────────────────────────────────────────────────

#[test]
fn tool_category_direct() {
    assert_eq!(ToolCategory::classify("fs/read"),      ToolCategory::Direct);
    assert_eq!(ToolCategory::classify("llm/complete"), ToolCategory::Direct);
    assert_eq!(ToolCategory::classify("exec/python"),  ToolCategory::Direct);
}

#[test]
fn tool_category_avix_behaviour() {
    assert_eq!(ToolCategory::classify("agent/spawn"),       ToolCategory::AvixBehaviour);
    assert_eq!(ToolCategory::classify("pipe/open"),         ToolCategory::AvixBehaviour);
    assert_eq!(ToolCategory::classify("cap/request-tool"),  ToolCategory::AvixBehaviour);
    assert_eq!(ToolCategory::classify("cap/escalate"),      ToolCategory::AvixBehaviour);
    assert_eq!(ToolCategory::classify("job/watch"),         ToolCategory::AvixBehaviour);
}

// ── CapabilityToolMap ─────────────────────────────────────────────────────────

#[test]
fn capability_map_spawn_grants_agent_tools() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("spawn");
    assert!(tools.contains(&"agent/spawn"));
    assert!(tools.contains(&"agent/list"));
    assert!(tools.contains(&"agent/wait"));
    assert!(tools.contains(&"agent/send-message"));
}

#[test]
fn capability_map_pipe_grants_pipe_tools() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("pipe");
    assert!(tools.contains(&"pipe/open"));
    assert!(tools.contains(&"pipe/write"));
    assert!(tools.contains(&"pipe/read"));
    assert!(tools.contains(&"pipe/close"));
}

#[test]
fn capability_map_always_present_tools() {
    let map = CapabilityToolMap::default();
    let always = map.always_present();
    assert!(always.contains(&"cap/request-tool"));
    assert!(always.contains(&"cap/escalate"));
    assert!(always.contains(&"cap/list"));
    assert!(always.contains(&"job/watch"));
}

#[test]
fn capability_map_llm_inference_grants_complete() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("llm:inference");
    assert!(tools.contains(&"llm/complete"));
}

#[test]
fn capability_map_llm_image_grants_generate_image() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("llm:image");
    assert!(tools.contains(&"llm/generate-image"));
}

#[test]
fn capability_map_unknown_capability_returns_empty() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("not:a:real:cap");
    assert!(tools.is_empty());
}
```

---

## Step 3 — Implement the Types

**`src/types/pid.rs`**

```rust
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct Pid(u32);

impl Pid {
    pub fn new(n: u32) -> Self { Self(n) }
    pub fn is_kernel(&self) -> bool { self.0 == 0 }
    pub fn as_u32(&self) -> u32 { self.0 }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

**`src/types/ipc_addr.rs`**

```rust
use super::Pid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcAddr(String);

impl IpcAddr {
    pub fn from_name(name: &str) -> Self {
        #[cfg(unix)]
        return Self(format!("/run/avix/{}.sock", name));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-{}", name));
    }

    pub fn for_agent(pid: Pid) -> Self {
        #[cfg(unix)]
        return Self(format!("/run/avix/agents/{}.sock", pid));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-agent-{}", pid));
    }

    pub fn for_service(name: &str) -> Self {
        #[cfg(unix)]
        return Self(format!("/run/avix/services/{}.sock", name));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-svc-{}", name));
    }

    pub fn router() -> Self { Self::from_name("router") }
    pub fn kernel() -> Self { Self::from_name("kernel") }
    pub fn auth()   -> Self { Self::from_name("auth") }
    pub fn memfs()  -> Self { Self::from_name("memfs") }

    pub fn os_path(&self) -> &str { &self.0 }
}
```

**`src/types/role.rs`**

```rust
use std::str::FromStr;
use crate::error::AvixError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Guest    = 0,
    User     = 1,
    Operator = 2,
    Admin    = 3,
}

impl Role {
    pub fn can_access_domain(&self, domain: &str) -> bool {
        match domain {
            "proc" | "fs" | "llm" | "exec" | "jobs" => true,
            "sys" | "cap" => *self >= Role::Admin,
            "kernel" => *self >= Role::Operator,
            _ => false,
        }
    }
}

impl FromStr for Role {
    type Err = AvixError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin"    => Ok(Role::Admin),
            "operator" => Ok(Role::Operator),
            "user"     => Ok(Role::User),
            "guest"    => Ok(Role::Guest),
            other => Err(AvixError::ConfigParse(format!("unknown role: {other}"))),
        }
    }
}
```

**`src/types/modality.rs`**

```rust
use std::str::FromStr;
use crate::error::AvixError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
    Speech,
    Transcription,
    Embedding,
}

impl Modality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text          => "text",
            Self::Image         => "image",
            Self::Speech        => "speech",
            Self::Transcription => "transcription",
            Self::Embedding     => "embedding",
        }
    }

    pub fn all() -> &'static [Modality] {
        &[
            Modality::Text,
            Modality::Image,
            Modality::Speech,
            Modality::Transcription,
            Modality::Embedding,
        ]
    }
}

impl FromStr for Modality {
    type Err = AvixError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text"          => Ok(Modality::Text),
            "image"         => Ok(Modality::Image),
            "speech"        => Ok(Modality::Speech),
            "transcription" => Ok(Modality::Transcription),
            "embedding"     => Ok(Modality::Embedding),
            other => Err(AvixError::ConfigParse(format!("unknown modality: {other}"))),
        }
    }
}
```

**`src/types/tool.rs`**

```rust
use crate::error::AvixError;

/// A validated Avix tool name (uses `/` as namespace separator).
/// Invariant: never contains `__` (double underscore).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolName(String);

impl ToolName {
    /// Parse and validate an Avix tool name.
    pub fn parse(s: &str) -> Result<Self, AvixError> {
        if s.is_empty() {
            return Err(AvixError::InvalidToolName {
                name: s.to_string(),
                reason: "name must not be empty".into(),
            });
        }
        if s.contains("__") {
            return Err(AvixError::InvalidToolName {
                name: s.to_string(),
                reason: "name must not contain '__' (reserved for wire mangling)".into(),
            });
        }
        Ok(Self(s.to_string()))
    }

    /// Mangle for the wire: replace `/` with `__`.
    pub fn mangled(&self) -> String {
        self.0.replace('/', "__")
    }

    /// Unmangle a wire name back to an Avix name.
    pub fn unmangle(mangled: &str) -> Result<Self, AvixError> {
        let unmangled = mangled.replace("__", "/");
        Self::parse(&unmangled)
    }

    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Runtime availability state of a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    Available,
    Degraded,
    Unavailable,
}

impl ToolState {
    pub fn can_transition_to(&self, next: &ToolState) -> bool {
        !matches!((self, next), (ToolState::Available, ToolState::Available))
    }
}

/// How the RuntimeExecutor exposes a capability to the LLM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCategory {
    /// LLM calls directly; RuntimeExecutor validates + dispatches via IPC.
    Direct,
    /// Avix-specific; RuntimeExecutor registers at spawn and translates to syscall/IPC.
    AvixBehaviour,
    /// Transparent — LLM never sees these; RuntimeExecutor handles automatically.
    Transparent,
}

impl ToolCategory {
    pub fn classify(tool_name: &str) -> Self {
        let ns = tool_name.split('/').next().unwrap_or("");
        match ns {
            "agent" | "pipe" | "cap" | "job" => Self::AvixBehaviour,
            _ => Self::Direct,
        }
    }
}
```

**`src/types/capability_map.rs`**

```rust
use std::collections::HashMap;

/// Maps capability grant strings to the Category 2 tool names they unlock.
pub struct CapabilityToolMap {
    map: HashMap<&'static str, Vec<&'static str>>,
    always: Vec<&'static str>,
}

impl Default for CapabilityToolMap {
    fn default() -> Self {
        let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        map.insert("spawn", vec!["agent/spawn", "agent/list", "agent/wait", "agent/send-message"]);
        map.insert("pipe",  vec!["pipe/open", "pipe/write", "pipe/read", "pipe/close"]);
        map.insert("llm:inference",    vec!["llm/complete"]);
        map.insert("llm:image",        vec!["llm/generate-image"]);
        map.insert("llm:speech",       vec!["llm/generate-speech"]);
        map.insert("llm:transcription",vec!["llm/transcribe"]);
        map.insert("llm:embedding",    vec!["llm/embed"]);

        Self {
            map,
            always: vec!["cap/request-tool", "cap/escalate", "cap/list", "job/watch"],
        }
    }
}

impl CapabilityToolMap {
    pub fn tools_for_capability(&self, cap: &str) -> &[&'static str] {
        // Strip provider sub-scope: "llm:inference::anthropic" → "llm:inference"
        let base = cap.split("::").next().unwrap_or(cap);
        self.map.get(base).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn always_present(&self) -> &[&'static str] {
        &self.always
    }
}
```

**`src/types/token.rs`** — minimal stubs for now (full implementation on Day 11):

```rust
use super::Role;

/// Stub — full implementation on Day 11.
#[derive(Debug, Clone)]
pub struct ATPToken {
    pub role: Role,
    pub session_id: String,
}

/// Stub — full implementation on Day 11.
#[derive(Debug, Clone)]
pub struct CapabilityToken {
    pub granted_tools: Vec<String>,
    pub signature: String,
}

impl CapabilityToken {
    pub fn has_tool(&self, tool: &str) -> bool {
        self.granted_tools.iter().any(|t| t == tool)
    }
}
```

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: all Day 2 type tests pass (30+ tests)

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings

cargo fmt --check
# Expected: exit 0

# Quick coverage spot-check
cargo tarpaulin --workspace --out Stdout 2>/dev/null | grep "Coverage"
# Expected: >= 80% for avix-core (types are well-covered)
```

---

## Commit

```bash
git add -A
git commit -m "day-02: core types — Pid, IpcAddr, Role, Modality, ToolName, CapabilityToolMap"
```

---

## Success Criteria

- [ ] 30+ type tests pass
- [ ] `IpcAddr` resolves correct OS path on the current platform
- [ ] `Modality::all()` round-trips through `as_str()` / `FromStr`
- [ ] `ToolName::parse` rejects `__` and empty strings
- [ ] Mangle/unmangle is lossless for all test inputs
- [ ] `CapabilityToolMap` covers all five `llm:*` modalities plus `spawn` and `pipe`
- [ ] `always_present()` returns all four always-on tools
- [ ] 0 clippy warnings
