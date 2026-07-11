mod builder;
mod evaluator;
mod orchestrator;
mod report;
mod scout;
mod text_pruner_mock;
mod traits;
mod transformer;
mod triage;
mod web_fetcher;

pub use crate::tools::{BuildCommandDiscoverer, LlmBuildDiscoverer, NoopBuildDiscoverer};
pub use builder::{
    builder_tool_set, default_builder_agent, BuilderAgent, DefaultBuilderAgent,
    BUILDER_SYSTEM_PROMPT,
};
pub use evaluator::{EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT};
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
pub use triage::{
    triage_tool_set, BuildCommandRunner, SystemBuildRunner, TriageAgent, TRIAGE_SYSTEM_PROMPT,
};
pub use web_fetcher::{
    run_web_fetch_with_cache, WebCacheOutcome, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT,
};
