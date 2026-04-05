pub mod git_fetch;
pub mod installer;
pub mod loader;
pub mod resolver;
pub mod scanner;
pub mod schema;

pub use installer::{AgentInstallRequest, AgentInstallResult, AgentInstaller, InstallScope};
pub use loader::ManifestLoader;
pub use resolver::{
    GoalRenderer, ModelValidator, ResolvedSpawnContext, SpawnResolver, ToolGrantResolver,
    UserToolPermissions,
};
pub use scanner::{AgentManifestSummary, AgentScope, ManifestScanner};
pub use schema::{
    AgentManifest, AgentSpec, EntrypointType, ManifestDefaults, ManifestEntrypoint,
    ManifestEnvironment, ManifestMemory, ManifestMetadata, ManifestSnapshot, ManifestTools,
    ModelRequirements, PackagingMetadata, SemanticStoreAccess, SnapshotMode, WorkingContextMode,
};
