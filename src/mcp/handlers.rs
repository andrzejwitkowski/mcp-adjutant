use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{AgentLoopOrchestrator, TriageAgent, TRIAGE_SYSTEM_PROMPT};
use crate::domain::AdjutantConfig;
use crate::llm::create_triage_llm_client;

pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";

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
    vec![verify_and_triage_schema()]
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

    let client = create_triage_llm_client(&config)?;
    let agent = TriageAgent::new(client, target_paths, Arc::clone(&config));

    let result = AgentLoopOrchestrator::run(
        &agent,
        format!("{VERIFY_AND_TRIAGE_TOOL_NAME}\n\n{TRIAGE_SYSTEM_PROMPT}"),
        3,
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
