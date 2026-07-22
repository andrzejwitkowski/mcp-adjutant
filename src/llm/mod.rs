mod factory;
mod openai_compatible;
mod preflight;
mod request;
mod tools;
mod traits;
mod types;

pub use factory::{
    create_babysitter_llm_client, create_builder_llm_client, create_evaluator_llm_client,
    create_git_janitor_llm_client, create_llm_client, create_llm_client_for_phase,
    create_log_analyzer_llm_client, create_planner_emit_llm_client, create_planner_llm_client,
    create_scout_llm_client, create_transformer_llm_client, create_triage_llm_client,
    create_web_fetcher_llm_client, ConfiguredLlmClient,
};
pub use openai_compatible::OpenAiCompatibleClient;
pub use preflight::{
    config_checksum, preflight_config, preflight_phase, skip_preflight, PreflightReport,
};
pub use request::LlmRequest;
pub use tools::{
    required_str, LlmTool, LlmToolSet, ParamType, ToolDefinition, ToolInvocationResult, ToolParam,
};
pub use traits::LlmClient;
pub use types::{LlmModelTurn, LlmToolCall, LlmUsage};
