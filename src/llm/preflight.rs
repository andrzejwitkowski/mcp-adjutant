use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde_json::Value;

use crate::domain::{AdjutantConfig, AgentPhase, PhaseProfile};
use crate::llm::tools::{LlmTool, LlmToolSet, ToolDefinition};
use crate::llm::{LlmClient, LlmRequest, OpenAiCompatibleClient};

#[derive(Debug, Clone)]
pub struct PreflightReport {
    pub base_url: String,
    pub model_name: String,
    pub omit_temperature: bool,
    pub models_ok: bool,
    pub tool_call_ok: bool,
    pub no_tools_ok: bool,
    pub config_checksum: String,
}

impl PreflightReport {
    pub fn is_ok(&self) -> bool {
        self.models_ok && self.tool_call_ok && self.no_tools_ok
    }

    pub fn summary(&self) -> String {
        format!(
            "preflight model={} base={} checksum={} models={} tools={} no_tools={} omit_temp={}",
            self.model_name,
            self.base_url,
            self.config_checksum,
            self.models_ok,
            self.tool_call_ok,
            self.no_tools_ok,
            self.omit_temperature
        )
    }
}

pub fn skip_preflight() -> bool {
    std::env::var("MCP_ADJUTANT_SKIP_PREFLIGHT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn config_checksum(config: &AdjutantConfig) -> String {
    let mut hasher = DefaultHasher::new();
    let mut phases: Vec<_> = config.phases.keys().copied().collect();
    phases.sort_by_key(|p| format!("{p:?}"));
    for phase in phases {
        if let Ok(profile) = config.resolve_phase(phase) {
            profile.provider.hash(&mut hasher);
            profile.base_url.hash(&mut hasher);
            profile.model_name.hash(&mut hasher);
            profile.max_tokens.hash(&mut hasher);
            profile.temperature.to_bits().hash(&mut hasher);
            profile.api_key.is_some().hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

struct PingTool {
    definition: ToolDefinition,
}

impl PingTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new("preflight_ping", "No-op probe tool for preflight.")
                .string_param("note", "Ignored", false),
        }
    }
}

impl LlmTool for PingTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, _arguments: &Value) -> Result<String, String> {
        Ok("pong".into())
    }
}

fn model_listed(models_json: &Value, model_name: &str) -> bool {
    let Some(data) = models_json.get("data").and_then(Value::as_array) else {
        return models_json.to_string().contains(model_name);
    };
    data.iter().any(|item| {
        item.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == model_name || id.ends_with(&format!("/{model_name}")))
    })
}

pub fn preflight_phase(
    profile: &PhaseProfile,
    config: &AdjutantConfig,
) -> Result<PreflightReport, String> {
    let checksum = config_checksum(config);
    let mut client = OpenAiCompatibleClient::new(profile.clone());

    let models_json = client.fetch_models_list()?;
    let models_ok = model_listed(&models_json, &profile.model_name);
    if !models_ok {
        return Err(format!(
            "preflight: model '{}' not found at {}/models (checksum={checksum})",
            profile.model_name,
            profile.base_url.trim_end_matches('/')
        ));
    }

    let tools = LlmToolSet::new().register(PingTool::new());
    let tool_req = LlmRequest::new(
        "You are a preflight probe. Call preflight_ping once.",
        "ping",
        &tools,
    );
    let tool_turn = client
        .complete(tool_req)
        .map_err(|err| format!("preflight tool-call failed ({checksum}): {err}"))?;
    let tool_call_ok = !tool_turn.tool_calls.is_empty();

    let empty = LlmToolSet::new();
    let plain_req = LlmRequest::new("Reply with the single word ok.", "Say ok", &empty);
    let plain = match client.complete(plain_req) {
        Ok(turn) => turn,
        Err(err) if err.to_ascii_lowercase().contains("temperature") => {
            client = OpenAiCompatibleClient::with_omit_temperature(profile.clone(), true);
            client
                .complete(LlmRequest::new(
                    "Reply with the single word ok.",
                    "Say ok",
                    &empty,
                ))
                .map_err(|e| {
                    format!("preflight no-tools failed after omit temperature ({checksum}): {e}")
                })?
        }
        Err(err) => return Err(format!("preflight no-tools failed ({checksum}): {err}")),
    };
    let no_tools_ok = plain.content.as_ref().is_some_and(|c| !c.trim().is_empty())
        || !plain.tool_calls.is_empty();

    let report = PreflightReport {
        base_url: profile.base_url.clone(),
        model_name: profile.model_name.clone(),
        omit_temperature: client.omits_temperature(),
        models_ok,
        tool_call_ok,
        no_tools_ok,
        config_checksum: checksum,
    };
    if !report.is_ok() {
        return Err(format!("preflight incomplete: {}", report.summary()));
    }
    eprintln!("[adjutant] {}", report.summary());
    Ok(report)
}

pub fn preflight_config(config: &AdjutantConfig) -> Result<String, String> {
    let checksum = config_checksum(config);
    let mut seen = std::collections::HashSet::new();
    for phase in [
        AgentPhase::Scout,
        AgentPhase::Builder,
        AgentPhase::Evaluator,
        AgentPhase::Triage,
        AgentPhase::Transformer,
    ] {
        let Ok(profile) = config.resolve_phase(phase) else {
            continue;
        };
        let key = format!(
            "{}|{}|{}",
            profile.base_url,
            profile.model_name,
            profile.temperature.to_bits()
        );
        if !seen.insert(key) {
            continue;
        }
        preflight_phase(&profile, config)?;
    }
    eprintln!("[adjutant] config checksum={checksum}");
    Ok(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_listed_matches_id() {
        let body = json!({
            "data": [
                { "id": "gpt-5-mini" },
                { "id": "github_copilot/gpt-5-mini" }
            ]
        });
        assert!(model_listed(&body, "gpt-5-mini"));
        assert!(!model_listed(&body, "gpt-4"));
        assert!(!model_listed(&body, "other"));
    }

    #[test]
    fn config_checksum_stable_for_same_config() {
        let config = AdjutantConfig::default();
        assert_eq!(config_checksum(&config), config_checksum(&config));
    }
}
