//! Deterministic blueprint apply → triage → Builder generate_tests.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::{
    apply_blueprint_file_steps, display_rel, extract_json_object, format_triage_success,
    source_under_test_from_goal, test_type_for_target, triage_passed, validate_blueprint,
    AgentLoopOrchestrator, SystemBuildRunner, TriageAgent, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::{mcp_workspace_root, require_workspace_root_arg, resolve_workspace_path};
use crate::domain::{AdjutantConfig, AgentPhase};
use crate::jobs::{parse_request_uuid, JobRegistry};
use crate::llm::create_triage_llm_client;
use crate::mcp::schemas::{EXECUTE_BLUEPRINT_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME};
use crate::tools::LlmBuildDiscoverer;

use super::builder_job::run_builder_green;
use super::{
    dispatch_async_job, ensure_mutating_preflight, finish_agent_job_with_eval,
    strip_auto_eval_appendix, TRIAGE_MAX_ITERATIONS,
};

pub async fn handle_execute_blueprint(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let blueprint_raw = args
        .get("blueprint")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "blueprint is required".to_string())?
        .to_string();

    dispatch_async_job(
        registry,
        request_uuid,
        EXECUTE_BLUEPRINT_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            ensure_mutating_preflight(
                &config,
                &[AgentPhase::Builder, AgentPhase::Scout, AgentPhase::Triage],
            )?;

            let stripped = strip_auto_eval_appendix(&blueprint_raw);
            let json_body = extract_json_object(stripped).unwrap_or(stripped);
            let blueprint = validate_blueprint(json_body)?;
            let task_id = blueprint
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();

            let project_root = mcp_workspace_root();
            let mut report = format!("[BLUEPRINT EXECUTE]\ntask_id: {task_id}\n");

            let touched = apply_blueprint_file_steps(&blueprint)?;
            report.push_str(&format!("\n## Applied file steps ({})\n", touched.len()));
            for path in &touched {
                report.push_str(&format!("- {}\n", display_rel(&project_root, path)));
            }

            if touched.is_empty() {
                report.push_str("\n## Triage\nskipped (no file steps)\n");
            } else {
                report.push_str(&run_post_apply_triage(&config, &touched).await?);
            }

            let pipeline = blueprint
                .get("pipeline")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            // ponytail: CI integration proves apply+triage without spinning Builder LLM
            let skip_generate = std::env::var("MCP_ADJUTANT_TEST_SKIP_GENERATE_TESTS")
                .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

            for step in &pipeline {
                if step.get("action").and_then(Value::as_str) != Some("generate_tests") {
                    continue;
                }
                if skip_generate {
                    report.push_str(
                        "\n## generate_tests\nskipped (MCP_ADJUTANT_TEST_SKIP_GENERATE_TESTS)\n",
                    );
                    continue;
                }
                let target_file = step
                    .get("target_file")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let goal = step.get("goal").and_then(Value::as_str).unwrap_or_default();
                let cited = source_under_test_from_goal(goal).ok_or_else(|| {
                    format!(
                        "generate_tests goal must cite non-test source path:line — got: {goal:?}"
                    )
                })?;
                let source_path = resolve_source_under_test(&cited)?;
                let source_rel = display_rel(&project_root, &source_path);
                let test_type = test_type_for_target(target_file);

                report.push_str(&format!(
                    "\n## generate_tests\ntarget={target_file}\nsource={source_rel}\ntest_type={test_type}\n"
                ));

                let built =
                    run_builder_green(&config, &source_path, &source_rel, test_type).await?;
                if !built.green_ok {
                    return Err(format!(
                        "Builder did not complete GREEN verification.\n{}",
                        built.output
                    ));
                }
                report.push_str(&built.output);
                report.push('\n');
            }

            let journal_note = crate::mutation_journal::with_active_journal(|j| j.diff_summary())
                .unwrap_or_default();
            if !journal_note.is_empty() {
                report.push_str(&format!("\n## Diff summary\n{journal_note}\n"));
            }

            Ok(finish_agent_job_with_eval(
                &config,
                EXECUTE_BLUEPRINT_TOOL_NAME,
                &format!("execute_blueprint {task_id}"),
                report,
            )
            .await)
        },
    )
    .await
}

async fn run_post_apply_triage(
    config: &AdjutantConfig,
    touched: &[PathBuf],
) -> Result<String, String> {
    let triage_client = Arc::new(create_triage_llm_client(config)?);
    let agent = TriageAgent::with_build_runner_and_discoverer(
        Arc::clone(&triage_client),
        touched.to_vec(),
        Arc::new(config.clone()),
        SystemBuildRunner,
        LlmBuildDiscoverer::new(Arc::clone(&triage_client)),
    );
    let triage_task = format!("{VERIFY_AND_TRIAGE_TOOL_NAME}\n{TRIAGE_SYSTEM_PROMPT}");
    let triage_result =
        AgentLoopOrchestrator::run(&agent, triage_task, TRIAGE_MAX_ITERATIONS).await?;
    if !triage_result.is_finished || !triage_passed(&triage_result) {
        return Err(format!(
            "Triage failed after blueprint apply (finished={}, iterations={}).\n{}",
            triage_result.is_finished, triage_result.iterations, triage_result.accumulated_data
        ));
    }
    Ok(format!(
        "\n## Triage\nPASS\n{}\n",
        format_triage_success(&triage_result, touched)
    ))
}

fn resolve_source_under_test(cited: &str) -> Result<PathBuf, String> {
    let path = resolve_workspace_path(cited);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "source under test not found for citation {cited:?} (workspace-relative path required)"
        ))
    }
}
