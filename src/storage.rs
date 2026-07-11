use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::domain::AdjutantConfig;
use crate::error::AdjutantConfigError;

const KNOWN_PHASES: &[&str] = &[
    "scout",
    "pruner",
    "builder",
    "triage",
    "babysitter",
    "evaluator",
    "web_fetcher",
];

pub fn load_from_file(path: &Path) -> Result<AdjutantConfig, AdjutantConfigError> {
    let contents = fs::read_to_string(path)?;
    parse_config_json(&contents)
}

pub fn parse_config_json(contents: &str) -> Result<AdjutantConfig, AdjutantConfigError> {
    let mut value: Value = serde_json::from_str(contents)?;
    migrate_config_value(&mut value);
    Ok(serde_json::from_value(value)?)
}

/// Strips legacy/unknown phase keys so serde can map phases to `AgentPhase`.
pub fn migrate_config_value(value: &mut Value) {
    let Some(phases) = value.get_mut("phases").and_then(Value::as_object_mut) else {
        return;
    };

    // Legacy UI used "transformer" before pruner existed in the config schema.
    if let Some(transformer) = phases.remove("transformer") {
        phases.entry("pruner".to_string()).or_insert(transformer);
    }

    phases.retain(|key, _| KNOWN_PHASES.contains(&key.as_str()));
}

pub fn save_to_file(config: &AdjutantConfig, path: &Path) -> Result<(), AdjutantConfigError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let contents = serde_json::to_string_pretty(config)?;
    fs::write(path, contents)?;
    Ok(())
}
