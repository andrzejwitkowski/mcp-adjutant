use serde_json::json;

#[test]
fn test_migrate_config_value_transformer_to_pruner() {
    let mut config_json = json!({
        "phases": {
            "transformer": { "model": "gpt-4" }
        }
    });

    mcp_adjutant::storage::migrate_config_value(&mut config_json);

    let phases = config_json.get("phases").unwrap().as_object().unwrap();

    assert!(
        phases.contains_key("pruner"),
        "transformer should migrate to pruner"
    );
    assert!(
        !phases.contains_key("transformer"),
        "Should have removed transformer"
    );
    assert_eq!(phases["pruner"]["model"], "gpt-4");
}

#[test]
fn test_migrate_config_value_missing_planner_inherits_builder() {
    let mut config_json = json!({
        "phases": {
            "builder": { "model": "claude-3" }
        }
    });

    mcp_adjutant::storage::migrate_config_value(&mut config_json);

    let phases = config_json.get("phases").unwrap().as_object().unwrap();

    assert!(
        phases.contains_key("planner"),
        "planner should inherit builder"
    );
    assert_eq!(phases["planner"]["model"], "claude-3");
}
