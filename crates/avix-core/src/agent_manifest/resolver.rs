use std::collections::HashMap;
use std::sync::Arc;

use tracing::warn;

use super::loader::ManifestLoader;
use super::schema::{AgentManifest, ManifestTools, ModelRequirements};
use crate::error::AvixError;
use crate::memfs::{VfsPath, VfsRouter};

// ── User permissions ──────────────────────────────────────────────────────────

/// A user's tool permission set, derived from crew + user ACL at spawn time.
pub struct UserToolPermissions {
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
}

// ── Tool grant resolver ───────────────────────────────────────────────────────

pub struct ToolGrantResolver;

impl ToolGrantResolver {
    /// Built-in kernel tools always granted regardless of ACL (ADR-04 Category 2 tools).
    pub const ALWAYS_GRANTED: &'static [&'static str] =
        &["cap/request-tool", "cap/escalate", "cap/list", "job/watch"];

    /// Compute the granted tool list.
    ///
    /// Algorithm:
    ///   permitted = (allowed_tools) - denied_tools
    ///   candidate = manifest.required ∪ manifest.optional
    ///   granted   = intersection(candidate, permitted) + ALWAYS_GRANTED
    ///
    /// Returns `Err(RequiredToolDenied)` if any tool in `manifest.required` is absent.
    pub fn resolve(
        manifest_tools: &ManifestTools,
        user_permissions: &UserToolPermissions,
        agent_name: &str,
    ) -> Result<Vec<String>, AvixError> {
        let permitted: std::collections::HashSet<&str> = user_permissions
            .allowed_tools
            .iter()
            .map(|s| s.as_str())
            .filter(|t| {
                !user_permissions
                    .denied_tools
                    .iter()
                    .any(|d| d.as_str() == *t)
            })
            .collect();

        let mut granted: Vec<String> = Vec::new();

        for tool in &manifest_tools.required {
            if permitted.contains(tool.as_str()) {
                granted.push(tool.clone());
            } else {
                return Err(AvixError::RequiredToolDenied {
                    tool: tool.clone(),
                    agent: agent_name.to_string(),
                });
            }
        }

        for tool in &manifest_tools.optional {
            if permitted.contains(tool.as_str()) && !granted.contains(tool) {
                granted.push(tool.clone());
            }
        }

        for builtin in Self::ALWAYS_GRANTED {
            let s = builtin.to_string();
            if !granted.contains(&s) {
                granted.push(s);
            }
        }

        Ok(granted)
    }
}

// ── Model validator ───────────────────────────────────────────────────────────

pub struct ModelValidator;

impl ModelValidator {
    /// Validate that `selected_model` satisfies `requirements`.
    pub fn validate(
        selected_model: &str,
        requirements: &ModelRequirements,
    ) -> Result<(), AvixError> {
        if selected_model.is_empty() {
            return Err(AvixError::ModelRequirementsNotMet {
                reason: "selected model name is empty".into(),
            });
        }
        if requirements.min_context_window > 0 {
            warn!(
                model = selected_model,
                min_context_window = requirements.min_context_window,
                "model context window validation deferred to llm.svc in v1"
            );
        }
        Ok(())
    }
}

// ── Goal renderer ─────────────────────────────────────────────────────────────

pub struct GoalRenderer;

impl GoalRenderer {
    /// Render `template` by substituting `{{key}}` with values from `vars`.
    pub fn render(template: &str, vars: &HashMap<String, String>) -> String {
        let mut result = template.to_string();
        for (k, v) in vars {
            result = result.replace(&format!("{{{{{}}}}}", k), v);
        }
        result
    }
}

// ── Resolved spawn context ────────────────────────────────────────────────────

/// The fully resolved context ready to hand to `RuntimeExecutor`.
pub struct ResolvedSpawnContext {
    pub manifest: AgentManifest,
    pub selected_model: String,
    pub granted_tools: Vec<String>,
    pub system_prompt: String,
    pub rendered_goal: String,
}

// ── Spawn resolver ────────────────────────────────────────────────────────────

pub struct SpawnResolver {
    vfs: Arc<VfsRouter>,
    loader: ManifestLoader,
}

impl SpawnResolver {
    pub fn new(vfs: Arc<VfsRouter>) -> Self {
        Self {
            loader: ManifestLoader::new(Arc::clone(&vfs)),
            vfs,
        }
    }

    /// Full spawn-time resolution pipeline.
    pub async fn resolve(
        &self,
        agent_name: &str,
        username: &str,
        goal: &str,
        selected_model: &str,
        vars: HashMap<String, String>,
        user_permissions: &UserToolPermissions,
    ) -> Result<ResolvedSpawnContext, AvixError> {
        let (manifest, pkg_dir) = self.loader.load_with_dir(agent_name, username).await?;
        ModelValidator::validate(selected_model, &manifest.spec.entrypoint.model_requirements)?;
        let granted_tools =
            ToolGrantResolver::resolve(&manifest.spec.tools, user_permissions, agent_name)?;
        let system_prompt = self.load_system_prompt(&manifest, &pkg_dir).await;
        let rendered_goal = match &manifest.spec.defaults.goal_template {
            Some(tmpl) => GoalRenderer::render(tmpl, &vars),
            None => goal.to_string(),
        };
        Ok(ResolvedSpawnContext {
            manifest,
            selected_model: selected_model.to_string(),
            granted_tools,
            system_prompt,
            rendered_goal,
        })
    }

    /// Read the system prompt file from the VFS package directory.
    /// Returns an empty string if no `systemPromptPath` is set or the file is missing.
    async fn load_system_prompt(&self, manifest: &AgentManifest, pkg_dir: &str) -> String {
        let Some(ref rel_path) = manifest.spec.system_prompt_path else {
            return String::new();
        };
        let full_path = format!("{}/{}", pkg_dir, rel_path);
        let Ok(vfs_path) = VfsPath::parse(&full_path) else {
            warn!(path = full_path, "invalid system prompt path");
            return String::new();
        };
        match self.vfs.read(&vfs_path).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => {
                warn!(path = full_path, "system prompt file not found in VFS");
                String::new()
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memfs::VfsPath;

    fn make_perms(allowed: &[&str], denied: &[&str]) -> UserToolPermissions {
        UserToolPermissions {
            allowed_tools: allowed.iter().map(|s| s.to_string()).collect(),
            denied_tools: denied.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_tools(required: &[&str], optional: &[&str]) -> ManifestTools {
        ManifestTools {
            required: required.iter().map(|s| s.to_string()).collect(),
            optional: optional.iter().map(|s| s.to_string()).collect(),
        }
    }

    // T-MGB-01
    #[test]
    fn resolver_grants_intersection() {
        let tools = make_tools(&["fs/read", "web/search"], &["code/interpreter"]);
        let perms = make_perms(&["fs/read", "web/search", "web/fetch"], &[]);
        let granted = ToolGrantResolver::resolve(&tools, &perms, "researcher").unwrap();
        assert!(granted.contains(&"fs/read".to_string()));
        assert!(granted.contains(&"web/search".to_string()));
        assert!(!granted.contains(&"code/interpreter".to_string()));
        assert!(granted.contains(&"cap/list".to_string()));
    }

    // T-MGB-02
    #[test]
    fn resolver_fails_when_required_tool_denied() {
        let tools = make_tools(&["fs/read", "bash"], &[]);
        let perms = make_perms(&["fs/read"], &[]);
        let result = ToolGrantResolver::resolve(&tools, &perms, "coder");
        assert!(matches!(
            result,
            Err(AvixError::RequiredToolDenied { tool, .. }) if tool == "bash"
        ));
    }

    // T-MGB-03
    #[test]
    fn resolver_always_grants_built_ins() {
        let tools = make_tools(&[], &[]);
        let perms = make_perms(&[], &[]);
        let granted = ToolGrantResolver::resolve(&tools, &perms, "minimal").unwrap();
        for builtin in ToolGrantResolver::ALWAYS_GRANTED {
            assert!(
                granted.contains(&builtin.to_string()),
                "missing builtin: {}",
                builtin
            );
        }
    }

    // T-MGB-04
    #[test]
    fn user_denied_tools_override_allowed() {
        let tools = make_tools(&[], &["fs/write"]);
        let perms = make_perms(&["fs/write"], &["fs/write"]);
        let granted = ToolGrantResolver::resolve(&tools, &perms, "agent").unwrap();
        assert!(!granted.contains(&"fs/write".to_string()));
    }

    // T-MGB-05
    #[test]
    fn goal_renderer_substitutes_vars() {
        let vars = HashMap::from([
            ("topic".into(), "quantum computing".into()),
            ("format".into(), "markdown".into()),
        ]);
        let rendered = GoalRenderer::render("Research: {{topic}}. Format: {{format}}.", &vars);
        assert_eq!(rendered, "Research: quantum computing. Format: markdown.");
    }

    // T-MGB-06
    #[test]
    fn goal_renderer_leaves_unknown_tokens() {
        let rendered = GoalRenderer::render("Research: {{topic}}.", &HashMap::new());
        assert_eq!(rendered, "Research: {{topic}}.");
    }

    // T-MGB-07
    #[test]
    fn goal_renderer_empty_template() {
        let rendered = GoalRenderer::render("", &HashMap::new());
        assert_eq!(rendered, "");
    }

    const ECHO_BOT_YAML: &str = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: echo-bot
  version: 1.0.0
  description: Simple echo agent
  author: avix-core
packaging:
  signature: "sha256:"
spec:
  systemPromptPath: system-prompt.md
  entrypoint:
    type: llm-loop
"#;

    async fn vfs_with_files(files: &[(&str, &str)]) -> Arc<VfsRouter> {
        let vfs = Arc::new(VfsRouter::new());
        for (path, content) in files {
            let vfs_path = VfsPath::parse(path).unwrap();
            vfs.write(&vfs_path, content.as_bytes().to_vec())
                .await
                .unwrap();
        }
        vfs
    }

    // T-MGB-11
    #[tokio::test]
    async fn spawn_resolver_produces_correct_context() {
        let vfs = vfs_with_files(&[
            ("/bin/echo-bot@1.0.0/manifest.yaml", ECHO_BOT_YAML),
            (
                "/bin/echo-bot@1.0.0/system-prompt.md",
                "You are a helpful assistant.",
            ),
        ])
        .await;
        let resolver = SpawnResolver::new(vfs);
        let perms = make_perms(&["fs/read"], &[]);
        let ctx = resolver
            .resolve(
                "echo-bot",
                "alice",
                "echo hello",
                "claude-sonnet-4",
                HashMap::new(),
                &perms,
            )
            .await
            .unwrap();
        assert_eq!(ctx.manifest.metadata.name, "echo-bot");
        assert_eq!(ctx.selected_model, "claude-sonnet-4");
        assert!(ctx.granted_tools.contains(&"cap/list".to_string()));
        assert_eq!(ctx.rendered_goal, "echo hello");
        assert_eq!(ctx.system_prompt, "You are a helpful assistant.");
    }

    #[tokio::test]
    async fn spawn_resolver_renders_goal_template() {
        let yaml = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: researcher
  version: 1.0.0
packaging:
  signature: "sha256:"
spec:
  defaults:
    goalTemplate: "Research: {{topic}}"
"#;
        let vfs = vfs_with_files(&[("/bin/researcher@1.0.0/manifest.yaml", yaml)]).await;
        let resolver = SpawnResolver::new(vfs);
        let perms = make_perms(&[], &[]);
        let vars = HashMap::from([("topic".into(), "quantum computing".into())]);
        let ctx = resolver
            .resolve(
                "researcher",
                "alice",
                "ignored",
                "claude-sonnet-4",
                vars,
                &perms,
            )
            .await
            .unwrap();
        assert_eq!(ctx.rendered_goal, "Research: quantum computing");
    }

    #[tokio::test]
    async fn spawn_resolver_empty_system_prompt_when_no_path() {
        let yaml = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: minimal
  version: 1.0.0
packaging:
  signature: "sha256:"
spec: {}
"#;
        let vfs = vfs_with_files(&[("/bin/minimal@1.0.0/manifest.yaml", yaml)]).await;
        let resolver = SpawnResolver::new(vfs);
        let ctx = resolver
            .resolve(
                "minimal",
                "alice",
                "do something",
                "claude-sonnet-4",
                HashMap::new(),
                &make_perms(&[], &[]),
            )
            .await
            .unwrap();
        assert_eq!(ctx.system_prompt, "");
    }

    #[test]
    fn model_validator_rejects_empty_model() {
        let reqs = ModelRequirements::default();
        let result = ModelValidator::validate("", &reqs);
        assert!(matches!(
            result,
            Err(AvixError::ModelRequirementsNotMet { .. })
        ));
    }

    #[test]
    fn model_validator_accepts_valid_model() {
        let reqs = ModelRequirements::default();
        assert!(ModelValidator::validate("claude-sonnet-4", &reqs).is_ok());
    }
}
