mod builder;
mod evaluator;
mod orchestrator;
mod scout;
mod text_pruner_mock;
mod traits;
mod transformer;
mod triage;

pub use crate::tools::{BuildCommandDiscoverer, LlmBuildDiscoverer, NoopBuildDiscoverer};
pub use builder::{
    builder_tool_set, default_builder_agent, BuilderAgent, DefaultBuilderAgent,
    BUILDER_SYSTEM_PROMPT,
};
pub use evaluator::{EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT};
pub use orchestrator::AgentLoopOrchestrator;
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
