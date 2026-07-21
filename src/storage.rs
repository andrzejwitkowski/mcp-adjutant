use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Map, Value};

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

/// Legacy phase keys + flat PhaseProfile → shared profiles + PhaseBinding.
pub fn migrate_config_value(value: &mut Value) {
    let Some(phases) = value.get_mut("phases").and_then(Value::as_object_mut) else {
        return;
    };

    // Legacy flat-phase configs used "transformer" for what is now "pruner".
    // Only rename when it's a legacy flat binding (has a "provider" key directly on the phase),
    // not a modern profile-based config where "transformer" is its own distinct agent phase.
    let transformer_is_legacy = phases
        .get("transformer")
        .and_then(Value::as_object)
        .is_some_and(|obj| obj.contains_key("provider"));
    if transformer_is_legacy && !phases.contains_key("pruner") {
        if let Some(transformer) = phases.remove("transformer") {
            phases.insert("pruner".to_string(), transformer);
        }
    }

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
    migrate_flat_phases_to_profiles(value);
}

fn normalize_provider(raw: &str) -> String {
    match raw {
        "deepseek" => "deep_seek".into(),
        other => other.to_string(),
    }
}

fn migrate_flat_phases_to_profiles(value: &mut Value) {
    let Some(root) = value.as_object_mut() else {
        return;
    };
    if root
        .get("profiles")
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty())
    {
        return;
    }
    let Some(phases) = root.get("phases").and_then(Value::as_object) else {
        return;
    };
    let is_legacy = phases.values().any(|v| v.get("provider").is_some());
    if !is_legacy {
        return;
    }

    let mut profiles = Map::new();
    let mut dedupe: HashMap<(String, String, String), String> = HashMap::new();
    let mut new_phases = Map::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut next = 0usize;

    for (phase_name, phase_val) in phases {
        let Some(obj) = phase_val.as_object() else {
            continue;
        };
        let provider = normalize_provider(
            obj.get("provider")
                .and_then(Value::as_str)
                .unwrap_or("deep_seek"),
        );
        let base_url = obj
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("https://api.deepseek.com/v1")
            .to_string();
        let api_key_val = obj.get("api_key").cloned().unwrap_or(Value::Null);
        let api_key_str = api_key_val.as_str().unwrap_or("").to_string();
        let key = (provider.clone(), base_url.clone(), api_key_str);

        let profile_id = if let Some(id) = dedupe.get(&key) {
            id.clone()
        } else {
            next += 1;
            let id = if next == 1 {
                "default".to_string()
            } else {
                format!("profile-{next}")
            };
            dedupe.insert(key, id.clone());
            let name = match provider.as_str() {
                "open_router" if next == 1 => "OpenRouter Default".into(),
                "open_router" => format!("OpenRouter {next}"),
                "deep_seek" if next == 1 => "DeepSeek Default".into(),
                "deep_seek" => format!("DeepSeek {next}"),
                "open_ai" if next == 1 => "OpenAI Default".into(),
                "open_ai" => format!("OpenAI {next}"),
                _ if next == 1 => format!("{provider} Default"),
                _ => format!("{provider} {next}"),
            };
            profiles.insert(
                id.clone(),
                json!({
                    "id": id,
                    "name": name,
                    "provider": provider,
                    "api_key": api_key_val,
                    "base_url": base_url,
                }),
            );
            id
        };
        *counts.entry(profile_id.clone()).or_insert(0) += 1;

        new_phases.insert(
            phase_name.clone(),
            json!({
                "profile_id": profile_id,
                "model_name": obj.get("model_name").cloned().unwrap_or(json!("deepseek-chat")),
                "max_tokens": obj.get("max_tokens").cloned().unwrap_or(json!(4096)),
                "temperature": obj.get("temperature").cloned().unwrap_or(json!(0.2)),
            }),
        );
    }

    // Stable tie-break: highest use count, then prefer id "default", else lexicographic min.
    let default_id = counts
        .into_iter()
        .min_by_key(|(id, n)| (std::cmp::Reverse(*n), id.as_str() != "default", id.clone()))
        .map(|(id, _)| id)
        .or_else(|| profiles.keys().next().cloned())
        .unwrap_or_else(|| "default".into());

    root.insert("profiles".to_string(), Value::Object(profiles));
    root.insert("default_profile_id".to_string(), json!(default_id));
    root.insert("phases".to_string(), Value::Object(new_phases));
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
            phases.get("planner_emit").and_then(|v| v.get("model_name")),
            phases.get("builder").and_then(|v| v.get("model_name"))
        );
        assert!(value.get("profiles").and_then(|p| p.as_object()).is_some());
        assert!(phases
            .get("builder")
            .and_then(|v| v.get("profile_id"))
            .is_some());
    }

    #[test]
    fn migrate_config_value_seeds_planner_emit_from_planner_when_no_builder() {
        let mut value = json!({
            "phases": {
                "scout": { "model_name": "scout-model", "provider": "deep_seek", "base_url": "https://api.deepseek.com/v1", "api_key": null, "max_tokens": 1, "temperature": 0.0 },
                "planner": { "model_name": "planner-model", "provider": "deep_seek", "base_url": "https://api.deepseek.com/v1", "api_key": null, "max_tokens": 1, "temperature": 0.0 }
            }
        });
        migrate_config_value(&mut value);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert_eq!(
            phases.get("planner_emit").and_then(|v| v.get("model_name")),
            phases.get("planner").and_then(|v| v.get("model_name"))
        );
    }

    #[test]
    fn migrate_config_value_scout_builder_inherits_builder_for_planner_emit() {
        let mut value = json!({
            "phases": {
                "scout": { "model_name": "scout-model", "provider": "deep_seek", "base_url": "u", "api_key": null, "max_tokens": 1, "temperature": 0.0 },
                "builder": { "model_name": "builder-coder", "provider": "deep_seek", "base_url": "u", "api_key": null, "max_tokens": 1, "temperature": 0.0 }
            }
        });
        migrate_config_value(&mut value);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert_eq!(
            phases.get("planner_emit").and_then(|v| v.get("model_name")),
            Some(&json!("builder-coder"))
        );
        assert_eq!(
            phases.get("planner").and_then(|v| v.get("model_name")),
            Some(&json!("scout-model"))
        );
    }

    #[test]
    fn migrate_config_value_preserves_modern_transformer_phase() {
        // Modern profile-based config: transformer must NOT be renamed to pruner.
        let mut value = json!({
            "profiles": { "p1": { "id": "p1", "name": "OR", "provider": "open_router", "api_key": null, "base_url": "https://openrouter.ai/api/v1" } },
            "phases": {
                "transformer": { "profile_id": "p1", "model_name": "qwen/qwen3-235b-a22b", "max_tokens": 8192, "temperature": 0.1 },
                "scout":       { "profile_id": "p1", "model_name": "qwen/qwen3.6-35b-a3b", "max_tokens": 4096, "temperature": 0.3 }
            }
        });
        migrate_config_value(&mut value);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert!(
            phases.contains_key("transformer"),
            "transformer must survive in modern configs"
        );
        assert!(
            !phases.contains_key("pruner") || phases.contains_key("transformer"),
            "pruner should not steal transformer's binding"
        );
    }

    #[test]
    fn migrate_config_value_renames_legacy_transformer_to_pruner() {
        // Legacy flat config: transformer (with provider key) should become pruner.
        let mut value = json!({
            "phases": {
                "transformer": { "provider": "deep_seek", "api_key": null, "base_url": "https://api.deepseek.com/v1", "model_name": "deepseek-coder", "max_tokens": 8192, "temperature": 0.1 },
                "scout":       { "provider": "deep_seek", "api_key": null, "base_url": "https://api.deepseek.com/v1", "model_name": "deepseek-chat",  "max_tokens": 4096, "temperature": 0.3 }
            }
        });
        migrate_config_value(&mut value);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert!(
            phases.contains_key("pruner"),
            "legacy transformer should be renamed pruner"
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
        assert!(phases
            .get("git_janitor")
            .and_then(|v| v.get("profile_id"))
            .is_some());
        assert_eq!(
            phases.get("git_janitor").and_then(|v| v.get("model_name")),
            Some(&json!("qwen/qwen3.6-35b-a3b"))
        );
        assert!(!phases.contains_key("unknown_phase"));
        let profiles = value.get("profiles").unwrap().as_object().unwrap();
        assert!(profiles
            .values()
            .any(|p| p.get("api_key") == Some(&json!("sk-test-janitor"))));
    }

    #[test]
    fn migrate_dedupes_identical_credentials() {
        let mut value = json!({
            "phases": {
                "scout": {
                    "provider": "open_router",
                    "api_key": "sk-1",
                    "base_url": "https://openrouter.ai/api/v1",
                    "model_name": "a",
                    "max_tokens": 1,
                    "temperature": 0.0
                },
                "triage": {
                    "provider": "open_router",
                    "api_key": "sk-1",
                    "base_url": "https://openrouter.ai/api/v1",
                    "model_name": "b",
                    "max_tokens": 2,
                    "temperature": 0.1
                }
            }
        });
        migrate_config_value(&mut value);
        let profiles = value.get("profiles").unwrap().as_object().unwrap();
        assert_eq!(profiles.len(), 1);
        let phases = value.get("phases").unwrap().as_object().unwrap();
        assert_eq!(
            phases.get("scout").and_then(|v| v.get("profile_id")),
            phases.get("triage").and_then(|v| v.get("profile_id"))
        );
    }
}
