use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

mod babysit_pr;
mod builder_job;
mod compact_context;
mod execute_blueprint;
mod transpile_types;

pub use babysit_pr::handle_babysit_pr;
pub use compact_context::{handle_compact_context, handle_get_agent_context_caps};
pub use execute_blueprint::handle_execute_blueprint;
pub use transpile_types::handle_transpile_types;

use super::schemas::{
    ANALYZE_LOG_TOOL_NAME, BABYSIT_PR_TOOL_NAME, COMPACT_CONTEXT_TOOL_NAME,
    CREATE_GIT_BRANCH_TOOL_NAME, EVALUATE_AGENT_PERFORMANCE_TOOL_NAME, EXECUTE_BLUEPRINT_TOOL_NAME,
    EXECUTE_GLOBAL_REFACTOR_TOOL_NAME, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
    GET_AGENT_CONTEXT_CAPS_TOOL_NAME, PLAN_BLUEPRINT_TOOL_NAME, PREPARE_GIT_COPY_TOOL_NAME,
    SCOUT_CONTEXT_TOOL_NAME, TRANSPILE_TYPES_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
    WEB_FETCH_TOOL_NAME,
};
use crate::agent::{
    analyze_log_at_path, create_git_branch, default_transformer_agent, extract_json_object,
    format_eval_job_appendix, format_scout_block, format_triage_success,
    gather_conventions_and_diff, parse_plan_blueprint_args, run_git_janitor, run_planner_hybrid,
    run_scout_with_cache, run_web_fetch_with_cache, triage_passed, validate_blueprint,
    validate_blueprint_coordinator, validate_blueprint_grounding, with_auto_compact_async,
    AgentContext, AgentEvalSummary, AgentLoopOrchestrator, AutoCompactGuard,
    CoordinatorConstraints, EvaluatorAgent, GitJanitorAgent, ScoutAgent, ScoutCacheOutcome,
    ScoutInputs, SystemBuildRunner, TriageAgent, WebCacheOutcome, WebFetcherAgent,
    GIT_JANITOR_SYSTEM_PROMPT, TRANSFORMER_MAX_ITERATIONS, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::{
    mcp_workspace_root, require_workspace_root_arg, resolve_workspace_path,
    with_thread_workspace_root, ProjectCacheManager,
};
use crate::domain::{AdjutantConfig, AgentPhase};
use crate::jobs::{accepted_job_response, parse_request_uuid, run_tracked_job, JobRegistry};
use crate::llm::{
    create_evaluator_llm_client, create_git_janitor_llm_client, create_planner_emit_llm_client,
    create_planner_llm_client, create_pruner_llm_client, create_scout_llm_client,
    create_transformer_llm_client, create_triage_llm_client, create_web_fetcher_llm_client,
    preflight_phase, LlmClient,
};
use crate::tools::LlmBuildDiscoverer;

use builder_job::run_builder_green;

const SCOUT_MAX_ITERATIONS: u32 = 10;
pub(crate) const TRIAGE_MAX_ITERATIONS: u32 = 3;
pub(crate) const BUILDER_MAX_ITERATIONS: u32 = 8;
const EVALUATOR_MAX_ITERATIONS: u32 = 1;

const AUTO_EVAL_APPENDIX_MARKER: &str = "[ADJUTANT AUTO-EVAL APPENDIX";

pub(crate) fn strip_auto_eval_appendix(output: &str) -> &str {
    output
        .split(AUTO_EVAL_APPENDIX_MARKER)
        .next()
        .unwrap_or(output)
        .trim_end()
}

/// Reject coordinator paraphrases that would pollute Evaluations UI with junk ≤3 scores.
fn reject_thin_eval_input(target_agent: &str, received_output: &str) -> Result<(), String> {
    let normalized = crate::cache::normalize_agent_name(target_agent);
    let is_triage =
        normalized == "Phase_5_Triage" || target_agent.to_ascii_lowercase().contains("triage");
    if !is_triage {
        return Ok(());
    }
    let out = received_output.trim();
    let claims_pass = out.contains("[TRIAGE PASS]")
        || out.contains("## Triage: PASS")
        || out.contains("Triage: PASS");
    let has_evidence = out.contains("Command:") || out.contains("Exit code");
    if claims_pass && !has_evidence {
        return Err(
            "received_output looks like a Triage PASS paraphrase without Command/Exit evidence. \
             Paste the full verify_and_triage job result (`query_job_status.result`), not a summary."
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn ensure_mutating_preflight(
    config: &AdjutantConfig,
    phases: &[AgentPhase],
) -> Result<(), String> {
    if crate::llm::skip_preflight() {
        return Ok(());
    }
    let mut seen = std::collections::HashSet::new();
    for &phase in phases {
        let profile = config.try_get_profile(phase)?;
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
    Ok(())
}

fn tool_eval_target_agent(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        SCOUT_CONTEXT_TOOL_NAME => Some("Phase_1_Scout"),
        VERIFY_AND_TRIAGE_TOOL_NAME => Some("Phase_5_Triage"),
        GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME => Some("Phase_4_Builder"),
        EXECUTE_GLOBAL_REFACTOR_TOOL_NAME => Some("Phase_3_5_TRANSFORMER"),
        TRANSPILE_TYPES_TOOL_NAME => Some("TranspilerAgent"),
        BABYSIT_PR_TOOL_NAME => Some("BabysitterAgent"),
        WEB_FETCH_TOOL_NAME => Some("WebFetcherAgent"),
        PLAN_BLUEPRINT_TOOL_NAME => Some("PlannerAgent"),
        EXECUTE_BLUEPRINT_TOOL_NAME => Some("BlueprintExecutor"),
        ANALYZE_LOG_TOOL_NAME => Some("LogAnalyzerAgent"),
        PREPARE_GIT_COPY_TOOL_NAME | CREATE_GIT_BRANCH_TOOL_NAME => Some("GitJanitorAgent"),
        COMPACT_CONTEXT_TOOL_NAME => Some("PrunerAgent"),
        GET_AGENT_CONTEXT_CAPS_TOOL_NAME | EVALUATE_AGENT_PERFORMANCE_TOOL_NAME => None,
        _ => None,
    }
}

/// Install auto-compact for `phase`'s window; handlers call this instead of nesting guard setup.
async fn with_phase_auto_compact<F, Fut, T>(
    config: Arc<AdjutantConfig>,
    phase: AgentPhase,
    work: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let mut merged = (*config).clone();
    merged.merge_missing_from_defaults();
    let window = merged.try_get_profile(phase)?.context_window_tokens;
    let pruner = Arc::new(create_pruner_llm_client(&merged)?) as Arc<dyn LlmClient>;
    with_auto_compact_async(
        AutoCompactGuard {
            window_tokens: window,
            pruner,
        },
        work,
    )
    .await
}

// ponytail: sync one-shot eval inside job closure — no extra async job UUID
async fn eval_after_agent_job(
    config: &AdjutantConfig,
    target_agent: &str,
    original_task: &str,
    received_output: &str,
) -> Option<AgentEvalSummary> {
    let received_output = strip_auto_eval_appendix(received_output);
    if normalize_eval_target(target_agent).as_deref() == Some("EvaluatorAgent")
        || received_output.trim().is_empty()
    {
        return None;
    }
    let cache_manager = Arc::new(Mutex::new(
        open_cache_manager_near(&mcp_workspace_root()).ok()?,
    ));
    let client = create_evaluator_llm_client(config).ok()?;
    let agent = EvaluatorAgent::new(
        client,
        cache_manager,
        target_agent,
        original_task,
        received_output,
    );
    agent.evaluate_once().await.ok()
}

fn normalize_eval_target(target_agent: &str) -> Option<String> {
    let trimmed = target_agent.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(crate::cache::normalize_agent_name(trimmed))
    }
}

pub(crate) async fn finish_agent_job_with_eval(
    config: &AdjutantConfig,
    tool_name: &str,
    original_task: &str,
    result: String,
) -> String {
    let summary = match tool_eval_target_agent(tool_name) {
        Some(agent) => eval_after_agent_job(config, agent, original_task, &result).await,
        None => None,
    };
    match summary {
        Some(summary) => result + &format_eval_job_appendix(&summary),
        None => result,
    }
}

pub(crate) async fn dispatch_async_job<F, Fut>(
    registry: &JobRegistry,
    request_uuid: String,
    tool_name: &str,
    await_timeout_secs: u64,
    workspace_root: PathBuf,
    premium_in: String,
    work: F,
) -> Result<String, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<String, String>> + Send + 'static,
{
    registry.register(&request_uuid, tool_name)?;
    let job_registry = registry.clone();
    let accepted_uuid = request_uuid.clone();
    let tool = tool_name.to_string();
    let handle = tokio::spawn(async move {
        run_tracked_job(
            job_registry,
            request_uuid,
            tool,
            Some(workspace_root),
            premium_in,
            work,
        )
        .await;
    });

    let timeout = Duration::from_secs(await_timeout_secs);
    match tokio::time::timeout(timeout, handle).await {
        Ok(join_result) => {
            if let Err(join_err) = join_result {
                registry.fail(&accepted_uuid, format!("job task error: {join_err}"));
            }
            match registry.terminal_result(&accepted_uuid) {
                Some(Ok(result)) => Ok(result),
                Some(Err(error)) => Err(error),
                None => Ok(accepted_job_response(&accepted_uuid, tool_name)),
            }
        }
        Err(_) => Ok(accepted_job_response(&accepted_uuid, tool_name)),
    }
}

pub async fn handle_query_job_status(
    args: Value,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    if args.get("cancel").and_then(Value::as_bool) == Some(true) {
        registry.request_cancel(&request_uuid)?;
    }
    let status = registry.query(&request_uuid)?;
    serde_json::to_string_pretty(&status).map_err(|err| format!("serialize status: {err}"))
}

pub async fn handle_scout_context(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .ok_or_else(|| "query is required".to_string())?
        .to_string();
    let force_refresh = args
        .get("force_refresh")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    dispatch_async_job(
        registry,
        request_uuid,
        SCOUT_CONTEXT_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            with_phase_auto_compact(Arc::clone(&config), AgentPhase::Scout, || async move {
                let cache_manager =
                    Arc::new(Mutex::new(open_cache_manager_near(&mcp_workspace_root())?));
                let client = create_scout_llm_client(&config)?;
                let agent = ScoutAgent::new(client);
                let result = match run_scout_with_cache(
                    &cache_manager,
                    &agent,
                    &query,
                    SCOUT_MAX_ITERATIONS,
                    !force_refresh,
                )
                .await?
                {
                    ScoutCacheOutcome::Hit(report) => format!("[CACHE HIT]\n{report}"),
                    ScoutCacheOutcome::Fresh(report) => report,
                };
                Ok(
                    finish_agent_job_with_eval(&config, SCOUT_CONTEXT_TOOL_NAME, &query, result)
                        .await,
                )
            })
            .await
        },
    )
    .await
}

pub async fn handle_verify_and_triage(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let target_path_raws: Vec<String> = args
        .get("target_paths")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|path| !path.is_empty())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();

    dispatch_async_job(
        registry,
        request_uuid,
        VERIFY_AND_TRIAGE_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            ensure_mutating_preflight(&config, &[AgentPhase::Triage])?;
            let target_paths: Vec<PathBuf> = target_path_raws
                .iter()
                .map(resolve_workspace_path)
                .collect();
            let triage_client = create_triage_llm_client(&config)?;
            let discoverer = LlmBuildDiscoverer::new(create_triage_llm_client(&config)?);
            let target_paths_for_report = target_paths.clone();
            let agent = TriageAgent::with_build_runner_and_discoverer(
                triage_client,
                target_paths,
                Arc::clone(&config),
                SystemBuildRunner,
                discoverer,
            );

            let original_task = format!("{VERIFY_AND_TRIAGE_TOOL_NAME}\n{TRIAGE_SYSTEM_PROMPT}");
            let result =
                AgentLoopOrchestrator::run(&agent, original_task.clone(), TRIAGE_MAX_ITERATIONS)
                    .await?;

            let output = if result.is_finished {
                if triage_passed(&result) {
                    format_triage_success(&result, &target_paths_for_report)
                } else if !result.accumulated_data.is_empty()
                    && !result.accumulated_data.starts_with("Triage targets")
                {
                    result.accumulated_data
                } else {
                    format!(
                        "Triage report (finished={}, iterations={}):\n{}\n{}",
                        result.is_finished,
                        result.iterations,
                        result.input_prompt,
                        result.accumulated_data
                    )
                }
            } else {
                format!(
                    "Triage report (finished={}, iterations={}):\n{}\n{}",
                    result.is_finished,
                    result.iterations,
                    result.input_prompt,
                    result.accumulated_data
                )
            };
            Ok(finish_agent_job_with_eval(
                &config,
                VERIFY_AND_TRIAGE_TOOL_NAME,
                &original_task,
                output,
            )
            .await)
        },
    )
    .await
}

fn embedding_fixture_paths() -> (PathBuf, PathBuf) {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/embedding");
    (fixtures.join("model.onnx"), fixtures.join("tokenizer.json"))
}

pub(crate) fn open_cache_manager_near(source_path: &Path) -> Result<ProjectCacheManager, String> {
    let start_dir = if source_path.is_file() {
        source_path.parent().unwrap_or(source_path)
    } else {
        source_path
    };
    let (model_path, tokenizer_path) = embedding_fixture_paths();
    ProjectCacheManager::new(start_dir, &model_path, &tokenizer_path)
}

pub async fn handle_generate_tests_and_scaffolding(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let source_file_path = args
        .get("source_file_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "source_file_path is required".to_string())?
        .to_string();

    let test_type = args
        .get("test_type")
        .and_then(Value::as_str)
        .ok_or_else(|| "test_type is required".to_string())?
        .to_string();

    const ALLOWED_TEST_TYPES: [&str; 3] = ["unit", "integration", "factory"];
    if !ALLOWED_TEST_TYPES.contains(&test_type.as_str()) {
        return Err(format!(
            "test_type must be one of {ALLOWED_TEST_TYPES:?}, got {test_type:?}"
        ));
    }

    dispatch_async_job(
        registry,
        request_uuid,
        GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            ensure_mutating_preflight(
                &config,
                &[AgentPhase::Builder, AgentPhase::Scout, AgentPhase::Triage],
            )?;
            let source_path = resolve_workspace_path(&source_file_path);
            let built =
                run_builder_green(&config, &source_path, &source_file_path, &test_type).await?;

            // Always auto-eval (incl. RED/fail) so Evaluations UI gets Phase_4_Builder rows.
            let mut final_out = finish_agent_job_with_eval(
                &config,
                GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
                &built.original_task,
                built.output,
            )
            .await;
            // Fail closed: do not leave intentional RED tests in the workspace.
            if !built.green_ok {
                return Err(format!(
                    "Builder did not complete GREEN verification (rolling back writes).\n{final_out}"
                ));
            }
            let journal_note = crate::mutation_journal::with_active_journal(|j| j.diff_summary())
                .unwrap_or_default();
            if !journal_note.is_empty() {
                final_out.push_str(&format!("\n\n## Diff summary\n{journal_note}\n"));
            }
            Ok(final_out)
        },
    )
    .await
}

pub async fn handle_execute_global_refactor(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let method_name = args
        .get("method_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "method_name is required".to_string())?
        .to_string();
    let refactor_instruction = args
        .get("refactor_instruction")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "refactor_instruction is required".to_string())?
        .to_string();
    let scope_path_raw = args
        .get("scope_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    dispatch_async_job(
        registry,
        request_uuid,
        EXECUTE_GLOBAL_REFACTOR_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            ensure_mutating_preflight(
                &config,
                &[
                    AgentPhase::Transformer,
                    AgentPhase::Scout,
                    AgentPhase::Triage,
                ],
            )?;
            let scope_path = scope_path_raw.as_ref().map(resolve_workspace_path);
            let transformer_client = create_transformer_llm_client(&config)?;
            let codemod_client = create_transformer_llm_client(&config)?;
            let scout_client = create_scout_llm_client(&config)?;
            let triage_client = create_triage_llm_client(&config)?;
            let agent = default_transformer_agent(
                transformer_client,
                codemod_client,
                scout_client,
                triage_client,
                Arc::clone(&config),
                scope_path.clone().into_iter().collect(),
                scope_path.clone(),
            );

            let scope_line = scope_path
                .as_ref()
                .map(|scope| format!("Scope: only modify files under `{}`.\n", scope.display()))
                .unwrap_or_default();

            let prompt = format!(
                "{EXECUTE_GLOBAL_REFACTOR_TOOL_NAME}\nPHASE_3_5_TRANSFORMER\n\n\
                 Method: {method_name}\n\
                 Refactor instruction: {refactor_instruction}\n\
                 {scope_line}\
                 First gather_refactor_targets for `{method_name}`, then apply_structural_codemod \
                 using the refactor instruction as transformation_rule."
            );

            let original_task = prompt.clone();
            let result =
                AgentLoopOrchestrator::run(&agent, prompt, TRANSFORMER_MAX_ITERATIONS).await?;

            let output =
                if result.is_finished && result.accumulated_data.contains("[TRANSFORMER OK]") {
                    result.accumulated_data
                } else {
                    format!(
                        "Transformer report (finished={}, iterations={}):\n{}",
                        result.is_finished, result.iterations, result.accumulated_data
                    )
                };
            Ok(finish_agent_job_with_eval(
                &config,
                EXECUTE_GLOBAL_REFACTOR_TOOL_NAME,
                &original_task,
                output,
            )
            .await)
        },
    )
    .await
}

pub async fn handle_evaluate_agent_performance(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let target_agent = args
        .get("target_agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "target_agent is required".to_string())?
        .to_string();

    let original_task = args
        .get("original_task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "original_task is required".to_string())?
        .to_string();

    let received_output = args
        .get("received_output")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "received_output is required".to_string())?
        .to_string();
    let received_output = strip_auto_eval_appendix(&received_output).to_string();
    reject_thin_eval_input(&target_agent, &received_output)?;

    dispatch_async_job(
        registry,
        request_uuid,
        EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            let cache_start = mcp_workspace_root();
            let cache_manager = Arc::new(Mutex::new(open_cache_manager_near(&cache_start)?));
            let client = create_evaluator_llm_client(&config)?;
            let agent = EvaluatorAgent::new(
                client,
                cache_manager,
                target_agent,
                original_task,
                received_output,
            );

            let result = AgentLoopOrchestrator::run(
                &agent,
                EVALUATE_AGENT_PERFORMANCE_TOOL_NAME.to_string(),
                EVALUATOR_MAX_ITERATIONS,
            )
            .await?;

            if result.is_finished {
                return Ok(result.accumulated_data);
            }

            Ok(format!(
                "Evaluator report (finished={}, iterations={}):\n{}",
                result.is_finished, result.iterations, result.accumulated_data
            ))
        },
    )
    .await
}

pub async fn handle_web_fetch(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let search_phrase = args
        .get("search_phrase")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|phrase| !phrase.is_empty())
        .ok_or_else(|| "search_phrase is required".to_string())?
        .to_string();
    let force_refresh = args
        .get("force_refresh")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    dispatch_async_job(
        registry,
        request_uuid,
        WEB_FETCH_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            with_phase_auto_compact(Arc::clone(&config), AgentPhase::WebFetcher, || async move {
                let web_profile = config.web_fetcher.clone().unwrap_or_default();
                let cache_manager =
                    Arc::new(Mutex::new(open_cache_manager_near(&mcp_workspace_root())?));
                let reasoning_client = create_web_fetcher_llm_client(&config)?;
                let max_hops = web_profile.max_search_hops;
                let ttl = web_profile.cache_ttl_seconds as i64;
                let cache_threshold = web_profile.web_cache_threshold;

                let agent = WebFetcherAgent::new(reasoning_client, web_profile);
                let output = match run_web_fetch_with_cache(
                    &cache_manager,
                    &agent,
                    &search_phrase,
                    max_hops,
                    ttl,
                    cache_threshold,
                    !force_refresh,
                )
                .await?
                {
                    WebCacheOutcome::Hit(report) => format!("[CACHE HIT]\n{report}"),
                    WebCacheOutcome::Fresh(report) => report,
                };
                Ok(
                    finish_agent_job_with_eval(
                        &config,
                        WEB_FETCH_TOOL_NAME,
                        &search_phrase,
                        output,
                    )
                    .await,
                )
            })
            .await
        },
    )
    .await
}

pub async fn handle_analyze_log(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let log_path_raw = args
        .get("log_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "log_path is required".to_string())?
        .to_string();

    dispatch_async_job(
        registry,
        request_uuid,
        ANALYZE_LOG_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            let root = mcp_workspace_root();
            let config_for_eval = Arc::clone(&config);
            let config_for_blocking = Arc::clone(&config);
            let log_path = log_path_raw.clone();
            let output = tokio::task::spawn_blocking(move || {
                with_thread_workspace_root(root, || {
                    analyze_log_at_path(&config_for_blocking, &log_path, false)
                })
            })
            .await
            .map_err(|err| format!("analyze_log task failed: {err}"))??;
            Ok(finish_agent_job_with_eval(
                &config_for_eval,
                ANALYZE_LOG_TOOL_NAME,
                &log_path_raw,
                output,
            )
            .await)
        },
    )
    .await
}

pub async fn handle_plan_blueprint(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let parsed = parse_plan_blueprint_args(&args)?;

    dispatch_async_job(
        registry,
        request_uuid,
        PLAN_BLUEPRINT_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            with_phase_auto_compact(Arc::clone(&config), AgentPhase::Planner, || async move {
                let coordinator = CoordinatorConstraints::from_args(&parsed);
                let scout_client = create_planner_llm_client(&config)?;
                let emit_client = create_planner_emit_llm_client(&config)?;

                let original_task = parsed.feature_request.clone();
                let result = run_planner_hybrid(scout_client, emit_client, parsed).await?;

                let output = final_blueprint_or_report(&result, &coordinator);
                Ok(finish_agent_job_with_eval(
                    &config,
                    PLAN_BLUEPRINT_TOOL_NAME,
                    &original_task,
                    output,
                )
                .await)
            })
            .await
        },
    )
    .await
}

pub async fn handle_prepare_git_copy(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("generate")
        .to_string();
    let hook_failure = args
        .get("hook_failure_output")
        .and_then(Value::as_str)
        .map(str::to_string);
    let persist_flag = args
        .get("persist_conventions")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let user_instructions = args
        .get("user_instructions")
        .and_then(Value::as_str)
        .map(str::to_string);
    let feature_context = args
        .get("feature_context")
        .and_then(Value::as_str)
        .map(str::to_string);
    let expected_ticket = args
        .get("expected_ticket")
        .and_then(Value::as_str)
        .map(str::to_string);

    if mode == "refine_from_hooks" && hook_failure.as_ref().is_none_or(|s| s.trim().is_empty()) {
        return Err("refine_from_hooks requires hook_failure_output".into());
    }

    let persist_allowed = persist_flag || mode == "update_conventions";

    dispatch_async_job(
        registry,
        request_uuid,
        PREPARE_GIT_COPY_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            let root = mcp_workspace_root();
            let inputs = ScoutInputs {
                feature_context: feature_context.clone(),
                expected_ticket: expected_ticket.clone(),
                user_instructions: user_instructions.clone(),
            };
            let scout = gather_conventions_and_diff(&root, &inputs).await?;
            let scout_block = format_scout_block(&scout);

            let mut prompt = format!(
                "{PREPARE_GIT_COPY_TOOL_NAME}\nmode={mode}\npersist_allowed={persist_allowed}\n\n\
                 {GIT_JANITOR_SYSTEM_PROMPT}\n\n{scout_block}"
            );
            if let Some(instr) = user_instructions.as_deref() {
                prompt.push_str("\n\n## User instructions\n");
                prompt.push_str(instr);
            }
            if let Some(hook) = hook_failure.as_deref() {
                prompt.push_str("\n\n## Hook failure output — revise conventions and regenerate copy\n");
                prompt.push_str(hook);
            }
            if mode == "update_conventions" {
                prompt.push_str(
                    "\n\nMode update_conventions: call update_git_conventions with a patch, then emit_git_copy.",
                );
            }

            let client = create_git_janitor_llm_client(&config)?;
            let agent = GitJanitorAgent::new(client, scout, persist_allowed, root);
            let original_task = prompt.clone();
            let result = run_git_janitor(&agent, prompt).await?;
            let output = if result.is_finished && result.agent_completed {
                result.accumulated_data
            } else {
                format!(
                    "GitJanitor report (finished={}, iterations={}):\n{}",
                    result.is_finished, result.iterations, result.accumulated_data
                )
            };
            Ok(finish_agent_job_with_eval(
                &config,
                PREPARE_GIT_COPY_TOOL_NAME,
                &original_task,
                output,
            )
            .await)
        },
    )
    .await
}

pub async fn handle_create_git_branch(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let workspace_root = require_workspace_root_arg(&args)?;
    let branch_name = args
        .get("branch_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "branch_name is required".to_string())?
        .to_string();

    dispatch_async_job(
        registry,
        request_uuid,
        CREATE_GIT_BRANCH_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
        args.to_string(),
        move || async move {
            let root = mcp_workspace_root();
            let output = create_git_branch(&root, &branch_name).await?;
            Ok(finish_agent_job_with_eval(
                &config,
                CREATE_GIT_BRANCH_TOOL_NAME,
                &format!("create_git_branch {branch_name}"),
                output,
            )
            .await)
        },
    )
    .await
}

/// Output-boundary gate before returning blueprint JSON to the coordinator.
fn final_blueprint_or_report(
    result: &AgentContext,
    coordinator: &CoordinatorConstraints,
) -> String {
    if let Some(json) = extract_json_object(&result.accumulated_data) {
        match validate_blueprint(json)
            .and_then(|bp| validate_blueprint_coordinator(&bp, coordinator).map(|_| bp))
            .and_then(|bp| validate_blueprint_grounding(&bp, &result.touched_files).map(|_| bp))
        {
            Ok(validated) => {
                return serde_json::to_string_pretty(&validated)
                    .unwrap_or_else(|_| validated.to_string());
            }
            Err(reason) => {
                return format!(
                    "Planner report (VALIDATION FAILED after {} iterations):\n\
                     Blueprint rejected at output boundary: {reason}\n\n\
                     Raw accumulated output:\n{}",
                    result.iterations, result.accumulated_data
                );
            }
        }
    }

    format!(
        "Planner report (finished={}, iterations={}):\n{}",
        result.is_finished, result.iterations, result.accumulated_data
    )
}

#[cfg(test)]
mod eval_hook_tests {
    use super::*;

    #[test]
    fn tool_eval_maps_builder() {
        assert_eq!(
            tool_eval_target_agent(GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME),
            Some("Phase_4_Builder")
        );
    }

    #[test]
    fn tool_eval_skips_evaluator_tool() {
        assert!(tool_eval_target_agent(EVALUATE_AGENT_PERFORMANCE_TOOL_NAME).is_none());
    }

    #[test]
    fn tool_eval_maps_babysit_pr() {
        assert_eq!(
            tool_eval_target_agent(BABYSIT_PR_TOOL_NAME),
            Some("BabysitterAgent")
        );
    }

    #[test]
    fn tool_eval_maps_git_janitor_tools() {
        assert_eq!(
            tool_eval_target_agent(PREPARE_GIT_COPY_TOOL_NAME),
            Some("GitJanitorAgent")
        );
        assert_eq!(
            tool_eval_target_agent(CREATE_GIT_BRANCH_TOOL_NAME),
            Some("GitJanitorAgent")
        );
    }
}

#[cfg(test)]
mod boundary_validator_tests {
    use super::final_blueprint_or_report;
    use crate::agent::{AgentContext, CoordinatorConstraints, PlanBlueprintArgs, PlanKind};
    use crate::cache::{resolve_workspace_path, with_thread_workspace_root};
    use std::path::PathBuf;

    fn ctx(data: &str) -> AgentContext {
        AgentContext {
            input_prompt: String::new(),
            accumulated_data: data.to_string(),
            iterations: 10,
            max_iterations: 20,
            is_finished: false,
            agent_completed: false,
            touched_files: Vec::new(),
            last_tool_call: None,
        }
    }

    fn constraints() -> CoordinatorConstraints {
        CoordinatorConstraints::from_args(&PlanBlueprintArgs {
            feature_request: "x".to_string(),
            plan_kind: Some(PlanKind::Feature),
            expectation: Some("surgical patches only".to_string()),
        })
    }

    #[test]
    fn boundary_returns_validated_json_when_completed() {
        let golden = include_str!("../../../tests/fixtures/golden-rate-limit-blueprint.json");
        with_thread_workspace_root(PathBuf::from(env!("CARGO_MANIFEST_DIR")), || {
            let mut c = ctx(golden);
            c.agent_completed = true;
            c.is_finished = true;
            c.touched_files = vec![
                resolve_workspace_path("src/lib.rs"),
                resolve_workspace_path("src/config_server.rs"),
                resolve_workspace_path("Cargo.toml"),
            ];
            let out = final_blueprint_or_report(&c, &CoordinatorConstraints::none());
            assert!(out.contains("\"task_id\""), "{out}");
            assert!(!out.contains("VALIDATION FAILED"), "{out}");
        });
    }

    #[test]
    fn boundary_flags_invalid_json_even_when_agent_completed() {
        let mut c = ctx("{\"task_id\":\"leaked-draft\",\"architecture_summary\":\"x\",\"pipeline\":[{\"step\":1,\"agent\":\"BuilderAgent\",\"action\":\"patch_file\",\"target_file\":\"src/lib.rs\",\"goal\":\"Wire at lib.rs:1.\",\"patch_content\":\"fn x() { ... }\\n\"},{\"step\":2,\"agent\":\"BuilderAgent\",\"action\":\"generate_tests\",\"target_file\":\"tests/x_test.rs\",\"goal\":\".\",\"patch_content\":\"\"}]}");
        c.agent_completed = true;
        c.is_finished = true;
        let out = final_blueprint_or_report(&c, &constraints());
        assert!(out.contains("VALIDATION FAILED"), "{out}");
    }

    #[test]
    fn boundary_flags_invalid_json_on_iteration_cap() {
        // Planner burned iterations, left an ungrounded patch in prose JSON.
        let raw = r#"{"task_id":"leaked-draft","architecture_summary":"x","pipeline":[{"step":1,"agent":"BuilderAgent","action":"patch_file","target_file":"src/lib.rs","goal":"Wire at lib.rs:1.","patch_content":"<<<<<<< SEARCH\n// FABRICATED\n=======\nfn x() { ... }\n>>>>>>> REPLACE\n"},{"step":2,"agent":"BuilderAgent","action":"generate_tests","target_file":"tests/x_test.rs","goal":".","patch_content":""}]}"#;
        let out = final_blueprint_or_report(&ctx(raw), &constraints());
        assert!(out.contains("VALIDATION FAILED"), "{out}");
        assert!(out.contains("output boundary"), "{out}");
    }

    #[test]
    fn boundary_falls_back_to_report_when_no_json() {
        let out = final_blueprint_or_report(&ctx("just prose, no json"), &constraints());
        assert!(out.contains("Planner report"), "{out}");
    }
}

#[cfg(test)]
mod eval_input_guard_tests {
    use super::*;

    #[test]
    fn strip_auto_eval_appendix_drops_tail() {
        let raw = "## Triage: PASS\nCommand: `cargo check`\nExit code: 0\n\n[ADJUTANT AUTO-EVAL APPENDIX — not part of agent output]\nQA score: 10/10\n";
        assert_eq!(
            strip_auto_eval_appendix(raw),
            "## Triage: PASS\nCommand: `cargo check`\nExit code: 0"
        );
    }

    #[test]
    fn reject_thin_triage_pass_paraphrase() {
        let err = reject_thin_eval_input(
            "Phase_5_Triage",
            "## Triage: PASS\n[TRIAGE PASS]\ncargo check exit 0",
        )
        .expect_err("thin");
        assert!(err.contains("paraphrase"));
    }

    #[test]
    fn accept_triage_pass_with_command_evidence() {
        reject_thin_eval_input(
            "Phase_5_Triage",
            "## Triage: PASS\n[TRIAGE PASS]\nCommand: `cargo check`\nExit code: 0\n",
        )
        .expect("ok");
    }
}

#[cfg(test)]
mod dispatch_async_job_tests {
    use super::*;
    use crate::jobs::JobRegistry;

    fn workspace() -> PathBuf {
        std::env::temp_dir()
    }

    #[tokio::test]
    async fn returns_result_inline_when_job_finishes_within_timeout() {
        let registry = JobRegistry::new();
        let out = dispatch_async_job(
            &registry,
            "job-inline".to_string(),
            "scout_context",
            30,
            workspace(),
            String::new(),
            || async { Ok("the answer".to_string()) },
        )
        .await
        .expect("dispatch");

        assert_eq!(out, "the answer");
        assert_eq!(
            registry.query("job-inline").expect("query")["status"],
            "completed"
        );
    }

    #[tokio::test]
    async fn surfaces_job_error_inline() {
        let registry = JobRegistry::new();
        let err = dispatch_async_job(
            &registry,
            "job-err".to_string(),
            "scout_context",
            30,
            workspace(),
            String::new(),
            || async { Err("agent blew up".to_string()) },
        )
        .await
        .expect_err("dispatch should surface job error");

        assert_eq!(err, "agent blew up");
    }

    #[tokio::test]
    async fn falls_back_to_accepted_response_on_timeout() {
        let registry = JobRegistry::new();
        let out = dispatch_async_job(
            &registry,
            "job-slow".to_string(),
            "scout_context",
            0,
            workspace(),
            String::new(),
            || async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok("late result".to_string())
            },
        )
        .await
        .expect("dispatch");

        assert!(out.contains("\"status\": \"accepted\""), "{out}");
        assert!(out.contains("job-slow"), "{out}");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            registry.query("job-slow").expect("query")["status"],
            "completed"
        );
    }
}
