use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{
    default_builder_agent, AgentLoopOrchestrator, EvaluatorAgent, ScoutAgent, SystemBuildRunner,
    TriageAgent, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::ProjectCacheManager;
use crate::domain::AdjutantConfig;
use crate::jobs::{
    accepted_job_response, parse_request_uuid, query_job_status_schema,
    request_uuid_schema_property, run_tracked_job, JobRegistry,
};
use crate::llm::{
    create_builder_llm_client, create_evaluator_llm_client, create_scout_llm_client,
    create_triage_llm_client,
};
use crate::tools::LlmBuildDiscoverer;

pub const SCOUT_CONTEXT_TOOL_NAME: &str = "scout_context";
pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";
pub const GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME: &str = "generate_tests_and_scaffolding";
pub const EVALUATE_AGENT_PERFORMANCE_TOOL_NAME: &str = "evaluate_agent_performance";

const SCOUT_MAX_ITERATIONS: u32 = 10;
const TRIAGE_MAX_ITERATIONS: u32 = 3;
const BUILDER_MAX_ITERATIONS: u32 = 5;
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
        query_job_status_schema(),
    ]
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
            let client = create_scout_llm_client(&config)?;
            let agent = ScoutAgent::new(client);
            let result = AgentLoopOrchestrator::run(&agent, query, SCOUT_MAX_ITERATIONS).await?;

            if result.is_finished {
                return Ok(result.accumulated_data);
            }

            Ok(format!(
                "Scout report (finished={}, iterations={}):\n{}",
                result.is_finished, result.iterations, result.accumulated_data
            ))
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
                .filter_map(|item| item.as_str().map(PathBuf::from))
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
            let source_path = PathBuf::from(&source_file_path);
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

            let prompt = format!(
                "{GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME}\nPHASE_4_BUILDER\n\nGenerate a `{test_type}` test for file: {source_file_path}"
            );

            let result = AgentLoopOrchestrator::run(&agent, prompt, BUILDER_MAX_ITERATIONS).await?;

            if result.is_finished {
                return Ok(format!(
                    "Builder finished successfully for {source_file_path} ({test_type}).\n{}",
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
        .unwrap_or_else(|| PathBuf::from("."));

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
