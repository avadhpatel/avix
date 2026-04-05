use std::collections::HashMap;
use std::sync::Arc;

use tracing::warn;

use super::loader::ManifestLoader;
use super::schema::{AgentManifest, ManifestTools, ModelRequirements};
use crate::error::AvixError;
use crate::memfs::VfsRouter;

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
        // Build permitted set: allowed minus denied.
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

        // Required tools: must all be in permitted.
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

        // Optional tools: silently omit if not permitted.
        for tool in &manifest_tools.optional {
            if permitted.contains(tool.as_str()) && !granted.contains(tool) {
                granted.push(tool.clone());
            }
        }

        // Always-granted built-ins.
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
    ///
    /// v1: checks model name is non-empty and logs a warning if min_context_window > 0.
    /// Full enforcement (actual context window lookup via llm.svc) is deferred.
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
    /// Unknown `{{key}}` tokens that have no corresponding var are left as-is.
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
    loader: ManifestLoader,
}

impl SpawnResolver {
    pub fn new(vfs: Arc<VfsRouter>) -> Self {
        Self {
            loader: ManifestLoader::new(vfs),
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
        let manifest = self.loader.load(agent_name, username).await?;
        ModelValidator::validate(selected_model, &manifest.spec.entrypoint.model_requirements)?;
        let granted_tools =
            ToolGrantResolver::resolve(&manifest.spec.tools, user_permissions, agent_name)?;
        let system_prompt = manifest
            .spec
            .defaults
            .system_prompt
            .clone()
            .unwrap_or_default();
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
        // code/interpreter is optional and not in user perms — absent
        assert!(!granted.contains(&"code/interpreter".to_string()));
        // built-ins always present
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
kind: AgentManifest
metadata:
  name: echo-bot
  version: 1.0.0
  description: Simple echo agent
  author: avix-core
  createdAt: "2026-03-15T10:00:00Z"
  signature: "sha256:"
spec:
  entrypoint:
    type: llm-loop
  defaults:
    systemPrompt: "You are a helpful assistant."
"#;

    async fn vfs_with_manifest(path: &str, yaml: &str) -> Arc<VfsRouter> {
        let vfs = Arc::new(VfsRouter::new());
        let vfs_path = VfsPath::parse(path).unwrap();
        vfs.write(&vfs_path, yaml.as_bytes().to_vec())
            .await
            .unwrap();
        vfs
    }

    // T-MGB-11
    #[tokio::test]
    async fn spawn_resolver_produces_correct_context() {
        let vfs = vfs_with_manifest("/bin/echo-bot@1.0.0/manifest.yaml", ECHO_BOT_YAML).await;
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
kind: AgentManifest
metadata:
  name: researcher
  version: 1.0.0
  description: Researcher
  author: test
  createdAt: "2026-01-01T00:00:00Z"
  signature: "sha256:"
spec:
  defaults:
    systemPrompt: "You are a researcher."
    goalTemplate: "Research: {{topic}}"
"#;
        let vfs = vfs_with_manifest("/bin/researcher@1.0.0/manifest.yaml", yaml).await;
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
