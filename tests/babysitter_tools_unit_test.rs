mod common;

use mcp_adjutant::agent::{
    babysitter_tool_set, parse_log_path, parse_report_body, parse_triage_arguments,
};
use serde_json::json;

#[test]
fn babysitter_tool_set_has_six_tools() {
    let tool_set = babysitter_tool_set();
    assert_eq!(tool_set.len(), 6);
    let names: Vec<_> = tool_set
        .definitions()
        .into_iter()
        .map(|def| def.name.clone())
        .collect();
    assert!(names.contains(&"finalize_session".to_string()));
}

#[test]
fn parse_log_path_reads_string() {
    let arguments = json!({ "log_path": "gh-run:123" });
    assert_eq!(parse_log_path(&arguments).expect("parse"), "gh-run:123");
}

#[test]
fn parse_triage_arguments_requires_paths() {
    let err = parse_triage_arguments(&json!({"error_context": "x"})).unwrap_err();
    assert!(err.contains("target_paths"));
}

#[test]
fn parse_report_body_reads_string() {
    let arguments = json!({ "report": "done" });
    assert_eq!(parse_report_body(&arguments).expect("parse"), "done");
}

#[test]
fn finalize_session_tool_is_terminal() {
    let output = babysitter_tool_set()
        .invoke("finalize_session", &json!({"summary": "ok"}))
        .expect("invoke");
    assert!(output.is_terminal);
    assert_eq!(output.output, "ok");
}
