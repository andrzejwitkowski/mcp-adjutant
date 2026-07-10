pub mod agent;
pub mod cache;
pub mod config_server;
pub mod domain;
pub mod error;
pub mod jobs;
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
    builder_tool_set, default_transformer_agent, find_refactor_targets, path_under_scope,
    scout_tool_set, transformer_tool_set, AgentContext, AgentLoopOrchestrator, AutonomousAgent,
    BuildCommandRunner, BuilderAgent, DefaultBuilderAgent, DefaultTransformerAgent,
    EvaluatorAgent, ScoutAgent, ScoutModelTurn, ScoutToolCall, SystemBuildRunner,
    TextPrunerMock, TransformerAgent, TriageAgent, BUILDER_SYSTEM_PROMPT,
    EVALUATOR_SYSTEM_PROMPT, SCOUT_SYSTEM_PROMPT, TRANSFORMER_MAX_ITERATIONS,
    TRANSFORMER_SYSTEM_PROMPT, TRIAGE_SYSTEM_PROMPT,
};
pub use cache::{
    LocalEmbeddingEngine, ProjectCacheManager, EMBEDDING_DIM, SEMANTIC_SIMILARITY_THRESHOLD,
};
pub use domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider};
pub use error::AdjutantConfigError;
pub use jobs::{query_job_status_schema, JobRegistry, QUERY_JOB_STATUS_TOOL_NAME};
pub use llm::{
    create_builder_llm_client, create_evaluator_llm_client, create_llm_client,
    create_llm_client_for_phase, create_scout_llm_client, create_transformer_llm_client,
    create_triage_llm_client, ConfiguredLlmClient, DeepSeekClient, LlmClient, LlmModelTurn, LlmRequest, LlmTool, LlmToolCall,
    LlmToolSet, ParamType, ToolDefinition, ToolInvocationResult, ToolParam,
};
pub use mcp::{
    evaluate_agent_performance_schema, execute_global_refactor_schema,
    generate_tests_and_scaffolding_schema, handle_evaluate_agent_performance,
    handle_execute_global_refactor, handle_generate_tests_and_scaffolding,
    handle_query_job_status, handle_scout_context, handle_verify_and_triage, registered_mcp_tools,
    scout_context_schema, verify_and_triage_schema, EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
    EXECUTE_GLOBAL_REFACTOR_TOOL_NAME, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
    SCOUT_CONTEXT_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
};
