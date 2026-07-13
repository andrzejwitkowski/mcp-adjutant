mod common;

use mcp_adjutant::agent::{
    parse_report_reason, parse_transpiler_triage_arguments, parse_write_arguments,
    transpiler_tool_set,
};
use serde_json::json;

#[test]
fn test_parse_write_arguments() {
    let arguments = json!({ "path": "src/main.rs", "content": "fn main() {}" });
    let (path, content) =
        parse_write_arguments(&arguments).expect("Failed to parse write arguments");
    assert_eq!(path, "src/main.rs");
    assert_eq!(content, "fn main() {}");
    assert!(parse_write_arguments(&json!({ "content": "fn main() {}" })).is_err());
    assert!(parse_write_arguments(&json!({ "path": "src/main.rs" })).is_err());
}

#[test]
fn test_parse_triage_arguments() {
    let arguments = json!({
        "target_paths": ["src/main.rs", "src/lib.rs"],
        "error_context": "error: expected type"
    });
    let (paths, error_context) =
        parse_transpiler_triage_arguments(&arguments).expect("Failed to parse triage arguments");
    assert_eq!(paths, vec!["src/main.rs", "src/lib.rs"]);
    assert_eq!(error_context, "error: expected type");
    assert!(parse_transpiler_triage_arguments(&json!({
        "target_paths": [],
        "error_context": "error: expected type"
    }))
    .is_err());
}

#[test]
fn test_parse_report_reason() {
    let reason =
        parse_report_reason(&json!({ "reason": "Failed to compile" })).expect("parse report");
    assert_eq!(reason, "Failed to compile");
    assert!(parse_report_reason(&json!({})).is_err());
}

#[test]
fn test_transpiler_tool_set_invoke_write_target_file() {
    assert!(transpiler_tool_set()
        .invoke(
            "write_target_file",
            &json!({ "path": "src/temp.rs", "content": "fn temp() {}" }),
        )
        .is_err());
}

#[test]
fn test_transpiler_tool_set_invoke_finalize_sync() {
    let tools = transpiler_tool_set();
    let with_summary = tools
        .invoke("finalize_sync", &json!({ "summary": "Sync complete" }))
        .expect("invoke finalize_sync");
    assert!(with_summary.is_terminal);
    assert_eq!(with_summary.output, "Sync complete");
    let without_summary = tools
        .invoke("finalize_sync", &json!({}))
        .expect("invoke finalize_sync default");
    assert_eq!(without_summary.output, "session finalized");
}

#[test]
fn test_transpiler_tool_set_invoke_report_error() {
    let tools = transpiler_tool_set();
    let out = tools
        .invoke("report_error", &json!({ "reason": "Compilation failed" }))
        .expect("invoke report_error");
    assert!(out.is_terminal);
    assert_eq!(out.output, "Compilation failed");
}

#[test]
fn transpiler_tool_set_has_four_tools() {
    assert_eq!(transpiler_tool_set().len(), 4);
}
