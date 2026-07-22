use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::schemas::{
    ANALYZE_LOG_TOOL_NAME, BABYSIT_PR_TOOL_NAME, CREATE_GIT_BRANCH_TOOL_NAME,
    EVALUATE_AGENT_PERFORMANCE_TOOL_NAME, EXECUTE_GLOBAL_REFACTOR_TOOL_NAME,
    GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME, PLAN_BLUEPRINT_TOOL_NAME, PREPARE_GIT_COPY_TOOL_NAME,
    SCOUT_CONTEXT_TOOL_NAME, TRANSPILE_TYPES_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
    WEB_FETCH_TOOL_NAME,
};
use crate::agent::{
    analyze_log_at_path, builder_task_parts, create_git_branch, default_builder_agent,
    default_transformer_agent, default_verify_workspace, embed_source_files, extract_json_object,
    format_babysitter_result, format_builder_report, format_eval_job_appendix, format_scout_block,
    format_triage_success, gather_conventions_and_diff, parse_plan_blueprint_args,
    parse_transpile_types_args, run_git_janitor, run_planner_hybrid, run_scout_with_cache,
    run_web_fetch_with_cache, triage_passed, validate_blueprint, validate_blueprint_coordinator,
    validate_blueprint_grounding, AgentContext, AgentEvalSummary, AgentLoopOrchestrator,
    BabysitterAgent, BuilderReportInput, CoordinatorConstraints, EvaluatorAgent, GitJanitorAgent,
    ScoutAgent, ScoutCacheOutcome, ScoutInputs, SystemBuildRunner, TranspilerAgent, TriageAgent,
    WebCacheOutcome, WebFetcherAgent, BABYSITTER_MAX_ITERATIONS, BABYSITTER_SYSTEM_PROMPT,
    BUILDER_GREEN_MARKER, GIT_JANITOR_SYSTEM_PROMPT, TRANSFORMER_MAX_ITERATIONS,
    TRANSPILER_MAX_ITERATIONS, TRANSPILER_SYSTEM_PROMPT, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::{
    load_best_desired_output_exemplar, mcp_workspace_root, open_cache_connection,
    require_workspace_root_arg, resolve_workspace_path, with_thread_workspace_root,
    ProjectCacheManager,
};
use crate::domain::AdjutantConfig;
use crate::jobs::{accepted_job_response, parse_request_uuid, run_tracked_job, JobRegistry};
use crate::llm::{
    create_babysitter_llm_client, create_builder_llm_client, create_evaluator_llm_client,
    create_git_janitor_llm_client, create_planner_emit_llm_client, create_planner_llm_client,
    create_scout_llm_client, create_transformer_llm_client, create_triage_llm_client,
    create_web_fetcher_llm_client,
};
use crate::tools::{assert_on_pr_head_branch, gh_pr_state, LlmBuildDiscoverer};

const SOURCE_EMBED_MAX_BYTES: usize = 64 * 1024;

const SCOUT_MAX_ITERATIONS: u32 = 10;
const TRIAGE_MAX_ITERATIONS: u32 = 3;
const BUILDER_MAX_ITERATIONS: u32 = 8;
const EVALUATOR_MAX_ITERATIONS: u32 = 1;

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
        ANALYZE_LOG_TOOL_NAME => Some("LogAnalyzerAgent"),
        PREPARE_GIT_COPY_TOOL_NAME | CREATE_GIT_BRANCH_TOOL_NAME => Some("GitJanitorAgent"),
        EVALUATE_AGENT_PERFORMANCE_TOOL_NAME => None,
        _ => None,
    }
}

// ponytail: sync one-shot eval inside job closure — no extra async job UUID
async fn eval_after_agent_job(
    config: &AdjutantConfig,
    target_agent: &str,
    original_task: &str,
    received_output: &str,
) -> Option<AgentEvalSummary> {
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

async fn finish_agent_job_with_eval(
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

async fn dispatch_async_job<F, Fut>(
    registry: &JobRegistry,
    request_uuid: String,
    tool_name: &str,
    await_timeout_secs: u64,
    workspace_root: PathBuf,
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
        run_tracked_job(job_registry, request_uuid, tool, Some(workspace_root), work).await;
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
    let status = registry.query(&request_uuid)?;
    serde_json::to_string_pretty(&status).map_err(|err| format!("serialize status: {err}"))
}

fn verify_npm_test_passes(test_path: &Path, project_root: &Path) -> Result<String, String> {
    let frontend = project_root.join("frontend");
    let rel = test_path
        .strip_prefix(&frontend)
        .or_else(|_| test_path.strip_prefix(project_root))
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| test_path.to_string_lossy().into_owned());

    let output = Command::new("npm")
        .args(["test", "--", &rel])
        .current_dir(&frontend)
        .output()
        .map_err(|err| format!("failed to run npm test in frontend: {err}"))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(format!("npm test -- {rel}: passed\n{combined}"))
    } else {
        Err(format!("npm test -- {rel} failed:\n{combined}"))
    }
}

fn verify_test_passes(test_path: &Path, project_root: &Path) -> Result<String, String> {
    match test_path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => verify_cargo_test_passes(test_path),
        Some("ts" | "tsx") => verify_npm_test_passes(test_path, project_root),
        _ => Ok(String::new()),
    }
}

fn extract_green_test_path(log: &str) -> Option<PathBuf> {
    log.lines()
        .filter(|line| line.contains("[SYSTEM]: Launching Triage (green)"))
        .filter_map(|line| line.split(" for ").nth(1))
        .map(str::trim)
        .map(PathBuf::from)
        .next_back()
}

fn verify_cargo_test_passes(test_path: &Path) -> Result<String, String> {
    let project_root = mcp_workspace_root();
    let stem = test_path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid test path: {}", test_path.display()))?;

    let output = Command::new("cargo")
        .args(["test", "--test", stem])
        .current_dir(&project_root)
        .output()
        .map_err(|err| format!("failed to run cargo test --test {stem}: {err}"))?;

    if output.status.success() {
        return Ok(format!("cargo test --test {stem}: all tests passed"));
    }

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Err(format!("cargo test --test {stem} failed:\n{combined}"))
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
        move || async move {
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
            Ok(finish_agent_job_with_eval(&config, SCOUT_CONTEXT_TOOL_NAME, &query, result).await)
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
        move || async move {
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

fn open_cache_manager_near(source_path: &Path) -> Result<ProjectCacheManager, String> {
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
        move || async move {
            let source_path = resolve_workspace_path(&source_file_path);
            let cache_manager = Arc::new(Mutex::new(open_cache_manager_near(&source_path)?));

            let builder_client = create_builder_llm_client(&config)?;
            let scout_client = create_scout_llm_client(&config)?;
            let triage_client = create_triage_llm_client(&config)?;
            let agent = default_builder_agent(
                builder_client,
                cache_manager,
                scout_client,
                triage_client,
                Arc::clone(&config),
                vec![source_path.clone()],
            );

            let source_excerpt = std::fs::read_to_string(&source_path)
                .map(|contents| {
                    const MAX: usize = 8_000;
                    if contents.len() > MAX {
                        format!("{}...\n(truncated)", &contents[..MAX])
                    } else {
                        contents
                    }
                })
                .unwrap_or_else(|err| format!("(could not read source file: {err})"));

            let project_root = mcp_workspace_root();
            let parts =
                builder_task_parts(&source_path, &test_type, &source_file_path, &project_root);

            let mut prompt = format!(
                "{GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME}\nPHASE_4_BUILDER\n\n{}",
                parts.workflow
            );
            if let Ok((_, conn)) = open_cache_connection(&project_root) {
                if let Ok(Some(exemplar)) =
                    load_best_desired_output_exemplar(&conn, "Phase_4_Builder")
                {
                    prompt.push_str("\n\n## 10/10 output exemplar (match this report shape)\n");
                    prompt.push_str(&exemplar);
                }
            }
            if !parts.exemplar.is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(&parts.exemplar);
            }
            prompt.push_str(&format!("\n\nSource excerpt:\n```\n{source_excerpt}\n```"));

            let original_task = prompt.clone();
            let result = AgentLoopOrchestrator::run(&agent, prompt, BUILDER_MAX_ITERATIONS).await?;

            let green_marker = result.accumulated_data.contains(BUILDER_GREEN_MARKER);
            let builder_hard_stopped = result.iterations >= BUILDER_MAX_ITERATIONS
                && result.accumulated_data.contains("iteration limit after");

            let (green_ok, verify_summary) =
                if result.is_finished && green_marker && !builder_hard_stopped {
                    match extract_green_test_path(&result.accumulated_data) {
                        None => (
                            false,
                            "builder GREEN but no test path found in log".to_string(),
                        ),
                        Some(test_path) => match verify_test_passes(&test_path, &project_root) {
                            Ok(summary) => (true, summary),
                            Err(err) => (false, format!("post-GREEN verify failed: {err}")),
                        },
                    }
                } else {
                    (false, String::new())
                };

            let output = format_builder_report(&BuilderReportInput {
                accumulated_data: &result.accumulated_data,
                project_root: &project_root,
                source_file_path: &source_file_path,
                test_type: &test_type,
                green_ok,
                verify_summary: (!verify_summary.is_empty()).then_some(verify_summary.as_str()),
            });
            Ok(finish_agent_job_with_eval(
                &config,
                GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
                &original_task,
                output,
            )
            .await)
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
        move || async move {
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

    dispatch_async_job(
        registry,
        request_uuid,
        EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
        config.job_await_timeout_secs,
        workspace_root,
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
        move || async move {
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
                finish_agent_job_with_eval(&config, WEB_FETCH_TOOL_NAME, &search_phrase, output)
                    .await,
            )
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
        move || async move {
            let pr_state = gh_pr_state(pr_number)?;
            assert_on_pr_head_branch(&pr_state.head_ref_name)?;

            let babysitter_client = create_babysitter_llm_client(&config)?;
            let triage_client = create_triage_llm_client(&config)?;
            let discoverer = LlmBuildDiscoverer::new(create_triage_llm_client(&config)?);
            let triage_agent = TriageAgent::with_build_runner_and_discoverer(
                triage_client,
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
            if let Ok((_, conn)) = open_cache_connection(&mcp_workspace_root()) {
                if let Ok(Some(exemplar)) =
                    load_best_desired_output_exemplar(&conn, "BabysitterAgent")
                {
                    prompt.push_str("\n\n## 10/10 output exemplar (match this JSON shape)\n");
                    prompt.push_str(&exemplar);
                }
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
        move || async move {
            let resolved_sources: Vec<PathBuf> = parsed
                .source_paths
                .iter()
                .map(resolve_workspace_path)
                .collect();
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
            let triage_client = create_triage_llm_client(&config)?;

            let original_task = prompt.clone();
            let result = if verify_command.is_some() {
                let triage_agent = TriageAgent::with_build_runner(
                    triage_client,
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
                let discoverer = LlmBuildDiscoverer::new(create_triage_llm_client(&config)?);
                let triage_agent = TriageAgent::with_build_runner_and_discoverer(
                    triage_client,
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
            Ok(
                finish_agent_job_with_eval(
                    &config,
                    TRANSPILE_TYPES_TOOL_NAME,
                    &original_task,
                    output,
                )
                .await,
            )
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
        move || async move {
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
    use crate::cache::resolve_workspace_path;

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
        let golden = include_str!("../../tests/fixtures/golden-rate-limit-blueprint.json");
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
