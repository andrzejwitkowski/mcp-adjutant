use mcp_adjutant::tools::{analyze_crash_log, parser_confident, to_report, ReportEnrichment};

#[test]
fn rust_compile_error_extracts_location() {
    let log = "   Compiling foo\nerror[E0425]: cannot find value `syntax` in this scope\n --> src/foo.rs:10:5\n  |\n5 |     let x = syntax;\n  |             ^^^^^^";
    let core = analyze_crash_log(log);

    assert_eq!(core.target_file.as_deref(), Some("src/foo.rs"));
    assert_eq!(core.line_number, Some(10));
    assert_eq!(core.column_number, Some(5));
    assert!(parser_confident(&core));
    assert_eq!(core.error_type, "CompileError");

    let report = to_report(
        core,
        "log/path",
        "unit-test",
        false,
        ReportEnrichment::default(),
    );
    assert!(!report.summary.is_empty());
}
