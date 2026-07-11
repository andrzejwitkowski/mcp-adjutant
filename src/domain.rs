use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AdjutantConfigError;
use crate::storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Scout,
    Pruner,
    Builder,
    Transformer,
    Triage,
    Babysitter,
    Evaluator,
    WebFetcher,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    DeepSeek,
    OpenRouter,
    OpenAI,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseProfile {
    pub provider: Provider,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model_name: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebFetcherProfile {
    #[serde(default, skip_serializing)]
    pub brave_api_key: Option<String>,
    #[serde(default = "default_max_search_hops")]
    pub max_search_hops: u32,
    #[serde(default = "default_token_budget")]
    pub token_budget: u32,
    #[serde(default = "default_cache_ttl_seconds")]
    pub cache_ttl_seconds: u64,
    #[serde(default = "default_web_cache_threshold")]
    pub web_cache_threshold: f32,
}

fn default_max_search_hops() -> u32 {
    3
}
fn default_token_budget() -> u32 {
    8_000
}
fn default_cache_ttl_seconds() -> u64 {
    604_800
}
fn default_web_cache_threshold() -> f32 {
    0.78
}

impl Default for WebFetcherProfile {
    fn default() -> Self {
        Self {
            brave_api_key: None,
            max_search_hops: default_max_search_hops(),
            token_budget: default_token_budget(),
            cache_ttl_seconds: default_cache_ttl_seconds(),
            web_cache_threshold: default_web_cache_threshold(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdjutantConfig {
    pub phases: HashMap<AgentPhase, PhaseProfile>,
    pub server_port: u16,
    pub storage_path: String,
    #[serde(default)]
    pub triage_overrides: Option<HashMap<String, String>>,
    #[serde(default)]
    pub web_fetcher: Option<WebFetcherProfile>,
}

impl Default for AdjutantConfig {
    fn default() -> Self {
        let phases = [
            (
                AgentPhase::Scout,
                phase_profile("deepseek-chat", 4_096, 0.3),
            ),
            (
                AgentPhase::Pruner,
                phase_profile("deepseek-chat", 8_192, 0.1),
            ),
            (
                AgentPhase::Builder,
                phase_profile("deepseek-coder", 8_192, 0.2),
            ),
            (
                AgentPhase::Transformer,
                phase_profile("deepseek-coder", 8_192, 0.1),
            ),
            (
                AgentPhase::Triage,
                phase_profile("deepseek-coder", 4_096, 0.0),
            ),
            (
                AgentPhase::Babysitter,
                phase_profile("deepseek-chat", 4_096, 0.4),
            ),
            (
                AgentPhase::Evaluator,
                phase_profile("deepseek-chat", 2_048, 0.0),
            ),
            (
                AgentPhase::WebFetcher,
                phase_profile("deepseek-chat", 2_048, 0.2),
            ),
        ]
        .into_iter()
        .collect();

        Self {
            phases,
            server_port: 3_000,
            storage_path: default_storage_path(),
            triage_overrides: None,
            web_fetcher: Some(WebFetcherProfile::default()),
        }
    }
}

impl AdjutantConfig {
    pub fn load_from_file(path: &Path) -> Result<Self, AdjutantConfigError> {
        storage::load_from_file(path)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), AdjutantConfigError> {
        storage::save_to_file(self, path)
    }

    pub fn get_profile(&self, phase: &AgentPhase) -> &PhaseProfile {
        self.phases
            .get(phase)
            .expect("every agent phase must have a configured profile")
    }

    pub fn try_get_profile(&self, phase: AgentPhase) -> Result<&PhaseProfile, String> {
        self.phases
            .get(&phase)
            .ok_or_else(|| format!("missing profile for phase {phase:?}"))
    }

    /// Fills in phase profiles present in defaults but missing from a persisted config.
    pub fn merge_missing_from_defaults(&mut self) {
        for (phase, profile) in AdjutantConfig::default().phases {
            self.phases.entry(phase).or_insert(profile);
        }
        if self.web_fetcher.is_none() {
            self.web_fetcher = Some(WebFetcherProfile::default());
        }
    }
}

fn phase_profile(model_name: &str, max_tokens: u32, temperature: f32) -> PhaseProfile {
    PhaseProfile {
        provider: Provider::DeepSeek,
        api_key: None,
        base_url: "https://api.deepseek.com/v1".to_string(),
        model_name: model_name.to_string(),
        max_tokens,
        temperature,
    }
}

fn default_storage_path() -> String {
    home::home_dir()
        .map(|dir| {
            dir.join(".config/mcp-adjutant/config.json")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "~/.config/mcp-adjutant/config.json".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_deepseek_profiles_for_all_phases() {
        let config = AdjutantConfig::default();

        let expected_models = [
            (AgentPhase::Scout, "deepseek-chat", 4_096, 0.3),
            (AgentPhase::Pruner, "deepseek-chat", 8_192, 0.1),
            (AgentPhase::Builder, "deepseek-coder", 8_192, 0.2),
            (AgentPhase::Transformer, "deepseek-coder", 8_192, 0.1),
            (AgentPhase::Triage, "deepseek-coder", 4_096, 0.0),
            (AgentPhase::Babysitter, "deepseek-chat", 4_096, 0.4),
            (AgentPhase::Evaluator, "deepseek-chat", 2_048, 0.0),
            (AgentPhase::WebFetcher, "deepseek-chat", 2_048, 0.2),
        ];

        for (phase, model_name, max_tokens, temperature) in expected_models {
            let profile = config.get_profile(&phase);
            assert_eq!(profile.provider, Provider::DeepSeek);
            assert_eq!(profile.api_key, None);
            assert_eq!(profile.base_url, "https://api.deepseek.com/v1");
            assert_eq!(profile.model_name, model_name);
            assert_eq!(profile.max_tokens, max_tokens);
            assert!((profile.temperature - temperature).abs() < f32::EPSILON);
        }

        assert_eq!(config.server_port, 3_000);
        assert!(!config.storage_path.is_empty());
    }

    #[test]
    fn merge_missing_from_defaults_adds_new_phases() {
        let mut legacy = AdjutantConfig {
            phases: HashMap::from([
                (
                    AgentPhase::Scout,
                    phase_profile("deepseek-chat", 4_096, 0.3),
                ),
                (
                    AgentPhase::Builder,
                    phase_profile("deepseek-coder", 8_192, 0.2),
                ),
            ]),
            ..Default::default()
        };

        assert!(legacy.try_get_profile(AgentPhase::Evaluator).is_err());

        legacy.merge_missing_from_defaults();

        let evaluator = legacy
            .try_get_profile(AgentPhase::Evaluator)
            .expect("evaluator profile");
        assert_eq!(evaluator.model_name, "deepseek-chat");
        assert_eq!(evaluator.max_tokens, 2_048);
        assert!((evaluator.temperature - 0.0).abs() < f32::EPSILON);
        assert_eq!(
            legacy.get_profile(&AgentPhase::Builder).model_name,
            "deepseek-coder"
        );
    }

    #[test]
    fn default_config_has_web_fetcher_phase_and_profile() {
        let config = AdjutantConfig::default();

        let web_fetcher = config.get_profile(&AgentPhase::WebFetcher);
        assert_eq!(web_fetcher.provider, Provider::DeepSeek);
        assert_eq!(web_fetcher.model_name, "deepseek-chat");
        assert_eq!(web_fetcher.max_tokens, 2_048);

        let profile = config
            .web_fetcher
            .as_ref()
            .expect("default config should include a WebFetcherProfile");
        assert_eq!(profile.max_search_hops, 3);
        assert_eq!(profile.token_budget, 8_000);
        assert_eq!(profile.cache_ttl_seconds, 604_800);
        assert!((profile.web_cache_threshold - 0.78).abs() < f32::EPSILON);
    }

    #[test]
    fn merge_missing_from_defaults_restores_web_fetcher_profile() {
        let mut legacy = AdjutantConfig {
            phases: HashMap::from([(
                AgentPhase::Scout,
                phase_profile("deepseek-chat", 4_096, 0.3),
            )]),
            web_fetcher: None,
            ..Default::default()
        };

        legacy.merge_missing_from_defaults();

        let profile = legacy
            .web_fetcher
            .as_ref()
            .expect("merge should restore WebFetcherProfile");
        assert_eq!(profile.max_search_hops, 3);
        assert_eq!(profile.token_budget, 8_000);
        assert!(legacy.try_get_profile(AgentPhase::WebFetcher).is_ok());
    }
}
