use std::path::Path;

use mcp_adjutant::cache::mcp_workspace_root;
use mcp_adjutant::tools::{
    detect_file_language, detect_project_languages, read_file_range, run_fd, run_ripgrep,
    AstUsageFinder, SourceLanguage,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scout");

#[test]
fn read_file_range_returns_requested_lines() {
    let path = Path::new(FIXTURES).join("readme.txt");
    let content = read_file_range(&path, 3, 4).expect("range read should succeed");

    assert_eq!(content, "second line\nthird line\n");
}

#[test]
fn read_file_range_rejects_start_after_end() {
    let path = Path::new(FIXTURES).join("readme.txt");
    let err = read_file_range(&path, 4, 2).expect_err("invalid range should fail");

    assert!(err.contains("start"), "error should mention start: {err}");
}

#[test]
fn run_ripgrep_from_target_cwd_uses_project_root() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target_dir = manifest.join("target");
    std::fs::create_dir_all(&target_dir).expect("create target dir");
    let original = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&target_dir).expect("chdir target");

    let root = mcp_workspace_root();
    let output = run_ripgrep("JobRegistry", &root).expect("ripgrep should succeed");

    std::env::set_current_dir(original).expect("restore cwd");

    assert_eq!(root, manifest);
    assert!(
        output.contains("JobRegistry"),
        "expected matches under project root: {output}"
    );
}

#[test]
fn run_ripgrep_finds_pattern_with_context() {
    let root = Path::new(FIXTURES);
    let output = run_ripgrep("alpha marker", root).expect("ripgrep should succeed");

    assert!(
        output.contains("alpha marker"),
        "output should include the match: {output}"
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
    assert_call_lines("sample.rs", vec![3, 5]);
}

#[test]
fn ast_usage_finder_locates_typescript_call_sites_only() {
    assert_call_lines("sample.ts", vec![3, 5]);
}

#[test]
fn ast_usage_finder_locates_python_call_sites_only() {
    assert_call_lines("sample.py", vec![3, 5]);
}

#[test]
fn ast_usage_finder_locates_java_call_sites_only() {
    assert_call_lines("sample.java", vec![4, 6]);
}

#[test]
fn ast_usage_finder_locates_kotlin_call_sites_only() {
    assert_call_lines("sample.kt", vec![3, 5]);
}

#[test]
fn ast_usage_finder_locates_c_call_sites_only() {
    assert_call_lines("sample.c", vec![5]);
}

#[test]
fn ast_usage_finder_locates_cpp_call_sites_only() {
    assert_call_lines("sample.cpp", vec![7, 9]);
}

#[test]
fn ast_usage_finder_locates_sql_call_sites_only() {
    assert_call_lines("sample.sql", vec![2, 4]);
}

#[test]
fn detect_file_language_from_extension() {
    let report = detect_file_language(&Path::new(FIXTURES).join("sample.py")).expect("detect");
    assert_eq!(report.language, SourceLanguage::Python);
}

#[test]
fn detect_file_language_for_cpp_header() {
    let report = detect_file_language(&Path::new(FIXTURES).join("sample.hpp")).expect("detect");
    assert_eq!(report.language, SourceLanguage::Cpp);
    assert!(report.method.contains("extension"));
}

#[test]
fn detect_project_languages_finds_rust_marker() {
    let report =
        detect_project_languages(&Path::new(FIXTURES).join("project")).expect("project detect");

    assert!(report
        .markers
        .iter()
        .any(|marker| marker.contains("Cargo.toml")));
    assert_eq!(report.primary, Some(SourceLanguage::Rust));
}

fn assert_call_lines(file_name: &str, expected: Vec<usize>) {
    let path = Path::new(FIXTURES).join(file_name);
    let lines = AstUsageFinder::find_calls_in_file(&path, "invoke").unwrap_or_else(|err| {
        panic!("AST scan failed for {file_name}: {err}");
    });
    assert_eq!(lines, expected, "unexpected call lines for {file_name}");
}
