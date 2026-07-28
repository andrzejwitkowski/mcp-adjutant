mod babysitter;
mod builder;
mod builder_prompt;
mod evaluator;
pub mod git_janitor;
mod log_analyzer;
mod orchestrator;
mod planner;
mod pruner;
mod read_only_tools;
mod report;
mod scout;
mod traits;
mod transformer;
mod transpiler;
mod triage;
mod web_fetcher;

pub use crate::tools::{BuildCommandDiscoverer, LlmBuildDiscoverer, NoopBuildDiscoverer};
pub use babysitter::{
    babysitter_tool_set, format_babysitter_result, parse_finalize_arguments, parse_log_path,
    parse_report_body, parse_triage_arguments, BabysitterAgent, BABYSITTER_MAX_ITERATIONS,
    BABYSITTER_SYSTEM_PROMPT,
};
pub use builder::{
    builder_tool_set, default_builder_agent, BuilderAgent, DefaultBuilderAgent,
    BUILDER_SYSTEM_PROMPT,
};
pub use builder_prompt::{
    builder_task_parts, source_file_from_builder_prompt, validate_test_path_for_source,
};
pub use evaluator::{
    format_eval_job_appendix, AgentEvalSummary, EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT,
};
pub use git_janitor::{
    create_git_branch, default_workspace_root, format_scout_block, gather_conventions_and_diff,
    run_git_janitor, GitJanitorAgent, ScoutInputs, GIT_JANITOR_MAX_ITERATIONS,
    GIT_JANITOR_SYSTEM_PROMPT,
};
pub use log_analyzer::{
    analyze_log_at_path, llm_payload_to_core, LogAnalyzerAgent, LOG_ANALYZER_SYSTEM_PROMPT,
};
pub use orchestrator::{build_tool_loop_message, run_single_tool_turn, AgentLoopOrchestrator};
pub use planner::{
    apply_blueprint_file_steps, apply_hunks_to_body, display_rel, extract_json_object,
    format_emit_prompt, format_scout_prompt, parse_plan_blueprint_args, path_line_from_goal,
    planner_emit_tool_set, planner_scout_tool_set, run_planner_hybrid, source_under_test_from_goal,
    test_type_for_target, validate_blueprint, validate_blueprint_coordinator,
    validate_blueprint_grounding, CoordinatorConstraints, PlanBlueprintArgs, PlanKind,
    PlannerAgent, PlannerHybridAgent, PLANNER_EMIT_MAX_ITERATIONS, PLANNER_EMIT_SYSTEM_PROMPT,
    PLANNER_MAX_ITERATIONS, PLANNER_SCOUT_MAX_ITERATIONS, PLANNER_SCOUT_SYSTEM_PROMPT,
    PLANNER_SYSTEM_PROMPT,
};
pub use report::{
    format_builder_report, format_triage_success, triage_passed, BuilderReportInput,
    BUILDER_GREEN_MARKER, TRIAGE_PASS_MARKER,
};
pub use pruner::{
    compact_text, estimate_tokens, is_context_overflow_err, rewrite_context_for_window,
    tokens_over_threshold, with_auto_compact_async, AutoCompactGuard, CompactMode,
    COMPACT_CONTEXT_TOOL_NAME, PRUNER_SYSTEM_PROMPT,
};
pub use scout::{
    run_scout_with_cache, scout_tool_set, ScoutAgent, ScoutCacheOutcome, ScoutModelTurn,
    ScoutToolCall, SCOUT_SYSTEM_PROMPT,
};
pub use traits::{AgentContext, AutonomousAgent};
pub use transformer::{
    default_transformer_agent, filter_targets_by_scope, find_refactor_targets, path_under_scope,
    transformer_tool_set, DefaultTransformerAgent, TransformerAgent, TRANSFORMER_MAX_ITERATIONS,
    TRANSFORMER_SYSTEM_PROMPT,
};
pub use transpiler::{
    default_verify_workspace, embed_source_files, parse_report_reason, parse_transpile_types_args,
    parse_triage_arguments as parse_transpiler_triage_arguments, parse_write_arguments,
    transpiler_tool_set, TranspileTypesArgs, TranspilerAgent, TRANSPILER_MAX_ITERATIONS,
    TRANSPILER_SYSTEM_PROMPT,
};
pub use triage::{
    triage_tool_set, BuildCommandRunner, SystemBuildRunner, TriageAgent, TRIAGE_SYSTEM_PROMPT,
};
pub use web_fetcher::{
    run_web_fetch_with_cache, WebCacheOutcome, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT,
};
