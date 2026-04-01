pub mod loader;
pub mod resolver;
pub mod scanner;
pub mod schema;

pub use loader::ManifestLoader;
pub use scanner::{AgentManifestSummary, AgentScope, ManifestScanner};
pub use resolver::{
    GoalRenderer, ModelValidator, ResolvedSpawnContext, SpawnResolver, ToolGrantResolver,
    UserToolPermissions,
};
pub use schema::{
    AgentManifest, AgentManifestMetadata, AgentManifestSpec, EntrypointType, ManifestDefaults,
    ManifestEntrypoint, ManifestEnvironment, ManifestMemory, ManifestSnapshot, ManifestTools,
    ModelRequirements, SemanticStoreAccess, SnapshotMode, WorkingContextMode,
};
