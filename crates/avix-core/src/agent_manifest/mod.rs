pub mod installer;
pub mod loader;
pub mod manifest_file;
pub mod resolver;
pub mod scanner;
pub mod schema;

pub use installer::{AgentInstallRequest, AgentInstallResult, AgentInstaller, InstallScope};
pub use loader::ManifestLoader;
pub use manifest_file::AgentManifestFile;
pub use resolver::{
    GoalRenderer, ModelValidator, ResolvedSpawnContext, SpawnResolver, ToolGrantResolver,
    UserToolPermissions,
};
pub use scanner::{AgentManifestSummary, AgentScope, ManifestScanner};
pub use schema::{
    AgentManifest, AgentManifestMetadata, AgentManifestSpec, EntrypointType, ManifestDefaults,
    ManifestEntrypoint, ManifestEnvironment, ManifestMemory, ManifestSnapshot, ManifestTools,
    ModelRequirements, SemanticStoreAccess, SnapshotMode, WorkingContextMode,
};
