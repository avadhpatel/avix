use crate::types::Pid;

use tracing::instrument;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipeDirection {
    /// Source → Target only (default).
    Out,
    /// Target → Source only.
    In,
    /// Both directions; both agents may write and read.
    Bidirectional,
}

impl PipeDirection {
    #[instrument]
    pub fn parse(s: &str) -> Self {
        match s {
            "in" => Self::In,
            "bidirectional" => Self::Bidirectional,
            _ => Self::Out,
        }
    }

    #[instrument]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Out => "out",
            Self::In => "in",
            Self::Bidirectional => "bidirectional",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackpressurePolicy {
    /// Block source until target reads (safe; default).
    Block,
    /// Silently drop excess tokens.
    Drop,
    /// Return `PipeError::Full` to source.
    Error,
}

impl BackpressurePolicy {
    #[instrument]
    pub fn parse(s: &str) -> Self {
        match s {
            "drop" => Self::Drop,
            "error" => Self::Error,
            _ => Self::Block,
        }
    }

    #[instrument]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Drop => "drop",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipeEncoding {
    Text,
    Json,
    Yaml,
}

impl PipeEncoding {
    #[instrument]
    pub fn parse(s: &str) -> Self {
        match s {
            "json" => Self::Json,
            "yaml" => Self::Yaml,
            _ => Self::Text,
        }
    }

    #[instrument]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

/// Configuration for a pipe, capturing both agent endpoints and channel behaviour.
#[derive(Debug, Clone)]
pub struct PipeConfig {
    pub source_pid: Pid,
    pub target_pid: Pid,
    pub direction: PipeDirection,
    pub buffer_tokens: usize,
    pub backpressure: BackpressurePolicy,
    pub encoding: PipeEncoding,
}

impl PipeConfig {
    #[instrument]
    pub fn new(source_pid: Pid, target_pid: Pid) -> Self {
        Self {
            source_pid,
            target_pid,
            direction: PipeDirection::Out,
            buffer_tokens: 8192,
            backpressure: BackpressurePolicy::Block,
            encoding: PipeEncoding::Text,
        }
    }

    #[instrument]
    /// Return true if `pid` may write to this pipe.
    pub fn can_write(&self, pid: Pid) -> bool {
        match self.direction {
            PipeDirection::Out => pid == self.source_pid,
            PipeDirection::In => pid == self.target_pid,
            PipeDirection::Bidirectional => pid == self.source_pid || pid == self.target_pid,
        }
    }

    #[instrument]
    /// Return true if `pid` may read from this pipe.
    pub fn can_read(&self, pid: Pid) -> bool {
        match self.direction {
            PipeDirection::Out => pid == self.target_pid,
            PipeDirection::In => pid == self.source_pid,
            PipeDirection::Bidirectional => pid == self.source_pid || pid == self.target_pid,
        }
    }

    #[instrument]
    /// Return the partner PID for SIGPIPE delivery when `closer_pid` closes the pipe.
    pub fn partner(&self, closer_pid: Pid) -> Option<Pid> {
        if closer_pid == self.source_pid {
            Some(self.target_pid)
        } else if closer_pid == self.target_pid {
            Some(self.source_pid)
        } else {
            None
        }
    }
}
