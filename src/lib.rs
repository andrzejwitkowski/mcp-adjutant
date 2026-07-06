pub mod agent;
pub mod cache;
pub mod domain;
pub mod error;
pub mod storage;
pub mod tools;

pub use agent::{
    scout_tool_definitions, AgentContext, AgentLoopOrchestrator, AutonomousAgent, ChatClient,
    DeepSeekClient, ScoutAgent, ScoutModelTurn, ScoutToolCall, TextPrunerMock, SCOUT_SYSTEM_PROMPT,
};
pub use cache::{LocalEmbeddingEngine, ProjectCacheManager, SEMANTIC_SIMILARITY_THRESHOLD};
pub use domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider};
pub use error::AdjutantConfigError;
