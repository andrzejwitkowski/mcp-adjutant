use super::emit::EmitBlueprintTool;
use super::validate::{extract_json_object, is_comment_sketch, is_kebab_case};
use super::{
    planner_emit_tool_set, planner_scout_tool_set, validate_blueprint,
    validate_blueprint_coordinator, validate_blueprint_grounding,
};
use crate::agent::planner::constraints::CoordinatorConstraints;
use crate::agent::planner::{PlanBlueprintArgs, PlanKind};
use crate::cache::resolve_workspace_path;
use crate::llm::LlmTool;
use serde_json::json;

#[test]
fn extract_json_object_finds_first_object_span() {
    let text = "prefix {\"a\":1} suffix";
    assert_eq!(extract_json_object(text), Some("{\"a\":1}"));
}

#[test]
fn test_validate_blueprint_invalid_json() {
    let result = validate_blueprint("not json");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not valid JSON"));
}

#[test]
fn test_validate_blueprint_missing_fields() {
    let invalid_blueprint = json!({
        "task_id": "my-task"
    });
    let result = validate_blueprint(&invalid_blueprint.to_string());
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .contains("missing or non-string 'architecture_summary'"));
}

fn lib_rs_hunk(replace_extra: &str) -> String {
    format!(
        "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\n{replace_extra}>>>>>>> REPLACE"
    )
}

fn domain_adjutant_config_hunk(replace_extra: &str) -> String {
    format!(
        "<<<<<<< SEARCH\npub struct AdjutantConfig {{\n=======\npub struct AdjutantConfig {{\n{replace_extra}>>>>>>> REPLACE"
    )
}

fn valid_blueprint() -> String {
    let domain_patch = domain_adjutant_config_hunk("    pub cache_ttl: Option<u64>,\n");
    let lib_patch = lib_rs_hunk("pub mod cache_layer;\n");
    format!(
        r#"{{
        "task_id": "add-cache-layer",
        "architecture_summary": "Wrap the fetch fn in an in-memory cache.",
        "pipeline": [
            {{
                "step": 1,
                "agent": "BuilderAgent",
                "action": "create_file",
                "target_file": "src/cache_layer.rs",
                "goal": "New cache module at cache_layer.rs:1.",
                "patch_content": "pub struct Cache {{}}\n"
            }},
            {{
                "step": 2,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "Declare cache_layer at lib.rs:1.",
                "patch_content": {lib_patch_json}
            }},
            {{
                "step": 3,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/domain.rs",
                "goal": "Add cache field to config at domain.rs:84.",
                "patch_content": {domain_patch_json}
            }},
            {{
                "step": 4,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/cache_config_test.rs",
                "goal": "Assert cache config roundtrip at domain.rs:84.",
                "patch_content": ""
            }}
        ]
    }}"#,
        lib_patch_json = serde_json::to_string(&lib_patch).unwrap(),
        domain_patch_json = serde_json::to_string(&domain_patch).unwrap()
    )
}

#[test]
fn planner_scout_tool_set_excludes_emit_blueprint() {
    let tools = planner_scout_tool_set();
    let names: Vec<_> = tools
        .definitions()
        .into_iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(!names.contains(&"emit_blueprint".to_string()));
    assert!(names.contains(&"extract_search_anchor".to_string()));
}

#[test]
fn planner_emit_tool_set_includes_emit_and_read_file_only() {
    let tools = planner_emit_tool_set(CoordinatorConstraints::none());
    let names: Vec<_> = tools
        .definitions()
        .into_iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(
        names,
        vec!["read_file".to_string(), "emit_blueprint".to_string()]
    );
}

#[test]
fn hybrid_tool_sets_cover_scout_and_emit() {
    let scout_names: Vec<_> = planner_scout_tool_set()
        .definitions()
        .into_iter()
        .map(|t| t.name.clone())
        .collect();
    let emit_names: Vec<_> = planner_emit_tool_set(CoordinatorConstraints::none())
        .definitions()
        .into_iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(!scout_names.contains(&"emit_blueprint".to_string()));
    assert!(scout_names.contains(&"extract_search_anchor".to_string()));
    assert_eq!(
        emit_names,
        vec!["read_file".to_string(), "emit_blueprint".to_string()]
    );
}

#[test]
fn validate_accepts_well_formed_blueprint() {
    let result = validate_blueprint(&valid_blueprint());
    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn validate_allows_empty_patch_content_for_sync_types() {
    let raw = r#"{
        "task_id": "sync-dtos",
        "architecture_summary": "Sync user types.",
        "pipeline": [{
            "step": 1,
            "agent": "TranspilerAgent",
            "action": "sync_types",
            "target_file": "frontend/src/modules/config-ui/types.ts",
            "goal": "Transpile user.rs types at src/domain.rs:11.",
            "patch_content": ""
        }]
    }"#;
    assert!(validate_blueprint(raw).is_ok());
}

#[test]
fn validate_rejects_non_json_input() {
    assert!(validate_blueprint("not json at all").is_err());
    assert!(validate_blueprint("{ broken").is_err());
}

#[test]
fn validate_rejects_non_kebab_task_id() {
    let raw = valid_blueprint().replace("\"add-cache-layer\"", "\"AddCacheLayer\"");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("kebab-case"), "{err}");
}

#[test]
fn validate_rejects_empty_pipeline() {
    let raw = r#"{
        "task_id": "no-steps",
        "architecture_summary": "x",
        "pipeline": []
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("at least one step"), "{err}");
}

#[test]
fn validate_rejects_invalid_agent_value() {
    let raw = valid_blueprint().replace("\"BuilderAgent\"", "\"ScoutAgent\"");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("'agent' must be one of"), "{err}");
}

#[test]
fn validate_rejects_triage_agent_in_pipeline() {
    let raw = valid_blueprint().replace("\"BuilderAgent\"", "\"TriageAgent\"");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("TriageAgent is not a blueprint step"), "{err}");
}

#[test]
fn validate_rejects_wrong_agent_for_sync_types() {
    let raw = r#"{
        "task_id": "sync-wrong",
        "architecture_summary": "x",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "sync_types",
            "target_file": "frontend/src/modules/config-ui/types.ts",
            "goal": "Sync DTOs at types.ts:3.",
            "patch_content": ""
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("cannot perform action"), "{err}");
}

#[test]
fn validate_rejects_invalid_action_value() {
    let raw = valid_blueprint().replace("\"create_file\"", "\"run_tests\"");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("'action' must be one of"), "{err}");
}

#[test]
fn validate_rejects_placeholder_in_patch_content() {
    let raw = valid_blueprint().replace("pub struct Cache {}", "// implement logic here");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("placeholder"), "{err}");
}

#[test]
fn validate_rejects_comment_sketch_patch() {
    let patch = lib_rs_hunk("// split api router here\n// add rate limit layer\n");
    let raw = format!(
        r#"{{
        "task_id": "bad-sketch",
        "architecture_summary": "x",
        "pipeline": [
            {{
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "wire at lib.rs:1",
                "patch_content": {patch_json}
            }},
            {{
                "step": 2,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/sketch_test.rs",
                "goal": "Cover.",
                "patch_content": ""
            }}
        ]
    }}"#,
        patch_json = serde_json::to_string(&patch).unwrap()
    );
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("comment sketch"), "{err}");
}

#[test]
fn validate_rejects_ellipsis_in_patch() {
    let raw = valid_blueprint().replace("pub struct Cache {}", "fn run() { ... }");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("ellipsis"), "{err}");
}

#[test]
fn validate_rejects_empty_patch_for_create_file() {
    let raw = r#"{
        "task_id": "empty-patch",
        "architecture_summary": "x",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "create_file",
            "target_file": "src/x.rs",
            "goal": "create x",
            "patch_content": ""
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("required for action"), "{err}");
}

#[test]
fn validate_rejects_nonempty_patch_for_generate_tests() {
    let raw = r#"{
        "task_id": "bad-gen",
        "architecture_summary": "x",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "generate_tests",
            "target_file": "tests/domain_test.rs",
            "goal": "add tests",
            "patch_content": "fn test_x() {}"
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("must be empty"), "{err}");
}

#[test]
fn validate_rejects_missing_target_file_on_disk() {
    let raw = r#"{
        "task_id": "missing-file",
        "architecture_summary": "x",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/no_such_module.rs",
            "goal": "patch it",
            "patch_content": "fn x() {}\n"
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("target_file not found"), "{err}");
}

#[test]
fn validate_rejects_missing_patch_content_key() {
    let raw = r#"{
        "task_id": "missing-patch",
        "architecture_summary": "x",
        "pipeline": [{
            "step": 1,
            "agent": "TranspilerAgent",
            "action": "sync_types",
            "target_file": "frontend/src/modules/config-ui/types.ts",
            "goal": "sync"
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("missing 'patch_content'"), "{err}");
}

#[test]
fn validate_rejects_step_zero() {
    let raw = valid_blueprint().replace("\"step\": 1", "\"step\": 0");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("must be >= 1"), "{err}");
}

#[test]
fn is_kebab_case_checks() {
    assert!(is_kebab_case("add-cache"));
    assert!(is_kebab_case("a-b-c"));
    assert!(!is_kebab_case(""));
    assert!(!is_kebab_case("CamelCase"));
    assert!(!is_kebab_case("nohyphen"));
    assert!(!is_kebab_case("-leading"));
    assert!(!is_kebab_case("trailing-"));
    assert!(!is_kebab_case("double--hyphen"));
}

#[test]
fn is_comment_sketch_detects_majority_comments() {
    assert!(is_comment_sketch("// only\n// comments\n"));
    assert!(!is_comment_sketch("pub fn x() {}\n// one comment\n"));
}

#[test]
fn validate_blueprint_grounding_requires_read_files() {
    let bp = validate_blueprint(&valid_blueprint()).unwrap();
    let err = validate_blueprint_grounding(&bp, &[]).unwrap_err();
    assert!(err.contains("no files scouted"), "{err}");
}

#[test]
fn validate_blueprint_grounding_accepts_touched_paths() {
    let bp = validate_blueprint(&valid_blueprint()).unwrap();
    let touched = vec![
        resolve_workspace_path("src/lib.rs"),
        resolve_workspace_path("src/domain.rs"),
    ];
    assert!(validate_blueprint_grounding(&bp, &touched).is_ok());
}

#[test]
fn validate_blueprint_grounding_rejects_unread_target() {
    let bp = validate_blueprint(&valid_blueprint()).unwrap();
    let touched = vec![resolve_workspace_path("src/lib.rs")];
    let err = validate_blueprint_grounding(&bp, &touched).unwrap_err();
    assert!(err.contains("was not read"), "{err}");
}

#[test]
fn validate_rejects_code_changes_without_generate_tests() {
    let patch = lib_rs_hunk("pub mod extra;\n");
    let raw = format!(
        r#"{{
        "task_id": "one-step-only",
        "architecture_summary": "Single patch only.",
        "pipeline": [{{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Wire limit at lib.rs:1.",
            "patch_content": {patch_json}
        }}]
    }}"#,
        patch_json = serde_json::to_string(&patch).unwrap()
    );
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("generate_tests"), "{err}");
}

#[test]
fn validate_rejects_goal_without_path_line_citation() {
    let raw = valid_blueprint().replace("domain.rs:84", "domain module");
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("path:line"), "{err}");
}

#[test]
fn validate_generate_tests_goal_requires_path_line() {
    let patch = lib_rs_hunk("pub mod helper;\n");
    let raw = format!(
        r#"{{
        "task_id": "test-only-goal",
        "architecture_summary": "Add tests.",
        "pipeline": [
            {{
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "Expose helper at lib.rs:1.",
                "patch_content": {patch_json}
            }},
            {{
                "step": 2,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/helper_test.rs",
                "goal": "Assert helper at tests/helper_test.rs:1.",
                "patch_content": ""
            }}
        ]
    }}"#,
        patch_json = serde_json::to_string(&patch).unwrap()
    );
    assert!(
        validate_blueprint(&raw).is_ok(),
        "{:?}",
        validate_blueprint(&raw).err()
    );
}

#[test]
fn validate_rejects_generate_tests_goal_without_path_line() {
    let raw = r#"{
        "task_id": "bad-test-goal",
        "architecture_summary": "Add tests.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "generate_tests",
            "target_file": "tests/helper_test.rs",
            "goal": "Assert helper returns expected value.",
            "patch_content": ""
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("path:line"), "{err}");
}

#[test]
fn always_on_hunks_reject_non_hunk_patch_file() {
    let raw = r#"{
        "task_id": "full-rewrite",
        "architecture_summary": "Rewrite.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Rewrite at lib.rs:1.",
            "patch_content": "pub fn rewritten() {}\n"
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(
        err.contains("SEARCH/REPLACE") || err.contains("hunk"),
        "{err}"
    );
}

fn surgical_feature_constraints() -> CoordinatorConstraints {
    CoordinatorConstraints::from_args(&PlanBlueprintArgs {
        feature_request: "x".to_string(),
        plan_kind: Some(PlanKind::Feature),
        expectation: Some("surgical patches only".to_string()),
    })
}

#[test]
fn coordinator_sync_types_rejects_builder_agent() {
    let raw = r#"{
        "task_id": "sync-types",
        "architecture_summary": "Sync DTOs.",
        "pipeline": [{
            "step": 1,
            "agent": "TranspilerAgent",
            "action": "sync_types",
            "target_file": "frontend/src/modules/config-ui/types.ts",
            "goal": "Sync from domain.rs:1.",
            "patch_content": ""
        }]
    }"#;
    let bp = validate_blueprint(raw).unwrap();
    let c = CoordinatorConstraints::from_args(&PlanBlueprintArgs {
        feature_request: "sync".to_string(),
        plan_kind: Some(PlanKind::SyncTypes),
        expectation: None,
    });
    let bad = r#"{
        "task_id": "sync-types-bad",
        "architecture_summary": "Wrong agent.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Wrong at lib.rs:1.",
            "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub fn x() {}\n>>>>>>> REPLACE\n"
        }, {
            "step": 2,
            "agent": "BuilderAgent",
            "action": "generate_tests",
            "target_file": "tests/x_test.rs",
            "goal": "Smoke at tests/x_test.rs:1.",
            "patch_content": ""
        }]
    }"#;
    let bad_bp = validate_blueprint(bad).unwrap();
    let err = validate_blueprint_coordinator(&bad_bp, &c).unwrap_err();
    assert!(err.contains("sync_types"), "{err}");
    assert!(validate_blueprint_coordinator(&bp, &c).is_ok());
}

#[test]
fn coordinator_bugfix_rejects_create_file() {
    let bp = json!({
        "task_id": "bugfix-one",
        "architecture_summary": "Fix.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "create_file",
            "target_file": "tests/fix_test.rs",
            "goal": "New test at fix_test.rs:1.",
            "patch_content": "fn fix_smoke() {}\n"
        }]
    });
    let c = CoordinatorConstraints::from_args(&PlanBlueprintArgs {
        feature_request: "fix".to_string(),
        plan_kind: Some(PlanKind::Bugfix),
        expectation: None,
    });
    let err = validate_blueprint_coordinator(&bp, &c).unwrap_err();
    assert!(err.contains("create_file"), "{err}");
}

#[test]
fn coordinator_surgical_rejects_large_patch_ratio() {
    let raw = r#"{
        "task_id": "big-patch",
        "architecture_summary": "Wholesale equal-size rewrite.",
        "pipeline": [
            {
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "Tiny change at lib.rs:1.",
                "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub fn x() {}\n>>>>>>> REPLACE\n"
            },
            {
                "step": 2,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/config_server.rs",
                "goal": "Full rewrite at config_server.rs:35.",
                "patch_content": "<<<<<<< SEARCH\n    let app = Router::new()\n        .route(\"/api/config\", get(get_config).put(put_config))\n        .route(\"/api/evaluations\", get(get_evaluations))\n=======\n    let app = axum::Router::new()\n        .route(\"/api/metrics/summary\", get(get_metrics_summary))\n        .route(\"/api/metrics/daily\", get(get_metrics_daily))\n>>>>>>> REPLACE\n"
            },
            {
                "step": 3,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/x_test.rs",
                "goal": "Cover rate limit behavior at config_server.rs:35.",
                "patch_content": ""
            }
        ]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("rewrites every SEARCH line"), "{err}");
}

#[test]
fn coordinator_surgical_accepts_golden_lib_rs_one_line_patch() {
    let golden = include_str!("../../../../tests/fixtures/golden-rate-limit-blueprint.json");
    let bp = validate_blueprint(golden).unwrap();
    assert!(validate_blueprint_coordinator(&bp, &surgical_feature_constraints()).is_ok());
}

#[test]
fn coordinator_manifest_single_line_dep_ok() {
    let raw = r#"{
        "task_id": "dep-line",
        "architecture_summary": "Add crate.",
        "pipeline": [
            {
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "Use dep at lib.rs:1.",
                "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\nuse foo::Bar;\n>>>>>>> REPLACE\n"
            },
            {
                "step": 2,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "Cargo.toml",
                "goal": "Add dep at Cargo.toml:12.",
                "patch_content": "<<<<<<< SEARCH\naxum = \"0.7\"\n=======\naxum = \"0.7\"\ntower-governor = \"0.4\"\n>>>>>>> REPLACE\n"
            },
            {
                "step": 3,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/foo_test.rs",
                "goal": "Smoke test at tests/x_test.rs:1.",
                "patch_content": ""
            }
        ]
    }"#;
    let bp = validate_blueprint(raw).unwrap();
    assert!(validate_blueprint_coordinator(&bp, &surgical_feature_constraints()).is_ok());
}

#[test]
fn coordinator_manifest_dependencies_block_rejected() {
    let raw = r#"{
        "task_id": "dep-block",
        "architecture_summary": "Rewrite manifest deps block.",
        "pipeline": [
            {
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "Stub at lib.rs:1.",
                "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub fn x() {}\n>>>>>>> REPLACE\n"
            },
            {
                "step": 2,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "Cargo.toml",
                "goal": "Deps at Cargo.toml:11.",
                "patch_content": "<<<<<<< SEARCH\nasync-trait = \"0.1\"\naxum = \"0.7\"\nbytemuck = { version = \"1.16\", features = [\"derive\"] }\n=======\ntracing = \"0.1\"\nserde_json = \"1\"\ntokio = { version = \"1\", features = [\"macros\"] }\n>>>>>>> REPLACE\n"
            },
            {
                "step": 3,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/x_test.rs",
                "goal": "Test at lib.rs:1.",
                "patch_content": ""
            }
        ]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("rewrites every SEARCH line"), "{err}");
}

#[test]
fn parse_hunks_accepts_well_formed_single_hunk() {
    let patch = "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub mod config_rate_limit;\n>>>>>>> REPLACE\n";
    let hunks = super::validate::parse_hunks(patch).expect("parse");
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].search, "pub mod agent;\n");
    assert_eq!(
        hunks[0].replace,
        "pub mod agent;\npub mod config_rate_limit;\n"
    );
}

#[test]
fn parse_hunks_rejects_content_outside_hunk() {
    let patch = "fn stray() {}\n<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n";
    let err = super::validate::parse_hunks(patch).unwrap_err();
    assert!(err.contains("outside a SEARCH/REPLACE hunk"), "{err}");
}

#[test]
fn parse_hunks_rejects_missing_separator() {
    let patch = "<<<<<<< SEARCH\nx\n>>>>>>> REPLACE\n";
    let err = super::validate::parse_hunks(patch).unwrap_err();
    assert!(err.contains("======="), "{err}");
}

#[test]
fn parse_hunks_rejects_empty_patch() {
    let err = super::validate::parse_hunks("   \n  ").unwrap_err();
    assert!(err.contains("no SEARCH/REPLACE hunks"), "{err}");
}

#[test]
fn coordinator_surgical_rejects_replace_equals_search() {
    let raw = r#"{
        "task_id": "noop-edit",
        "architecture_summary": "No-op.",
        "pipeline": [
            {
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "No-op at lib.rs:1.",
                "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\n>>>>>>> REPLACE\n"
            },
            {
                "step": 2,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/config_server.rs",
                "goal": "No-op at config_server.rs:35.",
                "patch_content": "<<<<<<< SEARCH\n    let app = Router::new()\n=======\n    let app = Router::new()\n>>>>>>> REPLACE\n"
            },
            {
                "step": 3,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/x_test.rs",
                "goal": "Smoke at tests/x_test.rs:1.",
                "patch_content": ""
            }
        ]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("identical to SEARCH"), "{err}");
}

#[test]
fn coordinator_surgical_rejects_oversized_replace() {
    // SEARCH is 1 real line; REPLACE adds 20 → exceeds the +15 cap.
    let mut replace_body = String::from("pub mod agent;\n");
    for i in 0..20 {
        replace_body.push_str(&format!("pub mod m_{i};\n"));
    }
    let patch = format!("<<<<<<< SEARCH\npub mod agent;\n=======\n{replace_body}>>>>>>> REPLACE\n");
    let patch_json = serde_json::to_string(&patch).unwrap();
    let raw = format!(
        r#"{{
        "task_id": "oversized-replace",
        "architecture_summary": "Dump logic.",
        "pipeline": [
            {{
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "Add modules at lib.rs:1.",
                "patch_content": {patch_json}
            }},
            {{
                "step": 2,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/config_server.rs",
                "goal": "Wiring at config_server.rs:35.",
                "patch_content": "<<<<<<< SEARCH\n    let app = Router::new()\n=======\n    let app = Router::new()\n    let limit = 60;\n>>>>>>> REPLACE\n"
            }},
            {{
                "step": 3,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/x_test.rs",
                "goal": "Smoke at tests/x_test.rs:1.",
                "patch_content": ""
            }}
        ]
    }}"#
    );
    let err = validate_blueprint(&raw).unwrap_err();
    assert!(err.contains("create_file") || err.contains("max"), "{err}");
}

#[test]
fn validate_accepts_rust_range_in_search_anchor() {
    // SEARCH contains a verbatim Rust range `0..10` — must NOT trigger the ellipsis check.
    let raw = r#"{
        "task_id": "range-ok",
        "architecture_summary": "Wire range.",
        "pipeline": [
            {
                "step": 1,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/lib.rs",
                "goal": "Wire at lib.rs:1.",
                "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\nlet r = 0..10;\n>>>>>>> REPLACE\n"
            },
            {
                "step": 2,
                "agent": "BuilderAgent",
                "action": "patch_file",
                "target_file": "src/config_server.rs",
                "goal": "Wiring at config_server.rs:35.",
                "patch_content": "<<<<<<< SEARCH\n    let app = Router::new()\n=======\n    let app = Router::new()\n    let limit = 60;\n>>>>>>> REPLACE\n"
            },
            {
                "step": 3,
                "agent": "BuilderAgent",
                "action": "generate_tests",
                "target_file": "tests/range_test.rs",
                "goal": "Smoke at tests/x_test.rs:1.",
                "patch_content": ""
            }
        ]
    }"#;
    assert!(
        validate_blueprint(raw).is_ok(),
        "{:?}",
        validate_blueprint(raw).err()
    );
}

#[test]
fn validate_rejects_ellipsis_in_replace_body() {
    let raw = r#"{
        "task_id": "ellipsis-bad",
        "architecture_summary": "Sketch.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Wire at lib.rs:1.",
            "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\nfn x() { ... }\n>>>>>>> REPLACE\n"
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("ellipsis"), "{err}");
}

#[test]
fn validate_rejects_generate_tests_target_not_in_tests() {
    let raw = r#"{
        "task_id": "bad-gen-path",
        "architecture_summary": "x",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "generate_tests",
            "target_file": "src/config_server.rs",
            "goal": "Unit test at tests/foo_test.rs:1.",
            "patch_content": ""
        }]
    }"#;
    let err = validate_blueprint(raw).unwrap_err();
    assert!(err.contains("tests/"), "{err}");
}

#[test]
fn validate_accepts_generate_tests_target_in_tests() {
    let raw = r#"{
        "task_id": "ok-gen-path",
        "architecture_summary": "x",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "generate_tests",
            "target_file": "tests/foo_test.rs",
            "goal": "Unit test at tests/foo_test.rs:1.",
            "patch_content": ""
        }]
    }"#;
    assert!(
        validate_blueprint(raw).is_ok(),
        "{:?}",
        validate_blueprint(raw).err()
    );
}

#[test]
fn emit_blueprint_invoke_returns_pretty_json_on_valid_input() {
    let tool = EmitBlueprintTool::new(CoordinatorConstraints::none());
    let args = json!({ "blueprint": valid_blueprint() });
    let out = tool.invoke(&args).unwrap();
    assert!(out.contains("\"task_id\""));
    assert!(out.contains("\"add-cache-layer\""));
}

#[test]
fn emit_blueprint_invoke_returns_error_on_invalid_input() {
    let tool = EmitBlueprintTool::new(CoordinatorConstraints::none());
    let args = json!({ "blueprint": "not json" });
    let err = tool.invoke(&args).unwrap_err();
    assert!(err.contains("Blueprint rejected"), "{err}");
}
