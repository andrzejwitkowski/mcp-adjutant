use std::fs;
use std::path::Path;

use mcp_adjutant::cache::resolve_workspace_path_bounded;
use mcp_adjutant::tools::{
    analyze_crash_log, build_summary, parser_confident, read_log_file, resolve_log_content,
    strip_file_url, LogSourceKind,
};

#[test]
fn python_traceback_parses_innermost_frame() {
    let log = "Traceback (most recent call last):\n  File \"lib/handlers.py\", line 24, in handle_request\n    result = process(data)\n  File \"lib/service.py\", line 12, in process\n    return int(data[\"count\"])\nValueError: invalid literal";
    let core = analyze_crash_log(log);
    assert_eq!(core.error_type, "ValueError");
    assert_eq!(core.target_file.as_deref(), Some("lib/service.py"));
    assert_eq!(core.line_number, Some(12));
}

#[test]
fn python_traceback_parses_file_line() {
    let log = "Traceback (most recent call last):\n  File \"src/main.py\", line 12, in <module>\n    main()\nValueError: bad input";
    let core = analyze_crash_log(log);
    assert_eq!(core.error_type, "ValueError");
    assert_eq!(core.target_file.as_deref(), Some("src/main.py"));
    assert_eq!(core.line_number, Some(12));
    assert!(parser_confident(&core));
    assert!(build_summary(&core).contains("src/main.py"));
}

#[test]
fn first_rust_error_wins_over_later_cascade() {
    let log = "noise\nHTTP GET /health 200\nerror[E0425]: cannot find value `syntax` in this scope\n --> src/foo.rs:10:5\nerror[E0308]: mismatched types\n --> src/bar.rs:2:1";
    let core = analyze_crash_log(log);
    assert_eq!(core.target_file.as_deref(), Some("src/foo.rs"));
    assert_eq!(core.line_number, Some(10));
}

#[test]
fn strip_file_url_prefix() {
    assert_eq!(strip_file_url("file:///tmp/test.log"), "/tmp/test.log");
}

#[test]
fn bounded_resolve_rejects_parent_escape() {
    assert!(resolve_workspace_path_bounded("../etc/passwd").is_err());
}

#[test]
fn read_log_fixture_from_temp_file() {
    let dir = std::env::temp_dir().join(format!("crash-log-fixture-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("tmpdir");
    let path = dir.join("fail.log");
    fs::write(
        &path,
        "error[E0425]: cannot find value `syntax` in this scope\n --> src/foo.rs:10:5\n",
    )
    .expect("write");

    let (content, truncated) = read_log_file(&path).expect("read");
    assert!(!truncated);
    let core = analyze_crash_log(&content);
    assert!(parser_confident(&core));

    fs::remove_dir_all(dir).ok();
}

#[test]
fn directory_path_is_not_a_file() {
    let dir = std::env::temp_dir().join(format!("crash-log-dir-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("tmpdir");
    assert!(!Path::new(&dir).is_file());
    fs::remove_dir_all(dir).ok();
}

#[test]
fn java_null_pointer_fixture_parser_hit() {
    let log = include_str!("fixtures/logs/v2/java_null_pointer.log");
    let core = analyze_crash_log(log);
    assert_eq!(core.error_type, "NullPointerException");
    assert_eq!(core.target_file.as_deref(), Some("App.java"));
    assert_eq!(core.line_number, Some(14));
    assert!(parser_confident(&core));
}

#[test]
fn rust_panic_fixture_v2_quoted_message() {
    let log = include_str!("fixtures/logs/v2/rust_quoted_panic.log");
    let core = analyze_crash_log(log);
    assert_eq!(
        core.error_message,
        "index out of bounds: the len is 3 but the index is 9"
    );
    assert_eq!(core.target_file.as_deref(), Some("src/storage.rs"));
    assert_eq!(core.line_number, Some(201));
}

#[test]
fn rust_panic_fixture_extracts_coordinates() {
    let log = include_str!("fixtures/logs/rust_panic.log");
    let core = analyze_crash_log(log);
    assert_eq!(core.error_type, "Panic");
    assert_eq!(core.target_file.as_deref(), Some("src/agent/triage.rs"));
    assert_eq!(core.line_number, Some(796));
    assert_eq!(core.column_number, Some(14));
    assert_eq!(core.error_message, "boom: assertion failed");
    assert!(parser_confident(&core));
}

#[test]
fn resolve_log_content_local_fixture() {
    let resolved =
        resolve_log_content("tests/fixtures/logs/rust_compile.log").expect("local fixture");
    assert_eq!(resolved.kind, LogSourceKind::Local);
    assert!(resolved.content.contains("error[E0425]"));
    assert!(!resolved.truncated);
}

#[test]
fn resolve_log_content_rejects_https_private_ip() {
    assert!(resolve_log_content("https://127.0.0.1/ci.log").is_err());
}

#[test]
fn resolve_log_content_rejects_gh_run_injection() {
    assert!(resolve_log_content("gh-run:123;rm -rf /").is_err());
}

#[test]
fn node_fixture_preserves_frontend_path() {
    let log = include_str!("fixtures/logs/node_typeerror.log");
    let core = analyze_crash_log(log);
    assert_eq!(
        core.target_file.as_deref(),
        Some("frontend/src/modules/config-ui/ConfigApp.tsx")
    );
    assert!(parser_confident(&core));
}
