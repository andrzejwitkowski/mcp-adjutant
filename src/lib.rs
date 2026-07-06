pub mod agent;
pub mod cache;
pub mod domain;
pub mod error;
pub mod llm;
pub mod storage;
pub mod tools;

pub use tools::{
    detect_file_language, detect_project_languages, language_from_extension, AstUsageFinder,
    FileLanguageReport, ProjectLanguageReport, SourceLanguage,
};

pub use agent::{
    scout_tool_definitions, AgentContext, AgentLoopOrchestrator, AutonomousAgent, ScoutAgent,
    ScoutModelTurn, ScoutToolCall, TextPrunerMock, SCOUT_SYSTEM_PROMPT,
};
pub use cache::{LocalEmbeddingEngine, ProjectCacheManager, SEMANTIC_SIMILARITY_THRESHOLD};
pub use domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider};
pub use error::AdjutantConfigError;
pub use llm::{DeepSeekClient, LlmClient, LlmModelTurn, LlmToolCall};
