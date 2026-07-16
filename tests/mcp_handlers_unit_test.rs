mod common;

use mcp_adjutant::mcp::handlers::scout_context_schema;

#[test]
fn test_scout_context_schema_structure() {
    let schema = scout_context_schema();

    assert_eq!(schema["name"], "scout_context");
    assert!(schema["input_schema"]["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("query")));
    assert!(schema["input_schema"]["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("request_uuid")));
    assert!(schema["input_schema"]["properties"]
        .as_object()
        .unwrap()
        .contains_key("workspace_root"));
}
