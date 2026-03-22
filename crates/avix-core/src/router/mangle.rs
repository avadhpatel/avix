/// Wire-format tool name mangling (ADR-03).
///
/// Avix tool names use `/` as namespace separator (`fs/read`).
/// Provider adapters transmit `__` on the wire (`fs__read`).
/// No Avix tool name ever contains `__` — it is reserved for wire mangling only.
use crate::error::AvixError;

/// Mangle an Avix tool name for the wire: replace `/` with `__`.
///
/// `"fs/read"` → `"fs__read"`
pub fn mangle(name: &str) -> String {
    name.replace('/', "__")
}

/// Unmangle a wire tool name back to the Avix form: replace `__` with `/`.
///
/// `"fs__read"` → `"fs/read"`
pub fn unmangle(name: &str) -> String {
    name.replace("__", "/")
}

/// Validate that a tool name is an Avix-internal name (no `__`).
///
/// Returns `Err` if the name contains `__`, which is reserved for the wire format.
pub fn validate_tool_name(name: &str) -> Result<(), AvixError> {
    if name.contains("__") {
        return Err(AvixError::InvalidToolName {
            name: name.to_string(),
            reason: "tool names must not contain '__' (reserved for wire mangling)".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangle_replaces_slash_with_double_underscore() {
        assert_eq!(mangle("fs/read"), "fs__read");
        assert_eq!(mangle("mcp/github/list-prs"), "mcp__github__list-prs");
        assert_eq!(mangle("cap/request-tool"), "cap__request-tool");
    }

    #[test]
    fn unmangle_replaces_double_underscore_with_slash() {
        assert_eq!(unmangle("fs__read"), "fs/read");
        assert_eq!(unmangle("mcp__github__list-prs"), "mcp/github/list-prs");
    }

    #[test]
    fn mangle_unmangle_round_trip() {
        let original = "fs/read";
        assert_eq!(unmangle(&mangle(original)), original);
    }

    #[test]
    fn validate_rejects_wire_names() {
        assert!(validate_tool_name("fs__read").is_err());
        assert!(validate_tool_name("fs/read").is_ok());
        assert!(validate_tool_name("cap/request-tool").is_ok());
    }
}
