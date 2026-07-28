//! Integration: handle_execute_blueprint applies a validated blueprint and green-triages.
//! No live LLM required when cargo check passes on first triage turn (auto-eval fails open).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::handle_execute_blueprint;
use mcp_adjutant::jobs::JobRegistry;
use serde_json::json;

fn setup_compiling_crate(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("Cargo.toml");
    std::fs::write(root.join("src/lib.rs"), "pub mod agent;\n").expect("lib.rs");
    std::fs::write(root.join("src/agent.rs"), "pub fn ping() -> u8 { 1 }\n").expect("agent.rs");
}

fn unique_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("adjutant-exec-bp-{label}-{nanos}"))
}

#[tokio::test]
async fn execute_blueprint_applies_patch_and_passes_triage() {
    std::env::set_var("MCP_ADJUTANT_SKIP_PREFLIGHT", "1");
    std::env::set_var("MCP_ADJUTANT_TEST_SKIP_GENERATE_TESTS", "1");

    let root = unique_root("happy");
    setup_compiling_crate(&root);

    let blueprint = json!({
        "task_id": "exec-limit-mod",
        "architecture_summary": "Add limit module and wire it in lib.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "create_file",
            "target_file": "src/limit.rs",
            "goal": "New module at src/limit.rs:1.",
            "patch_content": "pub fn max() -> u32 { 60 }\n"
        }, {
            "step": 2,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Declare limit at src/lib.rs:1.",
            "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub mod limit;\n>>>>>>> REPLACE\n"
        }, {
            "step": 3,
            "agent": "BuilderAgent",
            "action": "generate_tests",
            "target_file": "tests/limit_test.rs",
            "goal": "Cover src/limit.rs:1.",
            "patch_content": ""
        }]
    });

    let config = Arc::new(AdjutantConfig {
        job_await_timeout_secs: 180,
        ..Default::default()
    });
    let registry = JobRegistry::new();
    let request_uuid = format!(
        "exec-bp-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let args = json!({
        "workspace_root": root.to_string_lossy(),
        "request_uuid": request_uuid,
        "blueprint": blueprint.to_string(),
    });

    let out = handle_execute_blueprint(args, Arc::clone(&config), &registry)
        .await
        .expect("execute_blueprint");

    assert!(
        out.contains("[BLUEPRINT EXECUTE]") && out.contains("task_id: exec-limit-mod"),
        "missing execute header: {out}"
    );
    assert!(
        out.contains("## Applied file steps (2)"),
        "expected two applied steps: {out}"
    );
    assert!(
        out.contains("## Triage") && (out.contains("PASS") || out.contains("[TRIAGE PASS]")),
        "expected triage pass: {out}"
    );
    assert!(
        out.contains("generate_tests") && out.contains("skipped"),
        "expected test-skip marker: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "pub mod agent;\npub mod limit;\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/limit.rs")).unwrap(),
        "pub fn max() -> u32 { 60 }\n"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn execute_blueprint_rolls_back_when_apply_fails_mid_pipeline() {
    std::env::set_var("MCP_ADJUTANT_SKIP_PREFLIGHT", "1");
    std::env::set_var("MCP_ADJUTANT_TEST_SKIP_GENERATE_TESTS", "1");

    let root = unique_root("rollback");
    setup_compiling_crate(&root);
    let lib_before = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();

    let blueprint = json!({
        "task_id": "exec-rollback",
        "architecture_summary": "Intentionally break apply after create.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "create_file",
            "target_file": "src/ghost.rs",
            "goal": "Create at src/ghost.rs:1.",
            "patch_content": "pub fn ghost() {}\n"
        }, {
            "step": 2,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Bad SEARCH at src/lib.rs:1.",
            "patch_content": "<<<<<<< SEARCH\npub mod missing;\n=======\npub mod missing;\npub mod ghost;\n>>>>>>> REPLACE\n"
        }, {
            "step": 3,
            "agent": "BuilderAgent",
            "action": "generate_tests",
            "target_file": "tests/ghost_test.rs",
            "goal": "Cover src/ghost.rs:1.",
            "patch_content": ""
        }]
    });

    let config = Arc::new(AdjutantConfig {
        job_await_timeout_secs: 60,
        ..Default::default()
    });
    let registry = JobRegistry::new();
    let request_uuid = format!(
        "exec-rb-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let args = json!({
        "workspace_root": root.to_string_lossy(),
        "request_uuid": request_uuid,
        "blueprint": blueprint.to_string(),
    });

    let err = handle_execute_blueprint(args, Arc::clone(&config), &registry)
        .await
        .expect_err("apply should fail");

    assert!(
        err.contains("SEARCH not found") || err.contains("not found"),
        "unexpected error: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        lib_before
    );
    assert!(
        !root.join("src/ghost.rs").exists(),
        "created file should roll back"
    );

    let _ = std::fs::remove_dir_all(&root);
}
