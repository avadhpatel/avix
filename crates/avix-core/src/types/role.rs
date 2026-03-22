use crate::error::AvixError;
use std::str::FromStr;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Guest = 0,
    User = 1,
    Operator = 2,
    Admin = 3,
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

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::User => "user",
            Role::Guest => "guest",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_all_variants() {
        assert_eq!(format!("{}", Role::Admin), "admin");
        assert_eq!(format!("{}", Role::Operator), "operator");
        assert_eq!(format!("{}", Role::User), "user");
        assert_eq!(format!("{}", Role::Guest), "guest");
    }

    #[test]
    fn test_can_access_domain_unknown_returns_false() {
        assert!(!Role::Admin.can_access_domain("unknown-domain"));
    }
}

impl FromStr for Role {
    type Err = AvixError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Role::Admin),
            "operator" => Ok(Role::Operator),
            "user" => Ok(Role::User),
            "guest" => Ok(Role::Guest),
            other => Err(AvixError::ConfigParse(format!("unknown role: {other}"))),
        }
    }
}
