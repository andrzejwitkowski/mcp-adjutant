use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use mcp_adjutant::config_server::load_or_default;
use mcp_adjutant::{AdjutantConfig, AdjutantConfigError, AgentPhase, Provider};

fn unique_temp_path(test_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();

    std::env::temp_dir().join(format!("mcp-adjutant-{test_name}-{nanos}"))
}

#[test]
fn default_config_roundtrip_preserves_all_fields() {
    let temp_dir = unique_temp_path("default-roundtrip");
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let config_path = temp_dir.join("config.json");

    let original = AdjutantConfig::default();
    original
        .save_to_file(&config_path)
        .expect("save default config");

    let loaded = AdjutantConfig::load_from_file(&config_path).expect("load saved config");
    assert_eq!(original, loaded);

    std::fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn load_or_default_merges_missing_phases_from_defaults() {
    let temp_dir = unique_temp_path("legacy-config");
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let config_path = temp_dir.join("legacy.json");

    let legacy_json = r#"{
        "phases": {
            "scout": {
                "provider": "deepseek",
                "api_key": null,
                "base_url": "https://api.deepseek.com/v1",
                "model_name": "deepseek-chat",
                "max_tokens": 4096,
                "temperature": 0.3
            }
        },
        "server_port": 3000,
        "storage_path": "/tmp/legacy.json"
    }"#;
    std::fs::write(&config_path, legacy_json).expect("write legacy config");

    let loaded = load_or_default(&config_path);
    loaded
        .try_get_profile(AgentPhase::Evaluator)
        .expect("evaluator profile merged from defaults");

    std::fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn load_or_default_preserves_custom_evaluator_profile_when_present() {
    let temp_dir = unique_temp_path("custom-evaluator-config");
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let config_path = temp_dir.join("custom.json");

    let mut config = AdjutantConfig::default();
    config.phases.insert(
        AgentPhase::Evaluator,
        mcp_adjutant::PhaseProfile {
            provider: Provider::OpenAI,
            api_key: Some("sk-evaluator".to_string()),
            base_url: "https://api.openai.com/v1".to_string(),
            model_name: "gpt-4o-mini".to_string(),
            max_tokens: 1_024,
            temperature: 0.1,
        },
    );
    config.save_to_file(&config_path).expect("save config");

    let loaded = load_or_default(&config_path);
    let evaluator = loaded
        .try_get_profile(AgentPhase::Evaluator)
        .expect("evaluator profile");

    assert_eq!(evaluator.model_name, "gpt-4o-mini");
    assert_eq!(evaluator.provider, Provider::OpenAI);
    assert_eq!(evaluator.max_tokens, 1_024);

    std::fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn custom_config_roundtrip_preserves_all_fields() {
    let temp_dir = unique_temp_path("custom-roundtrip");
    let config_path = temp_dir.join("nested/config.json");

    let mut config = AdjutantConfig {
        server_port: 9_001,
        storage_path: "/tmp/custom-config.json".to_string(),
        ..Default::default()
    };

    let builder_profile = config.get_profile(&AgentPhase::Builder).clone();
    config.phases.insert(
        AgentPhase::Builder,
        mcp_adjutant::PhaseProfile {
            provider: Provider::OpenAI,
            api_key: Some("sk-test".to_string()),
            base_url: "https://api.openai.com/v1".to_string(),
            model_name: "gpt-4o-mini".to_string(),
            max_tokens: 16_384,
            temperature: 0.5,
        },
    );

    config
        .save_to_file(&config_path)
        .expect("save custom config");
    assert!(config_path.exists());

    let loaded = AdjutantConfig::load_from_file(&config_path).expect("load custom config");
    assert_eq!(config, loaded);
    assert_ne!(loaded.get_profile(&AgentPhase::Builder), &builder_profile);

    std::fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn load_from_corrupted_json_returns_parse_error() {
    let temp_dir = unique_temp_path("corrupted-json");
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let config_path = temp_dir.join("broken.json");
    std::fs::write(&config_path, "{ not valid json").expect("write broken json");

    let error = AdjutantConfig::load_from_file(&config_path).expect_err("expected parse error");
    match error {
        AdjutantConfigError::Json(_) => {}
        other => panic!("expected JSON error, got {other:?}"),
    }

    std::fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn save_creates_missing_parent_directories() {
    let temp_dir = unique_temp_path("nested-save");
    let config_path = temp_dir.join("deep/nested/dir/config.json");

    let config = AdjutantConfig::default();
    config
        .save_to_file(&config_path)
        .expect("save into nested path");

    assert!(config_path.is_file());

    std::fs::remove_dir_all(&temp_dir).ok();
}
