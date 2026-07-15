pub mod agent;
pub mod cache;
pub mod config_server;
pub mod domain;
pub mod error;
pub mod jobs;
pub mod llm;
pub mod mcp;
pub mod mcp_server;
pub mod metrics;
pub mod storage;
pub mod tools;

pub use tools::{
    detect_file_language, detect_project_languages, edit_file_line, find_nearest_module_boundary,
    get_dirty_files_from_git, language_from_extension, run_build_command, AstUsageFinder,
    FileLanguageReport, ProjectLanguageReport, SourceLanguage,
};

pub use agent::{
    babysitter_tool_set, builder_tool_set, default_transformer_agent, find_refactor_targets,
    llm_payload_to_core, path_under_scope, scout_tool_set, transformer_tool_set,
    validate_blueprint, validate_blueprint_coordinator, validate_blueprint_grounding, AgentContext,
    AgentLoopOrchestrator, AutonomousAgent, BabysitterAgent, BuildCommandRunner, BuilderAgent,
    CoordinatorConstraints, DefaultBuilderAgent, DefaultTransformerAgent, EvaluatorAgent,
    LogAnalyzerAgent, PlanBlueprintArgs, PlanKind, PlannerAgent, ScoutAgent, ScoutModelTurn,
    ScoutToolCall, SystemBuildRunner, TextPrunerMock, TransformerAgent, TranspilerAgent,
    TriageAgent, WebFetcherAgent, BABYSITTER_MAX_ITERATIONS, BABYSITTER_SYSTEM_PROMPT,
    BUILDER_SYSTEM_PROMPT, EVALUATOR_SYSTEM_PROMPT, LOG_ANALYZER_SYSTEM_PROMPT,
    PLANNER_MAX_ITERATIONS, PLANNER_SYSTEM_PROMPT, SCOUT_SYSTEM_PROMPT, TRANSFORMER_MAX_ITERATIONS,
    TRANSFORMER_SYSTEM_PROMPT, TRANSPILER_MAX_ITERATIONS, TRANSPILER_SYSTEM_PROMPT,
    TRIAGE_SYSTEM_PROMPT, WEB_FETCHER_SYSTEM_PROMPT,
};
pub use cache::{
    resolve_workspace_path_bounded, LocalEmbeddingEngine, ProjectCacheManager, EMBEDDING_DIM,
    SEMANTIC_SIMILARITY_THRESHOLD,
};
pub use domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider, WebFetcherProfile};
pub use error::AdjutantConfigError;
pub use jobs::{query_job_status_schema, JobRegistry, QUERY_JOB_STATUS_TOOL_NAME};
pub use llm::{
    create_babysitter_llm_client, create_builder_llm_client, create_evaluator_llm_client,
    create_llm_client, create_llm_client_for_phase, create_log_analyzer_llm_client,
    create_scout_llm_client, create_transformer_llm_client, create_triage_llm_client,
    create_web_fetcher_llm_client, ConfiguredLlmClient, DeepSeekClient, LlmClient, LlmModelTurn,
    LlmRequest, LlmTool, LlmToolCall, LlmToolSet, LlmUsage, ParamType, ToolDefinition,
    ToolInvocationResult, ToolParam,
};
pub use mcp::{
    analyze_log_schema, babysit_pr_schema, evaluate_agent_performance_schema,
    execute_global_refactor_schema, generate_tests_and_scaffolding_schema, handle_analyze_log,
    handle_babysit_pr, handle_evaluate_agent_performance, handle_execute_global_refactor,
    handle_generate_tests_and_scaffolding, handle_query_job_status, handle_scout_context,
    handle_transpile_types, handle_verify_and_triage, handle_web_fetch, registered_mcp_tools,
    scout_context_schema, transpile_types_schema, verify_and_triage_schema, web_fetch_schema,
    ANALYZE_LOG_TOOL_NAME, BABYSIT_PR_TOOL_NAME, EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
    EXECUTE_GLOBAL_REFACTOR_TOOL_NAME, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
    SCOUT_CONTEXT_TOOL_NAME, TRANSPILE_TYPES_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
    WEB_FETCH_TOOL_NAME,
};
