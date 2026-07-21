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
    LogAnalyzer,
    WebFetcher,
    Planner,
    PlannerEmit,
    GitJanitor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[serde(alias = "deepseek")]
    DeepSeek,
    OpenRouter,
    OpenAI,
    Custom,
}

/// Resolved LLM transport + generation settings (what clients consume).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseProfile {
    pub provider: Provider,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model_name: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// Shared credentials — referenced by phase bindings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub api_key: Option<String>,
    pub base_url: String,
}

/// Per-agent model settings pointing at a shared provider profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseBinding {
    pub profile_id: String,
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

pub const DEFAULT_PROFILE_ID: &str = "default";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdjutantConfig {
    #[serde(default)]
    pub profiles: HashMap<String, ProviderProfile>,
    #[serde(default)]
    pub default_profile_id: Option<String>,
    pub phases: HashMap<AgentPhase, PhaseBinding>,
    pub server_port: u16,
    pub storage_path: String,
    #[serde(default)]
    pub triage_overrides: Option<HashMap<String, String>>,
    #[serde(default)]
    pub web_fetcher: Option<WebFetcherProfile>,
}

impl Default for AdjutantConfig {
    fn default() -> Self {
        let profiles = HashMap::from([(
            DEFAULT_PROFILE_ID.to_string(),
            ProviderProfile {
                id: DEFAULT_PROFILE_ID.to_string(),
                name: "DeepSeek Default".to_string(),
                provider: Provider::DeepSeek,
                api_key: None,
                base_url: "https://api.deepseek.com/v1".to_string(),
            },
        )]);
        let phases = [
            (AgentPhase::Scout, phase_binding("deepseek-chat", 4_096, 0.3)),
            (AgentPhase::Pruner, phase_binding("deepseek-chat", 8_192, 0.1)),
            (
                AgentPhase::Builder,
                phase_binding("deepseek-coder", 8_192, 0.2),
            ),
            (
                AgentPhase::Transformer,
                phase_binding("deepseek-coder", 8_192, 0.1),
            ),
            (
                AgentPhase::Triage,
                phase_binding("deepseek-coder", 4_096, 0.0),
            ),
            (
                AgentPhase::Babysitter,
                phase_binding("deepseek-chat", 4_096, 0.4),
            ),
            (
                AgentPhase::Evaluator,
                phase_binding("deepseek-chat", 2_048, 0.0),
            ),
            (
                AgentPhase::LogAnalyzer,
                phase_binding("deepseek-chat", 2_048, 0.0),
            ),
            (
                AgentPhase::WebFetcher,
                phase_binding("deepseek-chat", 2_048, 0.2),
            ),
            // ponytail: planner scout uses chat; emit phase uses coder (see PlannerEmit)
            (
                AgentPhase::Planner,
                phase_binding("deepseek-chat", 4_096, 0.3),
            ),
            (
                AgentPhase::PlannerEmit,
                phase_binding("deepseek-coder", 8_192, 0.1),
            ),
            (
                AgentPhase::GitJanitor,
                phase_binding("deepseek-chat", 4_096, 0.2),
            ),
        ]
        .into_iter()
        .collect();

        Self {
            profiles,
            default_profile_id: Some(DEFAULT_PROFILE_ID.to_string()),
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

    pub fn resolve_phase(&self, phase: AgentPhase) -> Result<PhaseProfile, String> {
        let binding = self
            .phases
            .get(&phase)
            .ok_or_else(|| format!("missing binding for phase {phase:?}"))?;
        let provider = self
            .profiles
            .get(&binding.profile_id)
            .ok_or_else(|| format!("missing provider profile {}", binding.profile_id))?;
        Ok(PhaseProfile {
            provider: provider.provider.clone(),
            api_key: provider.api_key.clone(),
            base_url: provider.base_url.clone(),
            model_name: binding.model_name.clone(),
            max_tokens: binding.max_tokens,
            temperature: binding.temperature,
        })
    }

    pub fn get_profile(&self, phase: &AgentPhase) -> PhaseProfile {
        self.resolve_phase(*phase)
            .expect("every agent phase must have a configured profile")
    }

    pub fn try_get_profile(&self, phase: AgentPhase) -> Result<PhaseProfile, String> {
        self.resolve_phase(phase)
    }

    /// Fills in phase bindings / profiles present in defaults but missing from a persisted config.
    pub fn merge_missing_from_defaults(&mut self) {
        let defaults = AdjutantConfig::default();
        for (id, profile) in &defaults.profiles {
            self.profiles
                .entry(id.clone())
                .or_insert_with(|| profile.clone());
        }
        if self.default_profile_id.is_none() {
            self.default_profile_id = defaults.default_profile_id.clone();
        }
        if !self.phases.contains_key(&AgentPhase::Planner) {
            if let Some(scout) = self.phases.get(&AgentPhase::Scout).cloned() {
                self.phases.insert(AgentPhase::Planner, scout);
            }
        }
        if !self.phases.contains_key(&AgentPhase::PlannerEmit) {
            if let Some(builder) = self.phases.get(&AgentPhase::Builder).cloned() {
                let mut emit = planner_emit_from_builder(&builder);
                if self
                    .profiles
                    .get(&builder.profile_id)
                    .is_some_and(|p| p.provider == Provider::OpenRouter)
                {
                    emit.model_name = "google/gemini-2.5-flash".into();
                }
                self.phases.insert(AgentPhase::PlannerEmit, emit);
            }
        }
        for (phase, binding) in defaults.phases {
            self.phases.entry(phase).or_insert(binding);
        }
        if self.web_fetcher.is_none() {
            self.web_fetcher = Some(WebFetcherProfile::default());
        }
    }

    pub fn profile_in_use(&self, profile_id: &str) -> bool {
        self.phases
            .values()
            .any(|b| b.profile_id == profile_id)
            || self.default_profile_id.as_deref() == Some(profile_id)
    }
}

fn planner_emit_from_builder(builder: &PhaseBinding) -> PhaseBinding {
    PhaseBinding {
        profile_id: builder.profile_id.clone(),
        model_name: "deepseek-coder".into(),
        max_tokens: 8_192,
        temperature: 0.1,
    }
}

fn phase_binding(model_name: &str, max_tokens: u32, temperature: f32) -> PhaseBinding {
    PhaseBinding {
        profile_id: DEFAULT_PROFILE_ID.to_string(),
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
            (AgentPhase::LogAnalyzer, "deepseek-chat", 2_048, 0.0),
            (AgentPhase::WebFetcher, "deepseek-chat", 2_048, 0.2),
            (AgentPhase::Planner, "deepseek-chat", 4_096, 0.3),
            (AgentPhase::PlannerEmit, "deepseek-coder", 8_192, 0.1),
            (AgentPhase::GitJanitor, "deepseek-chat", 4_096, 0.2),
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
        assert_eq!(
            config.default_profile_id.as_deref(),
            Some(DEFAULT_PROFILE_ID)
        );
    }

    #[test]
    fn resolve_shares_provider_credentials_across_phases() {
        let mut config = AdjutantConfig::default();
        config.profiles.get_mut(DEFAULT_PROFILE_ID).unwrap().api_key =
            Some("sk-shared".into());
        assert_eq!(
            config.get_profile(&AgentPhase::Scout).api_key.as_deref(),
            Some("sk-shared")
        );
        assert_eq!(
            config.get_profile(&AgentPhase::Triage).api_key.as_deref(),
            Some("sk-shared")
        );
    }

    #[test]
    fn planner_emit_from_builder_keeps_profile_id() {
        let builder = PhaseBinding {
            profile_id: "or-1".into(),
            model_name: "google/gemini-3.1-flash-lite".into(),
            max_tokens: 8_192,
            temperature: 0.2,
        };
        let emit = planner_emit_from_builder(&builder);
        assert_eq!(emit.profile_id, "or-1");
        assert_eq!(emit.model_name, "deepseek-coder");
        assert_eq!(emit.max_tokens, 8_192);
    }

    #[test]
    fn merge_missing_from_defaults_adds_new_phases() {
        let mut legacy = AdjutantConfig {
            phases: HashMap::from([
                (
                    AgentPhase::Scout,
                    phase_binding("deepseek-chat", 4_096, 0.3),
                ),
                (
                    AgentPhase::Builder,
                    phase_binding("deepseek-coder", 8_192, 0.2),
                ),
            ]),
            profiles: AdjutantConfig::default().profiles,
            default_profile_id: Some(DEFAULT_PROFILE_ID.into()),
            ..Default::default()
        };
        // wipe other phases that Default filled via ..Default
        legacy.phases.retain(|p, _| {
            matches!(p, AgentPhase::Scout | AgentPhase::Builder)
        });

        assert!(legacy.try_get_profile(AgentPhase::Evaluator).is_err());
        legacy.merge_missing_from_defaults();
        let evaluator = legacy
            .try_get_profile(AgentPhase::Evaluator)
            .expect("evaluator profile");
        assert_eq!(evaluator.model_name, "deepseek-chat");
        assert_eq!(evaluator.max_tokens, 2_048);
        assert_eq!(
            legacy.get_profile(&AgentPhase::Builder).model_name,
            "deepseek-coder"
        );
        assert_eq!(
            legacy.get_profile(&AgentPhase::Planner).model_name,
            "deepseek-chat"
        );
        assert_eq!(
            legacy.get_profile(&AgentPhase::PlannerEmit).model_name,
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
                phase_binding("deepseek-chat", 4_096, 0.3),
            )]),
            web_fetcher: None,
            ..Default::default()
        };
        legacy.phases.retain(|p, _| matches!(p, AgentPhase::Scout));
        legacy.merge_missing_from_defaults();
        let profile = legacy
            .web_fetcher
            .as_ref()
            .expect("merge should restore WebFetcherProfile");
        assert_eq!(profile.max_search_hops, 3);
        assert!(legacy.try_get_profile(AgentPhase::WebFetcher).is_ok());
    }

    #[test]
    fn merge_missing_from_defaults_adds_log_analyzer() {
        let mut legacy = AdjutantConfig {
            phases: HashMap::from([(
                AgentPhase::Scout,
                phase_binding("deepseek-chat", 4_096, 0.3),
            )]),
            ..Default::default()
        };
        legacy.phases.retain(|p, _| matches!(p, AgentPhase::Scout));
        assert!(legacy.try_get_profile(AgentPhase::LogAnalyzer).is_err());
        legacy.merge_missing_from_defaults();
        let log_analyzer = legacy
            .try_get_profile(AgentPhase::LogAnalyzer)
            .expect("log_analyzer profile");
        assert_eq!(log_analyzer.model_name, "deepseek-chat");
        assert_eq!(log_analyzer.max_tokens, 2_048);
    }

    #[test]
    fn profile_in_use_detects_bindings() {
        let config = AdjutantConfig::default();
        assert!(config.profile_in_use(DEFAULT_PROFILE_ID));
        assert!(!config.profile_in_use("missing"));
    }
}
