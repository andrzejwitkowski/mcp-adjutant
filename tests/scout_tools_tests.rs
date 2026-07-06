use std::path::Path;

use mcp_adjutant::tools::{read_file_range, run_fd, run_ripgrep, AstUsageFinder};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scout");

#[test]
fn read_file_range_returns_requested_lines() {
    let path = Path::new(FIXTURES).join("readme.txt");
    let content = read_file_range(&path, 2, 3).expect("range read should succeed");

    assert_eq!(content, "second line\nthird line\n");
}

#[test]
fn read_file_range_rejects_start_after_end() {
    let path = Path::new(FIXTURES).join("readme.txt");
    let err = read_file_range(&path, 4, 2).expect_err("invalid range should fail");

    assert!(err.contains("start"), "error should mention start: {err}");
}

#[test]
fn run_ripgrep_finds_pattern_with_context() {
    let pattern = "alpha marker";
    let output = run_ripgrep(pattern).expect("ripgrep should succeed");

    assert!(
        output.contains("alpha marker"),
        "output should include the match: {output}"
    );
    assert!(
        output.contains("second line") || output.contains("context"),
        "output should include surrounding context: {output}"
    );
}

#[test]
fn run_fd_finds_fixture_files_by_name() {
    let files = run_fd("sample.rs").expect("fd should succeed");

    assert!(
        files.iter().any(|path| path.ends_with("sample.rs")),
        "expected sample.rs in results: {files:?}"
    );
}

#[test]
fn ast_usage_finder_locates_rust_call_sites_only() {
    let path = Path::new(FIXTURES).join("sample.rs");
    let lines = AstUsageFinder::find_calls_in_file(&path, "invoke").expect("rust ast scan");

    assert_eq!(
        lines,
        vec![3, 5],
        "expected physical call lines 3 and 5, got {lines:?}"
    );
}

#[test]
fn ast_usage_finder_locates_typescript_call_sites_only() {
    let path = Path::new(FIXTURES).join("sample.ts");
    let lines = AstUsageFinder::find_calls_in_file(&path, "invoke").expect("ts ast scan");

    assert_eq!(
        lines,
        vec![3, 5],
        "expected physical call lines 3 and 5, got {lines:?}"
    );
}
