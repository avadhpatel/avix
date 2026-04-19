use std::path::Path;

use crate::config::users::UsersConfig;
use crate::types::token::CapabilityToken;
use tracing::instrument;

#[derive(Debug, Clone)]
pub struct VfsCallerContext {
    pub username: String,
    pub crews: Vec<String>,
    pub is_admin: bool,
    pub token: Option<CapabilityToken>,
}

impl VfsCallerContext {
    #[instrument]
    pub async fn from_token(
        root: &Path,
        token: &CapabilityToken,
    ) -> Result<Option<Self>, crate::error::AvixError> {
        let users_path = root.join("etc/users.yaml");
        if !users_path.exists() {
            return Ok(None);
        }

        let users_yaml = tokio::fs::read_to_string(&users_path).await?;
        let users: UsersConfig = serde_yaml::from_str(&users_yaml)
            .map_err(|e| crate::error::AvixError::ConfigParse(e.to_string()))?;

        let issued_to = token.issued_to.as_ref().ok_or_else(|| {
            crate::error::AvixError::NotFound("no issued_to in token".to_string())
        })?;

        let user = users.find_user(&issued_to.spawned_by).ok_or_else(|| {
            crate::error::AvixError::NotFound(format!(
                "user '{}' not found in users.yaml",
                issued_to.spawned_by
            ))
        })?;

        Ok(Some(Self {
            username: user.username.clone(),
            crews: user.crews.clone(),
            is_admin: user.is_admin(),
            token: Some(token.clone()),
        }))
    }

    #[instrument]
    pub fn from_user_info(username: String, crews: Vec<String>, is_admin: bool) -> Self {
        Self {
            username,
            crews,
            is_admin,
            token: None,
        }
    }

    #[instrument]
    pub fn anonymous() -> Self {
        Self {
            username: "anonymous".to_string(),
            crews: vec![],
            is_admin: false,
            token: None,
        }
    }
}

#[derive(Debug)]
pub struct VfsPermissions {
    pub owner: String,
    pub crew: String,
    pub all: String,
}

impl Default for VfsPermissions {
    #[instrument]
    fn default() -> Self {
        Self {
            owner: "rwx".to_string(),
            crew: "rw-".to_string(),
            all: "r--".to_string(),
        }
    }
}

impl VfsPermissions {
    #[instrument]
    pub fn for_path(path: &str) -> Self {
        match path {
            p if p.starts_with("/tools/") => Self::default(),
            p if p.starts_with("/users/") => Self::default(),
            p if p.starts_with("/services/") => Self::default(),
            p if p.starts_with("/crews/") => Self::default(),
            p if p.starts_with("/etc/") => Self::default(),
            _ => Self::default(),
        }
    }

    #[instrument]
    pub fn effective_for(&self, caller: &VfsCallerContext) -> String {
        if caller.is_admin {
            return "rwx".to_string();
        }
        if caller.username == self.owner {
            return self.owner.clone();
        }
        if !self.crew.is_empty() && caller.crews.contains(&self.crew) {
            return self.crew.clone();
        }
        self.all.clone()
    }

    #[instrument]
    pub fn can_read(&self, caller: &VfsCallerContext) -> bool {
        let perms = self.effective_for(caller);
        perms.contains('r')
    }

    #[instrument]
    pub fn can_write(&self, caller: &VfsCallerContext) -> bool {
        let perms = self.effective_for(caller);
        perms.contains('w')
    }

    #[instrument]
    pub fn can_execute(&self, caller: &VfsCallerContext) -> bool {
        let perms = self.effective_for(caller);
        perms.contains('x')
    }
}
