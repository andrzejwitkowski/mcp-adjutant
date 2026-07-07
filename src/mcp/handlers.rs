use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{AgentLoopOrchestrator, ScoutAgent, SystemBuildRunner, TriageAgent, TRIAGE_SYSTEM_PROMPT};
use crate::domain::AdjutantConfig;
use crate::llm::{create_scout_llm_client, create_triage_llm_client};
use crate::tools::LlmBuildDiscoverer;

pub const SCOUT_CONTEXT_TOOL_NAME: &str = "scout_context";
pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";

const SCOUT_MAX_ITERATIONS: u32 = 10;
const TRIAGE_MAX_ITERATIONS: u32 = 3;

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

pub fn registered_mcp_tools() -> Vec<Value> {
    vec![scout_context_schema(), verify_and_triage_schema()]
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
