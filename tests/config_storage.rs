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
fn custom_config_roundtrip_preserves_all_fields() {
    let temp_dir = unique_temp_path("custom-roundtrip");
    let config_path = temp_dir.join("nested/config.json");

    let mut config = AdjutantConfig {
        server_port: 9_001,
        storage_path: "/tmp/custom-config.json".to_string(),
        ..Default::default()
    };

    let builder_profile = config.get_profile(&AgentPhase::Builder);
    let openai_id = "openai-custom".to_string();
    config.profiles.insert(
        openai_id.clone(),
        mcp_adjutant::ProviderProfile {
            id: openai_id.clone(),
            name: "OpenAI".into(),
            provider: Provider::OpenAI,
            api_key: Some("sk-test".into()),
            base_url: "https://api.openai.com/v1".into(),
        },
    );
    config.phases.insert(
        AgentPhase::Builder,
        mcp_adjutant::PhaseBinding {
            profile_id: openai_id,
            model_name: "gpt-4o-mini".into(),
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
    assert_ne!(loaded.get_profile(&AgentPhase::Builder), builder_profile);

    std::fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn config_with_transformer_phase_deserializes() {
    let json = r#"{
        "phases": {
            "transformer": {
                "provider": "open_router",
                "api_key": null,
                "base_url": "https://openrouter.ai/api/v1",
                "model_name": "google/gemini-2.5-flash",
                "max_tokens": 4096,
                "temperature": 0.1
            },
            "builder": {
                "provider": "open_router",
                "api_key": null,
                "base_url": "https://openrouter.ai/api/v1",
                "model_name": "google/gemini-2.5-flash",
                "max_tokens": 8192,
                "temperature": 0.2
            }
        },
        "server_port": 3000,
        "storage_path": "/tmp/transformer-phase.json"
    }"#;

    let config = mcp_adjutant::storage::parse_config_json(json).expect("deserialize transformer phase");
    // migrate remaps transformer → pruner when pruner missing
    let pruner = config.get_profile(&AgentPhase::Pruner);
    let builder = config.get_profile(&AgentPhase::Builder);
    assert_eq!(pruner.model_name, "google/gemini-2.5-flash");
    assert_eq!(pruner.base_url, "https://openrouter.ai/api/v1");
    assert_ne!(pruner.max_tokens, builder.max_tokens);
    assert_eq!(pruner.provider, Provider::OpenRouter);
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
fn parse_config_json_migrates_legacy_transformer_phase() {
    let raw = r#"{
        "phases": {
            "scout": {
                "provider": "deep_seek",
                "api_key": null,
                "base_url": "https://api.deepseek.com/v1",
                "model_name": "deepseek-chat",
                "max_tokens": 4096,
                "temperature": 0.3
            },
            "transformer": {
                "provider": "open_router",
                "api_key": null,
                "base_url": "https://openrouter.ai/api/v1",
                "model_name": "legacy-model",
                "max_tokens": 4096,
                "temperature": 0.1
            }
        },
        "server_port": 3000,
        "storage_path": "/tmp/config.json",
        "web_fetcher": {
            "brave_api_key": "BSA-test",
            "max_search_hops": 3,
            "token_budget": 8000,
            "cache_ttl_seconds": 604800,
            "web_cache_threshold": 0.78
        }
    }"#;

    let config = mcp_adjutant::storage::parse_config_json(raw).expect("parse legacy config");
    assert!(config.phases.contains_key(&AgentPhase::Scout));
    assert!(config.phases.contains_key(&AgentPhase::Pruner));
    let pruner = config.get_profile(&AgentPhase::Pruner);
    assert_eq!(pruner.model_name, "legacy-model");
    assert_eq!(
        config
            .web_fetcher
            .as_ref()
            .and_then(|profile| profile.brave_api_key.as_deref()),
        Some("BSA-test")
    );
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

#[test]
fn parse_config_json_copies_builder_to_planner_when_missing() {
    let raw = r#"{
        "phases": {
            "builder": {
                "provider": "open_router",
                "api_key": null,
                "base_url": "https://openrouter.ai/api/v1",
                "model_name": "google/gemini-3.1-flash-lite",
                "max_tokens": 8192,
                "temperature": 0.2
            }
        },
        "server_port": 3000,
        "storage_path": "/tmp/config.json"
    }"#;

    let config = mcp_adjutant::storage::parse_config_json(raw).expect("parse config");
    let planner = config.get_profile(&AgentPhase::Planner);
    let builder = config.get_profile(&AgentPhase::Builder);
    assert_eq!(planner.model_name, builder.model_name);
    assert_eq!(planner.base_url, builder.base_url);
    assert_eq!(planner.max_tokens, builder.max_tokens);
}
