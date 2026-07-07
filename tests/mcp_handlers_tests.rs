use std::sync::Arc;

use serde_json::json;

use mcp_adjutant::{
    generate_tests_and_scaffolding_schema, handle_generate_tests_and_scaffolding,
    registered_mcp_tools, scout_context_schema, verify_and_triage_schema, AdjutantConfig,
    GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME, SCOUT_CONTEXT_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
};

#[test]
fn registered_mcp_tools_includes_all_three_schemas_in_order() {
    let tools = registered_mcp_tools();
    let names: Vec<String> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(
        names,
        vec![
            SCOUT_CONTEXT_TOOL_NAME.to_string(),
            VERIFY_AND_TRIAGE_TOOL_NAME.to_string(),
            GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME.to_string(),
        ]
    );
}

#[test]
fn generate_tests_and_scaffolding_schema_declares_required_fields_and_enum() {
    let schema = generate_tests_and_scaffolding_schema();

    assert_eq!(schema["name"], GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME);
    assert_eq!(
        schema["input_schema"]["required"],
        json!(["source_file_path", "test_type"])
    );
    assert_eq!(
        schema["input_schema"]["properties"]["test_type"]["enum"],
        json!(["unit", "integration", "factory"])
    );
    assert_eq!(
        schema["input_schema"]["properties"]["source_file_path"]["type"],
        "string"
    );
}

#[test]
fn scout_context_and_verify_and_triage_schemas_still_expose_expected_names() {
    let scout_schema = scout_context_schema();
    assert_eq!(scout_schema["name"], SCOUT_CONTEXT_TOOL_NAME);

    let triage_schema = verify_and_triage_schema();
    assert_eq!(triage_schema["name"], VERIFY_AND_TRIAGE_TOOL_NAME);
}

#[tokio::test]
async fn handle_generate_tests_and_scaffolding_requires_source_file_path() {
    let config = Arc::new(AdjutantConfig::default());

    let err = handle_generate_tests_and_scaffolding(json!({ "test_type": "unit" }), config)
        .await
        .expect_err("missing source_file_path should error");

    assert_eq!(err, "source_file_path is required");
}

#[tokio::test]
async fn handle_generate_tests_and_scaffolding_rejects_blank_source_file_path() {
    let config = Arc::new(AdjutantConfig::default());

    let err = handle_generate_tests_and_scaffolding(
        json!({ "source_file_path": "   ", "test_type": "unit" }),
        config,
    )
    .await
    .expect_err("blank source_file_path should error");

    assert_eq!(err, "source_file_path is required");
}

#[tokio::test]
async fn handle_generate_tests_and_scaffolding_requires_test_type() {
    let config = Arc::new(AdjutantConfig::default());

    let err = handle_generate_tests_and_scaffolding(
        json!({ "source_file_path": "src/lib.rs" }),
        config,
    )
    .await
    .expect_err("missing test_type should error");

    assert_eq!(err, "test_type is required");
}

#[tokio::test]
async fn handle_generate_tests_and_scaffolding_rejects_non_string_test_type() {
    let config = Arc::new(AdjutantConfig::default());

    let err = handle_generate_tests_and_scaffolding(
        json!({ "source_file_path": "src/lib.rs", "test_type": 42 }),
        config,
    )
    .await
    .expect_err("non-string test_type should error");

    assert_eq!(err, "test_type is required");
}