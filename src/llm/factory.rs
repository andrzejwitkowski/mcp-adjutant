use crate::domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider};

use super::deepseek::DeepSeekClient;
use super::request::LlmRequest;
use super::traits::LlmClient;
use super::types::LlmModelTurn;

pub enum ConfiguredLlmClient {
    OpenAiCompatible(DeepSeekClient),
}

impl LlmClient for ConfiguredLlmClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        match self {
            Self::OpenAiCompatible(client) => client.complete(request),
        }
    }
}

pub fn create_llm_client(profile: PhaseProfile) -> Result<ConfiguredLlmClient, String> {
    match profile.provider {
        Provider::DeepSeek | Provider::OpenRouter | Provider::OpenAI | Provider::Custom => {
            // ponytail: one OpenAI-compatible transport; profile selects endpoint/model
            Ok(ConfiguredLlmClient::OpenAiCompatible(DeepSeekClient::new(
                profile,
            )))
        }
    }
}

pub fn create_llm_client_for_phase(
    config: &AdjutantConfig,
    phase: AgentPhase,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client(config.try_get_profile(phase)?.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Provider;
    use std::collections::HashMap;

    #[test]
    fn create_triage_llm_client_uses_triage_phase_profile() {
        let config = AdjutantConfig::default();
        let profile = config.get_profile(&AgentPhase::Triage);

        let client = create_triage_llm_client(&config).expect("triage client");
        assert!(matches!(client, ConfiguredLlmClient::OpenAiCompatible(_)));
        assert_eq!(profile.provider, Provider::DeepSeek);
        assert_eq!(profile.model_name, "deepseek-coder");
    }

    #[test]
    fn create_scout_llm_client_uses_scout_phase_profile() {
        let config = AdjutantConfig::default();
        let profile = config.get_profile(&AgentPhase::Scout);

        let client = create_scout_llm_client(&config).expect("scout client");
        assert!(matches!(client, ConfiguredLlmClient::OpenAiCompatible(_)));
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
    fn create_triage_llm_client_missing_phase_returns_error() {
        let config = AdjutantConfig {
            phases: HashMap::new(),
            ..Default::default()
        };

        match create_triage_llm_client(&config) {
            Err(err) => assert!(err.contains("missing profile for phase Triage")),
            Ok(_) => panic!("expected missing triage profile error"),
        }
    }

    #[test]
    fn create_web_fetcher_llm_client_uses_web_fetcher_phase_profile() {
        let config = AdjutantConfig::default();

        let client = create_web_fetcher_llm_client(&config).expect("web fetcher client");
        assert!(matches!(client, ConfiguredLlmClient::OpenAiCompatible(_)));

        let profile = config.get_profile(&AgentPhase::WebFetcher);
        assert_eq!(profile.model_name, "deepseek-chat");
    }
}
