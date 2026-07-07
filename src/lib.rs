pub mod agent;
pub mod cache;
pub mod domain;
pub mod error;
pub mod llm;
pub mod mcp;
pub mod storage;
pub mod tools;

pub use tools::{
    detect_file_language, detect_project_languages, edit_file_line, find_nearest_module_boundary,
    get_dirty_files_from_git, language_from_extension, run_build_command, AstUsageFinder,
    FileLanguageReport, ProjectLanguageReport, SourceLanguage,
};

pub use agent::{
    scout_tool_set, AgentContext, AgentLoopOrchestrator, AutonomousAgent, BuildCommandRunner,
    ScoutAgent, ScoutModelTurn, ScoutToolCall, SystemBuildRunner, TextPrunerMock, TriageAgent,
    SCOUT_SYSTEM_PROMPT, TRIAGE_SYSTEM_PROMPT,
};
pub use cache::{LocalEmbeddingEngine, ProjectCacheManager, SEMANTIC_SIMILARITY_THRESHOLD};
pub use domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider};
pub use error::AdjutantConfigError;
pub use llm::{
    DeepSeekClient, LlmClient, LlmModelTurn, LlmRequest, LlmTool, LlmToolCall, LlmToolSet,
    ParamType, ToolDefinition, ToolInvocationResult, ToolParam,
};
pub use mcp::{
    handle_verify_and_triage, registered_mcp_tools, verify_and_triage_schema,
    VERIFY_AND_TRIAGE_TOOL_NAME,
};
