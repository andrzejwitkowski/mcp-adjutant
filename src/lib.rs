pub mod agent;
pub mod cache;
pub mod config_server;
pub mod domain;
pub mod error;
pub mod llm;
pub mod mcp;
pub mod mcp_server;
pub mod storage;
pub mod tools;

pub use tools::{
    detect_file_language, detect_project_languages, edit_file_line, find_nearest_module_boundary,
    get_dirty_files_from_git, language_from_extension, run_build_command, AstUsageFinder,
    FileLanguageReport, ProjectLanguageReport, SourceLanguage,
};

pub use agent::{
    builder_tool_set, scout_tool_set, AgentContext, AgentLoopOrchestrator, AutonomousAgent,
    BuildCommandRunner, BuilderAgent, DefaultBuilderAgent, ScoutAgent, ScoutModelTurn,
    ScoutToolCall, SystemBuildRunner, TextPrunerMock, TriageAgent, BUILDER_SYSTEM_PROMPT,
    SCOUT_SYSTEM_PROMPT, TRIAGE_SYSTEM_PROMPT,
};
pub use cache::{LocalEmbeddingEngine, ProjectCacheManager, SEMANTIC_SIMILARITY_THRESHOLD};
pub use domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider};
pub use error::AdjutantConfigError;
pub use llm::{
    create_llm_client, create_llm_client_for_phase, create_scout_llm_client,
    create_triage_llm_client, ConfiguredLlmClient, DeepSeekClient, LlmClient, LlmModelTurn,
    LlmRequest, LlmTool, LlmToolCall, LlmToolSet, ParamType, ToolDefinition, ToolInvocationResult,
    ToolParam,
};
pub use mcp::{
    generate_tests_and_scaffolding_schema, handle_generate_tests_and_scaffolding,
    handle_scout_context, handle_verify_and_triage, registered_mcp_tools, scout_context_schema,
    verify_and_triage_schema, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME, SCOUT_CONTEXT_TOOL_NAME,
    VERIFY_AND_TRIAGE_TOOL_NAME,
};
