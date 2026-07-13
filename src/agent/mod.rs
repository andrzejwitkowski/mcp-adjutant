mod babysitter;
mod builder;
mod evaluator;
mod log_analyzer;
mod orchestrator;
mod report;
mod scout;
mod text_pruner_mock;
mod traits;
mod transformer;
mod transpiler;
mod triage;
mod web_fetcher;

pub use crate::tools::{BuildCommandDiscoverer, LlmBuildDiscoverer, NoopBuildDiscoverer};
pub use babysitter::{
    babysitter_tool_set, parse_log_path, parse_report_body, parse_triage_arguments,
    BabysitterAgent, BABYSITTER_MAX_ITERATIONS, BABYSITTER_SYSTEM_PROMPT,
};
pub use builder::{
    builder_tool_set, default_builder_agent, BuilderAgent, DefaultBuilderAgent,
    BUILDER_SYSTEM_PROMPT,
};
pub use evaluator::{EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT};
pub use log_analyzer::{
    analyze_log_at_path, llm_payload_to_core, LogAnalyzerAgent, LOG_ANALYZER_SYSTEM_PROMPT,
};
pub use orchestrator::{build_tool_loop_message, run_single_tool_turn, AgentLoopOrchestrator};
pub use report::{format_triage_success, triage_passed, TRIAGE_PASS_MARKER};
pub use scout::{
    run_scout_with_cache, scout_tool_set, ScoutAgent, ScoutCacheOutcome, ScoutModelTurn,
    ScoutToolCall, SCOUT_SYSTEM_PROMPT,
};
pub use text_pruner_mock::TextPrunerMock;
pub use traits::{AgentContext, AutonomousAgent};
pub use transformer::{
    default_transformer_agent, filter_targets_by_scope, find_refactor_targets, path_under_scope,
    transformer_tool_set, DefaultTransformerAgent, TransformerAgent, TRANSFORMER_MAX_ITERATIONS,
    TRANSFORMER_SYSTEM_PROMPT,
};
pub use transpiler::{
    default_verify_workspace, embed_source_files, parse_report_reason,
    parse_transpile_types_args, parse_triage_arguments as parse_transpiler_triage_arguments,
    parse_write_arguments, transpiler_tool_set, TranspileTypesArgs, TranspilerAgent,
    TRANSPILER_MAX_ITERATIONS, TRANSPILER_SYSTEM_PROMPT,
};
pub use triage::{
    triage_tool_set, BuildCommandRunner, SystemBuildRunner, TriageAgent, TRIAGE_SYSTEM_PROMPT,
};
pub use web_fetcher::{
    run_web_fetch_with_cache, WebCacheOutcome, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT,
};
