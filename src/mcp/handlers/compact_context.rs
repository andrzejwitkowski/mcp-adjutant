use std::sync::Arc;

use serde_json::Value;

use super::{dispatch_async_job, finish_agent_job_with_eval, JobRegistry};
use crate::agent::{compact_text, CompactMode, COMPACT_CONTEXT_TOOL_NAME};
use crate::cache::require_workspace_root_arg;
use crate::domain::{AdjutantConfig, AgentPhase};
use crate::jobs::parse_request_uuid;
use crate::llm::create_pruner_llm_client;

pub async fn handle_get_agent_context_caps(
    args: Value,
    config: Arc<AdjutantConfig>,
) -> Result<String, String> {
    let _workspace_root = require_workspace_root_arg(&args)?;
    let mut merged = (*config).clone();
    merged.merge_missing_from_defaults();
    serde_json::to_string_pretty(&merged.agent_context_caps())
        .map_err(|err| format!("serialize caps: {err}"))
}

pub async fn handle_compact_context(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let context = args
        .get("context")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "context is required".to_string())?
        .to_string();
    let mode = CompactMode::parse(
        args.get("mode")
            .and_then(Value::as_str)
            .ok_or_else(|| "mode is required".to_string())?,
    )?;
    let agent_phase = args
        .get("agent_phase")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let target_override = args.get("target_tokens").and_then(Value::as_u64).map(|v| v as u32);

    dispatch_async_job(
        registry,
        request_uuid,
        COMPACT_CONTEXT_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            let mut merged = (*config).clone();
            merged.merge_missing_from_defaults();
            let phase = resolve_phase_hint(agent_phase.as_deref()).unwrap_or(AgentPhase::Pruner);
            let window = merged
                .try_get_profile(phase)
                .or_else(|_| merged.try_get_profile(AgentPhase::Pruner))?
                .context_window_tokens;
            let target = target_override.unwrap_or(window);
            let client = create_pruner_llm_client(&merged)?;
            let densified = compact_text(&client, &context, mode, target)?;
            let output = format!(
                "## Compacted context (mode={}, target_tokens={target})\n\n{densified}",
                mode.as_str()
            );
            Ok(finish_agent_job_with_eval(
                &config,
                COMPACT_CONTEXT_TOOL_NAME,
                &format!("compact_context mode={}", mode.as_str()),
                output,
            )
            .await)
        },
    )
    .await
}

fn resolve_phase_hint(raw: Option<&str>) -> Option<AgentPhase> {
    let raw = raw?;
    serde_json::from_value(Value::String(raw.to_ascii_lowercase())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn caps_returns_pruner_and_scout() {
        let config = Arc::new(AdjutantConfig::default());
        let out = handle_get_agent_context_caps(
            json!({ "workspace_root": env!("CARGO_MANIFEST_DIR") }),
            config,
        )
        .await
        .expect("caps");
        assert!(out.contains("\"pruner\""), "{out}");
        assert!(out.contains("\"scout\""), "{out}");
        assert!(out.contains("context_window_tokens"), "{out}");
    }

    #[test]
    fn resolve_phase_hint_parses_snake() {
        assert_eq!(resolve_phase_hint(Some("scout")), Some(AgentPhase::Scout));
        assert_eq!(resolve_phase_hint(Some("PRUNER")), Some(AgentPhase::Pruner));
        assert!(resolve_phase_hint(Some("nope")).is_none());
    }
}
