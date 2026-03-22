use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SignalKind {
    Start,
    Pause,
    Resume,
    Kill,
    Stop,
    Save,
    Pipe,
    Escalate,
    /// Agent-defined event, agent → kernel direction.
    Usr1,
    /// Secondary agent-defined event, agent → kernel direction.
    Usr2,
}

impl SignalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "SIGSTART",
            Self::Pause => "SIGPAUSE",
            Self::Resume => "SIGRESUME",
            Self::Kill => "SIGKILL",
            Self::Stop => "SIGSTOP",
            Self::Save => "SIGSAVE",
            Self::Pipe => "SIGPIPE",
            Self::Escalate => "SIGESCALATE",
            Self::Usr1 => "SIGUSR1",
            Self::Usr2 => "SIGUSR2",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub target: crate::types::Pid,
    pub kind: SignalKind,
    pub payload: serde_json::Value,
}
