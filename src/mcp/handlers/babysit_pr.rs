//! Babysit a PR via BabysitterAgent.

use std::sync::Arc;

use serde_json::Value;

use crate::agent::{
    format_babysitter_result, AgentLoopOrchestrator, BabysitterAgent, SystemBuildRunner,
    TriageAgent, BABYSITTER_MAX_ITERATIONS, BABYSITTER_SYSTEM_PROMPT,
};
use crate::cache::require_workspace_root_arg;
use crate::domain::AdjutantConfig;
use crate::jobs::{parse_request_uuid, JobRegistry};
use crate::llm::{create_babysitter_llm_client, create_triage_llm_client};
use crate::mcp::schemas::BABYSIT_PR_TOOL_NAME;
use crate::metrics::{load_best_desired_output_exemplar, metrics_store};
use crate::tools::{assert_on_pr_head_branch, gh_pr_state, LlmBuildDiscoverer};

use super::{dispatch_async_job, finish_agent_job_with_eval};

pub async fn handle_babysit_pr(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let pr_number = args
        .get("pr_number")
        .and_then(Value::as_u64)
        .ok_or_else(|| "pr_number is required".to_string())?;

    dispatch_async_job(
        registry,
        request_uuid,
        BABYSIT_PR_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            let pr_state = gh_pr_state(pr_number)?;
            assert_on_pr_head_branch(&pr_state.head_ref_name)?;

            let babysitter_client = create_babysitter_llm_client(&config)?;
            let triage_client = Arc::new(create_triage_llm_client(&config)?);
            let discoverer = LlmBuildDiscoverer::new(Arc::clone(&triage_client));
            let triage_agent = TriageAgent::with_build_runner_and_discoverer(
                Arc::clone(&triage_client),
                Vec::new(),
                Arc::clone(&config),
                SystemBuildRunner,
                discoverer,
            );
            let agent = BabysitterAgent::new(
                babysitter_client,
                Arc::clone(&config),
                triage_agent,
                pr_number,
            );

            let original_task = format!("{BABYSIT_PR_TOOL_NAME} PR #{pr_number}");
            let mut prompt =
                format!("{BABYSIT_PR_TOOL_NAME}\nPR #{pr_number}\n\n{BABYSITTER_SYSTEM_PROMPT}");
            let exemplar = metrics_store().and_then(|store| {
                let guard = store.lock().ok()?;
                load_best_desired_output_exemplar(guard.connection(), "BabysitterAgent")
                    .ok()
                    .flatten()
            });
            if let Some(exemplar) = exemplar {
                prompt.push_str("\n\n## 10/10 output exemplar (match this JSON shape)\n");
                prompt.push_str(&exemplar);
            }
            let result =
                AgentLoopOrchestrator::run(&agent, prompt, BABYSITTER_MAX_ITERATIONS).await?;

            let output = if result.is_finished && result.agent_completed {
                result.accumulated_data
            } else {
                let state = gh_pr_state(pr_number)?;
                let (report_posted, paths_seen, paths_handled) = agent.session_snapshot();
                format_babysitter_result(
                    &state,
                    report_posted,
                    &paths_seen,
                    &paths_handled,
                    &[],
                    pr_number,
                    result.iterations,
                    Some(&result.accumulated_data),
                    Some("session incomplete"),
                )?
            };
            Ok(
                finish_agent_job_with_eval(&config, BABYSIT_PR_TOOL_NAME, &original_task, output)
                    .await,
            )
        },
    )
    .await
}
