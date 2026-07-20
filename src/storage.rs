use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::domain::AdjutantConfig;
use crate::error::AdjutantConfigError;

const KNOWN_PHASES: &[&str] = &[
    "scout",
    "pruner",
    "builder",
    "transformer",
    "triage",
    "babysitter",
    "evaluator",
    "log_analyzer",
    "web_fetcher",
    "planner",
    "planner_emit",
    "git_janitor",
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
    if phases.contains_key("transformer") && !phases.contains_key("pruner") {
        if let Some(transformer) = phases.remove("transformer") {
            phases.insert("pruner".to_string(), transformer);
        }
    }

    // New installs often have scout/builder tuned but no planner rows yet.
    if !phases.contains_key("planner") {
        if let Some(scout) = phases.get("scout").cloned() {
            phases.insert("planner".to_string(), scout);
        } else if let Some(builder) = phases.get("builder").cloned() {
            phases.insert("planner".to_string(), builder);
        }
    }

    if !phases.contains_key("planner_emit") {
        if let Some(builder) = phases.get("builder").cloned() {
            phases.insert("planner_emit".to_string(), builder);
        } else if let Some(planner) = phases.get("planner").cloned() {
            phases.insert("planner_emit".to_string(), planner);
        }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::migrate_config_value;

    #[test]
    fn migrate_config_value_seeds_planner_emit_from_builder() {
        let mut value = json!({
            "phases": {
                "builder": {
                    "provider": "deepseek",
                    "api_key": "sk-test",
                    "base_url": "https://api.deepseek.com/v1",
                    "model_name": "deepseek-coder",
                    "max_tokens": 8192,
                    "temperature": 0.2
                }
            }
        });
        migrate_config_value(&mut value);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert!(phases.contains_key("planner_emit"));
        assert_eq!(
            phases.get("planner_emit").unwrap(),
            phases.get("builder").unwrap()
        );
    }

    #[test]
    fn migrate_config_value_seeds_planner_emit_from_planner_when_no_builder() {
        let mut value = json!({
            "phases": {
                "scout": { "model_name": "scout-model" },
                "planner": { "model_name": "planner-model" }
            }
        });
        migrate_config_value(&mut value);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert_eq!(
            phases.get("planner_emit").unwrap(),
            phases.get("planner").unwrap()
        );
    }

    #[test]
    fn migrate_config_value_keeps_git_janitor_phase() {
        let mut value = json!({
            "phases": {
                "git_janitor": {
                    "provider": "open_router",
                    "api_key": "sk-test-janitor",
                    "base_url": "https://openrouter.ai/api/v1",
                    "model_name": "qwen/qwen3.6-35b-a3b",
                    "max_tokens": 4096,
                    "temperature": 0.2
                },
                "unknown_phase": { "model_name": "drop-me" }
            }
        });
        migrate_config_value(&mut value);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert!(phases.contains_key("git_janitor"));
        assert_eq!(
            phases.get("git_janitor").and_then(|v| v.get("api_key")),
            Some(&json!("sk-test-janitor"))
        );
        assert!(!phases.contains_key("unknown_phase"));
    }
}
