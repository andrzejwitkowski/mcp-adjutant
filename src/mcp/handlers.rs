use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{
    default_builder_agent, run_scout_with_cache, AgentLoopOrchestrator, EvaluatorAgent, ScoutAgent,
    ScoutCacheOutcome, SystemBuildRunner, TriageAgent, WebFetcherAgent, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::{mcp_workspace_root, resolve_workspace_path, ProjectCacheManager};
use crate::domain::AdjutantConfig;
use crate::jobs::{
    accepted_job_response, parse_request_uuid, query_job_status_schema,
    request_uuid_schema_property, run_tracked_job, JobRegistry,
};
use crate::llm::{
    create_builder_llm_client, create_evaluator_llm_client, create_llm_client,
    create_scout_llm_client, create_triage_llm_client, create_web_fetcher_llm_client,
};
use crate::tools::LlmBuildDiscoverer;

pub const SCOUT_CONTEXT_TOOL_NAME: &str = "scout_context";
pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";
pub const GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME: &str = "generate_tests_and_scaffolding";
pub const EVALUATE_AGENT_PERFORMANCE_TOOL_NAME: &str = "evaluate_agent_performance";
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";

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
        evaluate_agent_performance_schema(),
        web_fetch_schema(),
        query_job_status_schema(),
    ]
}

pub fn web_fetch_schema() -> Value {
    json!({
        "name": WEB_FETCH_TOOL_NAME,
        "description": "Fetches the latest web documentation for a search phrase as compacted markdown. The agent searches the live web via a browsing model and returns a condensed report. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "search_phrase": {
                    "type": "string",
                    "description": "Topic or search phrase to research on the web."
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
    tokio::spawn(async move {
        run_tracked_job(registry, request_uuid, work).await;
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

    dispatch_async_job(
        registry,
        request_uuid,
        SCOUT_CONTEXT_TOOL_NAME,
        move || async move {
            let cache_manager =
                Arc::new(Mutex::new(open_cache_manager_near(&mcp_workspace_root())?));
            let client = create_scout_llm_client(&config)?;
            let agent = ScoutAgent::new(client);
            match run_scout_with_cache(&cache_manager, &agent, &query, SCOUT_MAX_ITERATIONS, true)
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
                if result
                    .input_prompt
                    .contains("All builds/tests completed successfully.")
                {
                    return Ok(result.input_prompt);
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

    dispatch_async_job(
        registry,
        request_uuid,
        WEB_FETCH_TOOL_NAME,
        move || async move {
            let web_profile = config.web_fetcher.clone().unwrap_or_default();

            let reasoning_client = create_web_fetcher_llm_client(&config)?;
            let browsing_client = create_llm_client(web_profile.browsing.clone())?;
            // ponytail: prefer configured hop count, clamped to a sane [1, 10] range.
            let max_hops = web_profile.max_search_hops.clamp(1, 10);

            let agent = WebFetcherAgent::new(reasoning_client, browsing_client, web_profile);
            let result =
                AgentLoopOrchestrator::run(&agent, search_phrase.clone(), max_hops).await?;

            if result.is_finished {
                return Ok(result.accumulated_data);
            }
            Ok(format!(
                "Web fetch report (finished={}, iterations={}):\n{}",
                result.is_finished, result.iterations, result.accumulated_data
            ))
        },
    )
    .await
}
