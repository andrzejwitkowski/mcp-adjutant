mod common;

use mcp_adjutant::agent::git_janitor::tools::{parse_emit_fields, parse_patch_json};
use serde_json::json;

/// Test that parse_patch_json correctly parses a valid JSON string.
#[test]
fn parse_patch_json_parses_valid_json() {
    let args = json!({
        "patch_json": r#"{"git_rules": {"branch_prefix": "test"}}"#
    });

    let result = parse_patch_json(&args);
    assert!(
        result.is_ok(),
        "parse_patch_json should succeed for valid JSON"
    );

    let parsed = result.unwrap();
    assert!(parsed.is_object());
    assert!(parsed.get("git_rules").is_some());
}

/// Test that parse_patch_json returns an error for invalid JSON.
#[test]
fn parse_patch_json_rejects_invalid_json() {
    let args = json!({
        "patch_json": "not valid json {{{"
    });

    let result = parse_patch_json(&args);
    assert!(
        result.is_err(),
        "parse_patch_json should fail for invalid JSON"
    );

    let err = result.unwrap_err();
    assert!(
        err.contains("patch_json must be JSON object"),
        "error message should mention patch_json"
    );
}

/// Test that parse_emit_fields extracts all required fields correctly.
#[test]
fn parse_emit_fields_extracts_all_fields() {
    let args = json!({
        "commit_message": "fix: resolve issue #42",
        "pr_title": "Fix issue #42",
        "pr_body": "This PR fixes issue #42.",
        "changelog_entry": "Fixed a bug that caused crashes on startup."
    });

    let result = parse_emit_fields(&args);
    assert!(
        result.is_ok(),
        "parse_emit_fields should succeed with all required fields"
    );

    let fields = result.unwrap();
    assert_eq!(fields.commit_message, "fix: resolve issue #42");
    assert_eq!(fields.pr_title, "Fix issue #42");
    assert_eq!(fields.pr_body, "This PR fixes issue #42.");
    assert_eq!(
        fields.changelog_entry,
        "Fixed a bug that caused crashes on startup."
    );
}

/// Test that parse_emit_fields returns an error when a required field is missing.
#[test]
fn parse_emit_fields_rejects_missing_required_field() {
    let args = json!({
        "commit_message": "fix: resolve issue #42",
        "pr_title": "Fix issue #42",
        // pr_body is missing
        "changelog_entry": "Fixed a bug that caused crashes on startup."
    });

    let result = parse_emit_fields(&args);
    assert!(
        result.is_err(),
        "parse_emit_fields should fail when pr_body is missing"
    );
}
