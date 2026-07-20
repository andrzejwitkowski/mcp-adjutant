use crate::domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider};
use crate::metrics::record_llm_call;

use super::deepseek::DeepSeekClient;
use super::request::LlmRequest;
use super::traits::LlmClient;
use super::types::LlmModelTurn;

pub(crate) struct InstrumentedLlmClient {
    inner: DeepSeekClient,
    phase: AgentPhase,
    model_name: String,
}

impl LlmClient for InstrumentedLlmClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        let turn = self.inner.complete(request)?;
        let usage = turn.usage.unwrap_or_default();
        record_llm_call(self.phase, &self.model_name, usage);
        Ok(turn)
    }
}

pub struct ConfiguredLlmClient(Box<InstrumentedLlmClient>);

impl LlmClient for ConfiguredLlmClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        self.0.complete(request)
    }
}

pub fn create_llm_client(
    profile: PhaseProfile,
    phase: AgentPhase,
) -> Result<ConfiguredLlmClient, String> {
    match profile.provider {
        Provider::DeepSeek | Provider::OpenRouter | Provider::OpenAI | Provider::Custom => {
            let model_name = profile.model_name.clone();
            Ok(ConfiguredLlmClient(Box::new(InstrumentedLlmClient {
                inner: DeepSeekClient::new(profile),
                phase,
                model_name,
            })))
        }
    }
}

pub fn create_llm_client_for_phase(
    config: &AdjutantConfig,
    phase: AgentPhase,
) -> Result<ConfiguredLlmClient, String> {
    let profile = match config.try_get_profile(phase) {
        Ok(profile) => profile.clone(),
        Err(_) => {
            let mut merged = config.clone();
            merged.merge_missing_from_defaults();
            merged.try_get_profile(phase)?.clone()
        }
    };
    create_llm_client(profile, phase)
}

pub fn create_triage_llm_client(config: &AdjutantConfig) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::Triage)
}

pub fn create_scout_llm_client(config: &AdjutantConfig) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::Scout)
}

pub fn create_builder_llm_client(config: &AdjutantConfig) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::Builder)
}

pub fn create_evaluator_llm_client(config: &AdjutantConfig) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::Evaluator)
}

pub fn create_log_analyzer_llm_client(
    config: &AdjutantConfig,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::LogAnalyzer)
}

pub fn create_transformer_llm_client(
    config: &AdjutantConfig,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::Transformer)
}

pub fn create_web_fetcher_llm_client(
    config: &AdjutantConfig,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::WebFetcher)
}

pub fn create_babysitter_llm_client(
    config: &AdjutantConfig,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::Babysitter)
}

pub fn create_planner_llm_client(config: &AdjutantConfig) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::Planner)
}

pub fn create_planner_emit_llm_client(
    config: &AdjutantConfig,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::PlannerEmit)
}

pub fn create_git_janitor_llm_client(
    config: &AdjutantConfig,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::GitJanitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Provider;
    use std::collections::HashMap;

    #[test]
    fn create_triage_llm_client_uses_triage_phase_profile() {
        let config = AdjutantConfig::default();
        let profile = config.get_profile(&AgentPhase::Triage);

        create_triage_llm_client(&config).expect("triage client");
        assert_eq!(profile.provider, Provider::DeepSeek);
        assert_eq!(profile.model_name, "deepseek-coder");
    }

    #[test]
    fn create_scout_llm_client_uses_scout_phase_profile() {
        let config = AdjutantConfig::default();
        let profile = config.get_profile(&AgentPhase::Scout);

        create_scout_llm_client(&config).expect("scout client");
        assert_eq!(profile.provider, Provider::DeepSeek);
        assert_eq!(profile.model_name, "deepseek-chat");
    }

    #[test]
    fn create_transformer_llm_client_uses_transformer_phase_profile() {
        let mut config = AdjutantConfig::default();
        config.phases.insert(
            AgentPhase::Transformer,
            PhaseProfile {
                provider: Provider::OpenRouter,
                api_key: Some("sk-test".to_string()),
                base_url: "https://openrouter.ai/api/v1".to_string(),
                model_name: "google/gemini-2.5-flash".to_string(),
                max_tokens: 4_096,
                temperature: 0.1,
            },
        );

        create_transformer_llm_client(&config).expect("transformer client");
        assert_eq!(
            config.get_profile(&AgentPhase::Transformer).model_name,
            "google/gemini-2.5-flash"
        );
    }

    #[test]
    fn create_triage_llm_client_merges_missing_phases() {
        let config = AdjutantConfig {
            phases: HashMap::new(),
            ..Default::default()
        };

        create_triage_llm_client(&config).expect("merge backfills default triage profile");
    }

    #[test]
    fn create_web_fetcher_llm_client_uses_web_fetcher_phase_profile() {
        let config = AdjutantConfig::default();

        create_web_fetcher_llm_client(&config).expect("web fetcher client");

        let profile = config.get_profile(&AgentPhase::WebFetcher);
        assert_eq!(profile.model_name, "deepseek-chat");
    }

    #[test]
    fn create_log_analyzer_llm_client_uses_log_analyzer_phase_profile() {
        let config = AdjutantConfig::default();
        create_log_analyzer_llm_client(&config).expect("log analyzer client");
        assert_eq!(
            config.get_profile(&AgentPhase::LogAnalyzer).model_name,
            "deepseek-chat"
        );
    }
}
