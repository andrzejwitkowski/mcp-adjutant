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
}
