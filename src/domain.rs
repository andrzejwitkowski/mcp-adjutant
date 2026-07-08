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
    Triage,
    Babysitter,
    Evaluator,
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
pub struct AdjutantConfig {
    pub phases: HashMap<AgentPhase, PhaseProfile>,
    pub server_port: u16,
    pub storage_path: String,
    #[serde(default)]
    pub triage_overrides: Option<HashMap<String, String>>,
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
        ]
        .into_iter()
        .collect();

        Self {
            phases,
            server_port: 3_000,
            storage_path: default_storage_path(),
            triage_overrides: None,
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
            (AgentPhase::Triage, "deepseek-coder", 4_096, 0.0),
            (AgentPhase::Babysitter, "deepseek-chat", 4_096, 0.4),
            (AgentPhase::Evaluator, "deepseek-chat", 2_048, 0.0),
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
}
