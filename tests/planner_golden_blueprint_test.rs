use mcp_adjutant::cache::resolve_workspace_path;
use mcp_adjutant::{
    validate_blueprint, validate_blueprint_coordinator, validate_blueprint_grounding,
    CoordinatorConstraints, PlanBlueprintArgs, PlanKind,
};

const GOLDEN: &str = include_str!("fixtures/golden-rate-limit-blueprint.json");
const BAD_FULL_REWRITE: &str = include_str!("fixtures/bad-full-rewrite-blueprint.json");

fn comment_sketch_blueprint() -> String {
    r#"{
        "task_id": "bad-sketch",
        "architecture_summary": "Sketch only.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Wire limit at lib.rs:1.",
            "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\n// split api router here\n// add rate limit layer\n// return 429 on exceed\n>>>>>>> REPLACE\n"
        }]
    }"#
    .to_string()
}

fn triage_on_cargo_blueprint() -> String {
    GOLDEN.replace("\"BuilderAgent\"", "\"TriageAgent\"")
}

#[test]
fn golden_rate_limit_blueprint_passes_strict_validation() {
    let result = validate_blueprint(GOLDEN);
    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn golden_blueprint_passes_grounding_when_targets_read() {
    let bp = validate_blueprint(GOLDEN).expect("valid blueprint");
    let touched = vec![
        resolve_workspace_path("src/lib.rs"),
        resolve_workspace_path("src/config_server.rs"),
        resolve_workspace_path("Cargo.toml"),
    ];
    assert!(
        validate_blueprint_grounding(&bp, &touched).is_ok(),
        "create_file and generate_tests targets need not be touched"
    );
}

#[test]
fn comment_sketch_variant_fails_validation() {
    let err = validate_blueprint(&comment_sketch_blueprint()).unwrap_err();
    assert!(
        err.contains("comment sketch") || err.contains("placeholder"),
        "{err}"
    );
}

#[test]
fn triage_agent_on_cargo_variant_fails_routing() {
    let err = validate_blueprint(&triage_on_cargo_blueprint()).unwrap_err();
    assert!(
        err.contains("TriageAgent is not a blueprint step")
            || err.contains("cannot perform action"),
        "{err}"
    );
}

#[test]
fn nonempty_patch_on_generate_tests_fails() {
    let bad = GOLDEN.replace(
        "\"goal\": \"Integration test: burst GET /api/config at config_server.rs:35 and assert 429 after 60 requests.\",\n      \"patch_content\": \"\"",
        "\"goal\": \"Integration test: burst GET /api/config at config_server.rs:35 and assert 429 after 60 requests.\",\n      \"patch_content\": \"fn test() {}\"",
    );
    let err = validate_blueprint(&bad).unwrap_err();
    assert!(err.contains("must be empty"), "{err}");
}

#[test]
fn single_step_feature_blueprint_fails_completeness() {
    let thin = r#"{
        "task_id": "add-http-rate-limiting",
        "architecture_summary": "Rate limit /api routes.",
        "pipeline": [{
            "step": 1,
            "agent": "BuilderAgent",
            "action": "patch_file",
            "target_file": "src/lib.rs",
            "goal": "Add limit at lib.rs:1.",
            "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub mod limit;\n>>>>>>> REPLACE\n"
        }]
    }"#;
    let err = validate_blueprint(thin).unwrap_err();
    assert!(err.contains("generate_tests"), "{err}");
}

fn surgical_feature_constraints() -> CoordinatorConstraints {
    CoordinatorConstraints::from_args(&PlanBlueprintArgs {
        feature_request: "rate limit".to_string(),
        plan_kind: Some(PlanKind::Feature),
        expectation: Some("surgical patches only".to_string()),
    })
}

#[test]
fn golden_blueprint_passes_without_coordinator_constraints() {
    let bp = validate_blueprint(GOLDEN).expect("valid");
    assert!(validate_blueprint_coordinator(&bp, &CoordinatorConstraints::none()).is_ok());
}

#[test]
fn golden_blueprint_passes_surgical_feature_constraints() {
    let bp = validate_blueprint(GOLDEN).expect("valid");
    assert!(validate_blueprint_coordinator(&bp, &surgical_feature_constraints()).is_ok());
}

#[test]
fn bad_full_rewrite_fails_surgical_constraints() {
    let err = validate_blueprint(BAD_FULL_REWRITE).unwrap_err();
    assert!(
        err.contains("SEARCH block not found")
            || err.contains("SEARCH/REPLACE")
            || err.contains("hunk"),
        "{err}"
    );
}
