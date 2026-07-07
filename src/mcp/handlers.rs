use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{
    default_builder_agent, AgentLoopOrchestrator, ScoutAgent, SystemBuildRunner, TriageAgent,
    TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::ProjectCacheManager;
use crate::domain::AdjutantConfig;
use crate::llm::{create_builder_llm_client, create_scout_llm_client, create_triage_llm_client};
use crate::tools::LlmBuildDiscoverer;

pub const SCOUT_CONTEXT_TOOL_NAME: &str = "scout_context";
pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";
pub const GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME: &str = "generate_tests_and_scaffolding";

const SCOUT_MAX_ITERATIONS: u32 = 10;
const TRIAGE_MAX_ITERATIONS: u32 = 3;
const BUILDER_MAX_ITERATIONS: u32 = 5;

pub fn scout_context_schema() -> Value {
    json!({
        "name": SCOUT_CONTEXT_TOOL_NAME,
        "description": "Uruchamia autonomiczny zwiad kodu i zwraca skondensowany kontekst markdown.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Pytanie lub cel zwiadu po repozytorium."
                }
            },
            "required": ["query"]
        }
    })
}

pub fn verify_and_triage_schema() -> Value {
    json!({
        "name": VERIFY_AND_TRIAGE_TOOL_NAME,
        "description": "Uruchamia analizę błędów kompilacji/typowania i automatycznie naprawia trywialne usterki. Wywołuj ZAWSZE po zmianach w kodzie przed napisaniem commitu.",
        "input_schema": {
            "type": "object",
            "properties": {
                "target_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Lista ścieżek do sprawdzanych plików. Jeśli puste, agent sprawdzi git status."
                }
            }
        }
    })
}

pub fn generate_tests_and_scaffolding_schema() -> Value {
    json!({
        "name": GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
        "description": "Generuje testy (jednostkowe/integracyjne) i fabryki. Automatycznie sprawdza kompilację przez triage.",
        "input_schema": {
            "type": "object",
            "properties": {
                "source_file_path": { "type": "string" },
                "test_type": {
                    "type": "string",
                    "enum": ["unit", "integration", "factory"]
                }
            },
            "required": ["source_file_path", "test_type"]
        }
    })
}

pub fn registered_mcp_tools() -> Vec<Value> {
    vec![
        scout_context_schema(),
        verify_and_triage_schema(),
        generate_tests_and_scaffolding_schema(),
    ]
}

pub async fn handle_scout_context(
    args: Value,
    config: Arc<AdjutantConfig>,
) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .ok_or_else(|| "query is required".to_string())?
        .to_string();

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
}

pub async fn handle_verify_and_triage(
    args: Value,
    config: Arc<AdjutantConfig>,
) -> Result<String, String> {
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
            .contains("Wszystkie testy/kompilacje zakończone sukcesem.")
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
) -> Result<String, String> {
    let source_file_path = args
        .get("source_file_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "source_file_path is required".to_string())?;

    let test_type = args
        .get("test_type")
        .and_then(Value::as_str)
        .ok_or_else(|| "test_type is required".to_string())?;

    const ALLOWED_TEST_TYPES: [&str; 3] = ["unit", "integration", "factory"];
    if !ALLOWED_TEST_TYPES.contains(&test_type) {
        return Err(format!(
            "test_type must be one of {ALLOWED_TEST_TYPES:?}, got {test_type:?}"
        ));
    }

    let source_path = PathBuf::from(source_file_path);
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
        "{GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME}\nPHASE_4_BUILDER\n\nWygeneruj test typu `{test_type}` dla pliku: {source_file_path}"
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
}
