use crate::error::AvixError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolName(String);

impl ToolName {
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

    pub fn mangled(&self) -> String {
        self.0.replace('/', "__")
    }

    pub fn unmangle(mangled: &str) -> Result<Self, AvixError> {
        let unmangled = mangled.replace("__", "/");
        Self::parse(&unmangled)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCategory {
    Direct,
    AvixBehaviour,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolVisibility {
    All,
    Crew(String),
    User(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_tool_name() {
        let t = ToolName::parse("fs/read").unwrap();
        assert_eq!(format!("{t}"), "fs/read");
    }
}
