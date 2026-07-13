use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{
    analyze_log_at_path, default_builder_agent, default_transformer_agent,
    default_verify_workspace, embed_source_files, format_triage_success,
    parse_transpile_types_args, run_scout_with_cache, run_web_fetch_with_cache, triage_passed,
    AgentLoopOrchestrator, BabysitterAgent, EvaluatorAgent, ScoutAgent, ScoutCacheOutcome,
    SystemBuildRunner, TranspilerAgent, TriageAgent, WebCacheOutcome, WebFetcherAgent,
    BABYSITTER_MAX_ITERATIONS, BABYSITTER_SYSTEM_PROMPT, TRANSFORMER_MAX_ITERATIONS,
    TRANSPILER_MAX_ITERATIONS, TRANSPILER_SYSTEM_PROMPT, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::{mcp_workspace_root, resolve_workspace_path, ProjectCacheManager};
use crate::domain::AdjutantConfig;
use crate::jobs::{
    accepted_job_response, parse_request_uuid, query_job_status_schema,
    request_uuid_schema_property, run_tracked_job, JobRegistry,
};
use crate::llm::{
    create_babysitter_llm_client, create_builder_llm_client, create_evaluator_llm_client,
    create_scout_llm_client, create_transformer_llm_client, create_triage_llm_client,
    create_web_fetcher_llm_client,
};
use crate::tools::{assert_on_pr_head_branch, gh_pr_state, LlmBuildDiscoverer};

pub const SCOUT_CONTEXT_TOOL_NAME: &str = "scout_context";
pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";
pub const GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME: &str = "generate_tests_and_scaffolding";
pub const EXECUTE_GLOBAL_REFACTOR_TOOL_NAME: &str = "execute_global_refactor";
pub const EVALUATE_AGENT_PERFORMANCE_TOOL_NAME: &str = "evaluate_agent_performance";
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";
pub const ANALYZE_LOG_TOOL_NAME: &str = "analyze_log";
pub const BABYSIT_PR_TOOL_NAME: &str = "babysit_pr";
pub const TRANSPILE_TYPES_TOOL_NAME: &str = "transpile_types";

const SOURCE_EMBED_MAX_BYTES: usize = 64 * 1024;

const SCOUT_MAX_ITERATIONS: u32 = 10;
const TRIAGE_MAX_ITERATIONS: u32 = 3;
const BUILDER_MAX_ITERATIONS: u32 = 8;
const EVALUATOR_MAX_ITERATIONS: u32 = 1;

pub fn scout_context_schema() -> Value {
    json!({
        "name": SCOUT_CONTEXT_TOOL_NAME,
        "description": "Runs autonomous code scouting and returns condensed markdown context. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Question or scouting goal for the repository."
                },
                "force_refresh": {
                    "type": "boolean",
                    "description": "When true, bypass the semantic cache lookup and scout fresh. Successful runs are still stored in the cache."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["query", "request_uuid"]
        }
    })
}

pub fn verify_and_triage_schema() -> Value {
    json!({
        "name": VERIFY_AND_TRIAGE_TOOL_NAME,
        "description": "Runs compile/type error analysis and automatically fixes trivial issues. ALWAYS call after code changes before committing. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "target_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Paths to check. If empty, the agent uses git status."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["request_uuid"]
        }
    })
}

pub fn generate_tests_and_scaffolding_schema() -> Value {
    json!({
        "name": GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
        "description": "Generates unit/integration tests and factories. Automatically verifies compilation via triage. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "source_file_path": { "type": "string" },
                "test_type": {
                    "type": "string",
                    "enum": ["unit", "integration", "factory"]
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["source_file_path", "test_type", "request_uuid"]
        }
    })
}

pub fn execute_global_refactor_schema() -> Value {
    json!({
        "name": EXECUTE_GLOBAL_REFACTOR_TOOL_NAME,
        "description": "Call when changing a method signature, struct name, or propagating a type change across many files. Scout finds call sites; Triage verifies compilation. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "method_name": {
                    "type": "string",
                    "description": "Method or struct whose signature/call sites change."
                },
                "refactor_instruction": {
                    "type": "string",
                    "description": "What must change at each call site?"
                },
                "scope_path": {
                    "type": "string",
                    "description": "Optional directory scope; only files under this path are gathered, codemodded, verified, and triaged."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["method_name", "refactor_instruction", "request_uuid"]
        }
    })
}

pub fn evaluate_agent_performance_schema() -> Value {
    json!({
        "name": EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
        "description": "Evaluate the quality of a report or code produced by another agent (e.g. Scout or Builder). Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "target_agent": {
                    "type": "string",
                    "description": "Name of the agent you are evaluating (e.g. 'Phase_1_Scout')."
                },
                "original_task": {
                    "type": "string",
                    "description": "What exactly did you expect from this agent?"
                },
                "received_output": {
                    "type": "string",
                    "description": "Raw result/report the agent returned."
                },
                "project_path": {
                    "type": "string",
                    "description": "Optional project file or directory path where the evaluation is stored (defaults to the MCP working directory)."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["target_agent", "original_task", "received_output", "request_uuid"]
        }
    })
}

pub fn registered_mcp_tools() -> Vec<Value> {
    vec![
        scout_context_schema(),
        verify_and_triage_schema(),
        generate_tests_and_scaffolding_schema(),
        execute_global_refactor_schema(),
        evaluate_agent_performance_schema(),
        web_fetch_schema(),
        analyze_log_schema(),
        babysit_pr_schema(),
        transpile_types_schema(),
        query_job_status_schema(),
    ]
}

pub fn web_fetch_schema() -> Value {
    json!({
        "name": WEB_FETCH_TOOL_NAME,
        "description": "Fetches the latest authoritative web content for a search phrase as compacted markdown. Works for any topic - documentation, news, specs, comparisons, code examples, or any web research. The agent searches via Brave Search API, fetches top result pages, and returns a condensed report. Results are cached semantically. Requires brave_api_key in web_fetcher config. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "search_phrase": {
                    "type": "string",
                    "description": "Topic or search phrase to research on the web."
                },
                "force_refresh": {
                    "type": "boolean",
                    "description": "When true, bypass the semantic cache lookup and fetch fresh web content. Successful runs are still stored in the cache."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["search_phrase", "request_uuid"]
        }
    })
}

async fn dispatch_async_job<F, Fut>(
    registry: &JobRegistry,
    request_uuid: String,
    tool_name: &str,
    work: F,
) -> Result<String, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<String, String>> + Send + 'static,
{
    registry.register(&request_uuid, tool_name)?;
    let registry = registry.clone();
    let accepted_uuid = request_uuid.clone();
    let tool = tool_name.to_string();
    tokio::spawn(async move {
        run_tracked_job(registry, request_uuid, tool, work).await;
    });
    Ok(accepted_job_response(&accepted_uuid, tool_name))
}

pub async fn handle_query_job_status(
    args: Value,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let status = registry.query(&request_uuid)?;
    serde_json::to_string_pretty(&status).map_err(|err| format!("serialize status: {err}"))
}

fn integration_test_exemplar() -> String {
    let path = mcp_workspace_root().join("tests/cache_manager_tests.rs");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let excerpt = if lines.len() > 47 {
        lines[6..47].join("\n")
    } else {
        content
    };
    format!(
        "Golden integration-test pattern (copy this setup — do not use tempfile):\n```rust\n{excerpt}\n```"
    )
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
        move || async move {
            let cache_manager =
                Arc::new(Mutex::new(open_cache_manager_near(&mcp_workspace_root())?));
            let client = create_scout_llm_client(&config)?;
            let agent = ScoutAgent::new(client);
            match run_scout_with_cache(
                &cache_manager,
                &agent,
                &query,
                SCOUT_MAX_ITERATIONS,
                !force_refresh,
            )
            .await?
            {
                ScoutCacheOutcome::Hit(report) => Ok(format!("[CACHE HIT]\n{report}")),
                ScoutCacheOutcome::Fresh(report) => Ok(report),
            }
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
    let target_paths: Vec<PathBuf> = args
        .get("target_paths")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(resolve_workspace_path))
                .collect()
        })
        .unwrap_or_default();

    dispatch_async_job(
        registry,
        request_uuid,
        VERIFY_AND_TRIAGE_TOOL_NAME,
        move || async move {
            let triage_client = create_triage_llm_client(&config)?;
            let scout_client = create_scout_llm_client(&config)?;
            let discoverer = LlmBuildDiscoverer::new(scout_client);
            let agent = TriageAgent::with_build_runner_and_discoverer(
                triage_client,
                target_paths,
                Arc::clone(&config),
                SystemBuildRunner,
                discoverer,
            );

            let result = AgentLoopOrchestrator::run(
                &agent,
                format!("{VERIFY_AND_TRIAGE_TOOL_NAME}\n\n{TRIAGE_SYSTEM_PROMPT}"),
                TRIAGE_MAX_ITERATIONS,
            )
            .await?;

            if result.is_finished {
                if triage_passed(&result) {
                    return Ok(format_triage_success(&result));
                }
                if !result.accumulated_data.is_empty()
                    && !result.accumulated_data.starts_with("Triage targets")
                {
                    return Ok(result.accumulated_data);
                }
            }

            Ok(format!(
                "Triage report (finished={}, iterations={}):\n{}\n{}",
                result.is_finished, result.iterations, result.input_prompt, result.accumulated_data
            ))
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
        move || async move {
            let source_path = resolve_workspace_path(&source_file_path);
            let cache_manager = Arc::new(Mutex::new(open_cache_manager_near(&source_path)?));

            let builder_client = create_builder_llm_client(&config)?;
            // ponytail: builder sub-agents use builder phase model (same as parent agent)
            let scout_client = create_builder_llm_client(&config)?;
            let triage_client = create_builder_llm_client(&config)?;
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

            let exemplar = integration_test_exemplar();

            let prompt = format!(
                "{GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME}\nPHASE_4_BUILDER\n\n\
                 Generate a `{test_type}` test for file: {source_file_path}\n\n\
                 Write ONE test function first to a **new** file `tests/<name>_integration_test.rs` — never overwrite `tests/cache_manager_tests.rs`.\n\
                 Workflow: write_test_suite(tdd_phase=red) then write_test_suite(tdd_phase=green). Job succeeds only when GREEN passes.\n\
                 Use `mod common;` and helpers from `tests/common/mod.rs` — do not add new dev-dependencies.\n\
                 Direct SQLite checks use `project_root.join(\".adjutant/cache.db\")` — never `cache.sqlite`.\n\
                 Integration test crates cannot use `crate::` — import via `mcp_adjutant::...`.\n\n\
                 {exemplar}\n\n\
                 Source excerpt:\n```\n{source_excerpt}\n```"
            );

            let result = AgentLoopOrchestrator::run(&agent, prompt, BUILDER_MAX_ITERATIONS).await?;

            let green_ok = result.accumulated_data.contains("[BUILDER GREEN OK]");
            let builder_hard_stopped = result.iterations >= BUILDER_MAX_ITERATIONS
                && result.accumulated_data.contains("iteration limit after");

            if result.is_finished && green_ok && !builder_hard_stopped {
                let test_path = extract_green_test_path(&result.accumulated_data)
                    .ok_or_else(|| "builder finished GREEN but no test path found in log".to_string())?;
                let cargo_summary = verify_cargo_test_passes(&test_path)?;
                return Ok(format!(
                    "Builder finished successfully for {source_file_path} ({test_type}). {cargo_summary}\n{}",
                    result.accumulated_data
                ));
            }

            Ok(format!(
                "Builder report (finished={}, iterations={}):\n{}\n{}",
                result.is_finished, result.iterations, result.input_prompt, result.accumulated_data
            ))
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
    let scope_path = args
        .get("scope_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(resolve_workspace_path);

    dispatch_async_job(
        registry,
        request_uuid,
        EXECUTE_GLOBAL_REFACTOR_TOOL_NAME,
        move || async move {
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

            let result =
                AgentLoopOrchestrator::run(&agent, prompt, TRANSFORMER_MAX_ITERATIONS).await?;

            if result.is_finished && result.accumulated_data.contains("[TRANSFORMER OK]") {
                return Ok(result.accumulated_data);
            }

            Ok(format!(
                "Transformer report (finished={}, iterations={}):\n{}",
                result.is_finished, result.iterations, result.accumulated_data
            ))
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

    let cache_start = args
        .get("project_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(mcp_workspace_root);

    dispatch_async_job(
        registry,
        request_uuid,
        EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
        move || async move {
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
        move || async move {
            let web_profile = config.web_fetcher.clone().unwrap_or_default();
            let cache_manager =
                Arc::new(Mutex::new(open_cache_manager_near(&mcp_workspace_root())?));
            let reasoning_client = create_web_fetcher_llm_client(&config)?;
            let max_hops = web_profile.max_search_hops;
            let ttl = web_profile.cache_ttl_seconds as i64;
            let cache_threshold = web_profile.web_cache_threshold;

            let agent = WebFetcherAgent::new(reasoning_client, web_profile);
            match run_web_fetch_with_cache(
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
                WebCacheOutcome::Hit(report) => Ok(format!("[CACHE HIT]\n{report}")),
                WebCacheOutcome::Fresh(report) => Ok(report),
            }
        },
    )
    .await
}

pub fn analyze_log_schema() -> Value {
    json!({
        "name": ANALYZE_LOG_TOOL_NAME,
        "description": "Reads a log file or remote log source and triages the first root cause (what failed and where). Supports local paths, https:// URLs, and gh-run:<run_id> for GitHub Actions. ALWAYS call first when investigating logs, crash output, CI logs, or searching for errors in log files. Built-in parsers run first; cheap LLM fallback when needed. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "log_path": {
                    "type": "string",
                    "description": "Local workspace or absolute file path, https:// log URL, or gh-run:<run_id> for GitHub Actions failed-job logs."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["log_path", "request_uuid"]
        }
    })
}

pub async fn handle_analyze_log(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
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
        move || async move {
            let config = Arc::clone(&config);
            let log_path = log_path_raw.clone();
            tokio::task::spawn_blocking(move || analyze_log_at_path(&config, &log_path, false))
                .await
                .map_err(|err| format!("analyze_log task failed: {err}"))?
        },
    )
    .await
}

pub fn babysit_pr_schema() -> Value {
    json!({
        "name": BABYSIT_PR_TOOL_NAME,
        "description": "Runs the BabysitterAgent loop (max 20 turns) to drive a GitHub PR toward mergeable state: CI green and actionable reviews fixed. Requires `gh` CLI, authenticated `gh auth login`, and local checkout on the PR head branch. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "pr_number": {
                    "type": "integer",
                    "description": "GitHub pull request number in the current repository."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["pr_number", "request_uuid"]
        }
    })
}

pub async fn handle_babysit_pr(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let pr_number = args
        .get("pr_number")
        .and_then(Value::as_u64)
        .ok_or_else(|| "pr_number is required".to_string())?;

    dispatch_async_job(
        registry,
        request_uuid,
        BABYSIT_PR_TOOL_NAME,
        move || async move {
            let pr_state = gh_pr_state(pr_number)?;
            assert_on_pr_head_branch(&pr_state.head_ref_name)?;

            let babysitter_client = create_babysitter_llm_client(&config)?;
            let triage_client = create_triage_llm_client(&config)?;
            let scout_client = create_scout_llm_client(&config)?;
            let discoverer = LlmBuildDiscoverer::new(scout_client);
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

            let prompt =
                format!("{BABYSIT_PR_TOOL_NAME}\nPR #{pr_number}\n\n{BABYSITTER_SYSTEM_PROMPT}");
            let result =
                AgentLoopOrchestrator::run(&agent, prompt, BABYSITTER_MAX_ITERATIONS).await?;

            if result.is_finished && result.agent_completed {
                return Ok(result.accumulated_data);
            }

            Ok(format!(
                "Babysitter report (finished={}, iterations={}):\n{}",
                result.is_finished, result.iterations, result.accumulated_data
            ))
        },
    )
    .await
}

pub fn transpile_types_schema() -> Value {
    json!({
        "name": TRANSPILE_TYPES_TOOL_NAME,
        "description": "Sync API types/DTOs across languages via TranspilerAgent. Returns immediately; fetch via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "source_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Source-language files containing API types/DTOs."
                },
                "target_path": {
                    "type": "string",
                    "description": "Primary target-language output file (created or overwritten)."
                },
                "architecture_layout": {
                    "type": "string",
                    "description": "Coordinator wish: idiom mapping, file layout, symbol grouping, re-export strategy, validation libs, wire-format naming."
                },
                "preserve_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files the agent must not overwrite."
                },
                "verify_workspace": {
                    "type": "string",
                    "description": "Directory to run verify_command in (default: parent of target_path or repo root)."
                },
                "verify_command": {
                    "type": "string",
                    "description": "Optional verification shell command (e.g. npm run typecheck, cargo check, mypy pkg). Triage auto-discovers when omitted."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["source_paths", "target_path", "architecture_layout", "request_uuid"]
        }
    })
}

pub async fn handle_transpile_types(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let parsed = parse_transpile_types_args(&args)?;

    dispatch_async_job(
        registry,
        request_uuid,
        TRANSPILE_TYPES_TOOL_NAME,
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
                let scout_client = create_scout_llm_client(&config)?;
                let discoverer = LlmBuildDiscoverer::new(scout_client);
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

            if result.is_finished && result.agent_completed {
                return Ok(result.accumulated_data);
            }

            Ok(format!(
                "Transpiler report (finished={}, iterations={}):\n{}",
                result.is_finished, result.iterations, result.accumulated_data
            ))
        },
    )
    .await
}
