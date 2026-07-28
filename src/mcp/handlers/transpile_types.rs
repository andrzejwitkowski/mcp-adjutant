//! Cross-language type sync via TranspilerAgent.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::{
    default_verify_workspace, embed_source_files, parse_transpile_types_args,
    AgentLoopOrchestrator, SystemBuildRunner, TranspilerAgent, TriageAgent,
    TRANSPILER_MAX_ITERATIONS, TRANSPILER_SYSTEM_PROMPT,
};
use crate::cache::{require_workspace_root_arg, resolve_workspace_path};
use crate::domain::{AdjutantConfig, AgentPhase};
use crate::jobs::{parse_request_uuid, JobRegistry};
use crate::llm::{create_builder_llm_client, create_triage_llm_client};
use crate::mcp::schemas::TRANSPILE_TYPES_TOOL_NAME;
use crate::tools::LlmBuildDiscoverer;

use super::{dispatch_async_job, ensure_mutating_preflight, finish_agent_job_with_eval};

const SOURCE_EMBED_MAX_BYTES: usize = 64 * 1024;

pub async fn handle_transpile_types(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let parsed = parse_transpile_types_args(&args)?;

    dispatch_async_job(
        registry,
        request_uuid,
        TRANSPILE_TYPES_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            ensure_mutating_preflight(&config, &[AgentPhase::Builder, AgentPhase::Triage])?;
            let resolved_sources: Vec<PathBuf> = parsed
                .source_paths
                .iter()
                .map(resolve_workspace_path)
                .collect();
            // ponytail: transpile LLM uses builder phase profile
            let resolved_target = resolve_workspace_path(&parsed.target_path);
            let resolved_preserve: Vec<PathBuf> = parsed
                .preserve_paths
                .iter()
                .map(resolve_workspace_path)
                .collect();

            let verify_ws = parsed
                .verify_workspace
                .map(resolve_workspace_path)
                .unwrap_or_else(|| default_verify_workspace(&resolved_target));
            let verify_command = parsed.verify_command;

            let sources_block = embed_source_files(&resolved_sources, SOURCE_EMBED_MAX_BYTES)?;
            let preserve_list = resolved_preserve
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let verify_line = verify_command
                .as_deref()
                .map(|cmd| format!("cd {} && {cmd}", verify_ws.display()))
                .unwrap_or_else(|| "auto (child triage discovers)".to_string());

            let prompt = format!(
                "{TRANSPILE_TYPES_TOOL_NAME}\n\n\
                 ## Architecture layout (coordinator)\n\n{architecture_layout}\n\n\
                 {sources_block}\
                 ## Targets\n\n- target_path: {target}\n- preserve_paths: {preserve_list}\n- verify: {verify_line}\n\n\
                 {TRANSPILER_SYSTEM_PROMPT}",
                architecture_layout = parsed.architecture_layout,
                target = resolved_target.display(),
            );

            let transpiler_client = create_builder_llm_client(&config)?;
            let triage_client = Arc::new(create_triage_llm_client(&config)?);

            let original_task = prompt.clone();
            let result = if verify_command.is_some() {
                let triage_agent = TriageAgent::with_build_runner(
                    Arc::clone(&triage_client),
                    vec![resolved_target.clone()],
                    Arc::clone(&config),
                    SystemBuildRunner,
                );
                let agent = TranspilerAgent::new(
                    transpiler_client,
                    triage_agent,
                    resolved_target,
                    resolved_preserve,
                    verify_ws,
                    verify_command,
                );
                AgentLoopOrchestrator::run(&agent, prompt, TRANSPILER_MAX_ITERATIONS).await
            } else {
                let discoverer = LlmBuildDiscoverer::new(Arc::clone(&triage_client));
                let triage_agent = TriageAgent::with_build_runner_and_discoverer(
                    Arc::clone(&triage_client),
                    vec![resolved_target.clone()],
                    Arc::clone(&config),
                    SystemBuildRunner,
                    discoverer,
                );
                let agent = TranspilerAgent::new(
                    transpiler_client,
                    triage_agent,
                    resolved_target,
                    resolved_preserve,
                    verify_ws,
                    verify_command,
                );
                AgentLoopOrchestrator::run(&agent, prompt, TRANSPILER_MAX_ITERATIONS).await
            }?;

            let output = if result.is_finished && result.agent_completed {
                result.accumulated_data
            } else {
                format!(
                    "Transpiler report (finished={}, iterations={}):\n{}",
                    result.is_finished, result.iterations, result.accumulated_data
                )
            };
            Ok(finish_agent_job_with_eval(
                &config,
                TRANSPILE_TYPES_TOOL_NAME,
                &original_task,
                output,
            )
            .await)
        },
    )
    .await
}
