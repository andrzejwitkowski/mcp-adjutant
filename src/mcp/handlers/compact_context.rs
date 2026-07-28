use std::sync::Arc;

use serde_json::Value;

use super::{dispatch_async_job, finish_agent_job_with_eval, JobRegistry};
use crate::agent::{compact_text, CompactMode};
use crate::cache::require_workspace_root_arg;
use crate::domain::{AdjutantConfig, AgentPhase};
use crate::jobs::parse_request_uuid;
use crate::llm::create_pruner_llm_client;
use crate::mcp::schemas::COMPACT_CONTEXT_TOOL_NAME;

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
    let target_override = match args.get("target_tokens") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| "target_tokens must be a positive integer".to_string())?;
            let n = u32::try_from(n).map_err(|_| "target_tokens exceeds u32::MAX".to_string())?;
            if n == 0 {
                return Err("target_tokens must be greater than zero".to_string());
            }
            Some(n)
        }
    };

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
            let target = target_override.unwrap_or(
                merged
                    .try_get_profile(AgentPhase::Pruner)?
                    .context_window_tokens,
            );
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
}
