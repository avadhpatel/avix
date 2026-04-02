use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolPermissions {
    pub owner: String,
    pub crew: String,
    pub all: String,
}

impl Default for ToolPermissions {
    fn default() -> Self {
        Self {
            owner: "root".to_string(),
            crew: String::new(),
            all: "r--".to_string(),
        }
    }
}

impl ToolPermissions {
    pub fn new(owner: String, crew: String, all: String) -> Self {
        Self { owner, crew, all }
    }

    pub fn admin() -> Self {
        Self {
            owner: "root".to_string(),
            crew: String::new(),
            all: "rwx".to_string(),
        }
    }

    pub fn parse_rwx(s: &str) -> bool {
        matches!(s, "r--" | "rw-" | "rwx")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_permissions() {
        let perms = ToolPermissions::default();
        assert_eq!(perms.owner, "root");
        assert_eq!(perms.crew, "");
        assert_eq!(perms.all, "r--");
    }

    #[test]
    fn admin_permissions() {
        let perms = ToolPermissions::admin();
        assert_eq!(perms.all, "rwx");
    }

    #[test]
    fn parse_rwx_valid() {
        assert!(ToolPermissions::parse_rwx("r--"));
        assert!(ToolPermissions::parse_rwx("rw-"));
        assert!(ToolPermissions::parse_rwx("rwx"));
    }

    #[test]
    fn parse_rwx_invalid() {
        assert!(!ToolPermissions::parse_rwx(""));
        assert!(!ToolPermissions::parse_rwx("rwd"));
        assert!(!ToolPermissions::parse_rwx("rwxrwx"));
    }
}
