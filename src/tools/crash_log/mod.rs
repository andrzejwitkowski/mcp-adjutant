mod io;
mod parsers;

use serde::Serialize;

pub use io::{read_log_file, strip_file_url, truncate_for_llm, truncate_log_text, MAX_LOG_BYTES};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LogAnalysisReport {
    pub error_type: String,
    pub error_message: String,
    pub target_file: Option<String>,
    pub line_number: Option<u32>,
    pub column_number: Option<u32>,
    pub isolated_stack_trace: String,
    pub summary: String,
    pub log_path: String,
    pub log_truncated: bool,
    pub log_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_fallback_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CrashAnalysisCore {
    pub error_type: String,
    pub error_message: String,
    pub target_file: Option<String>,
    pub line_number: Option<u32>,
    pub column_number: Option<u32>,
    pub isolated_stack_trace: String,
}

pub fn analyze_crash_log(log: &str) -> CrashAnalysisCore {
    if let Some(core) = parsers::detect_rust_compile_error(log) {
        return core;
    }
    if let Some(core) = parsers::detect_rust_panic(log) {
        return core;
    }
    if let Some(core) = parsers::detect_python_traceback(log) {
        return core;
    }
    if let Some(core) = parsers::detect_node_error(log) {
        return core;
    }
    if let Some(core) = parsers::detect_java_exception(log) {
        return core;
    }
    if let Some(core) = parsers::detect_generic_runtime(log) {
        return core;
    }

    CrashAnalysisCore {
        error_type: "Unknown".into(),
        error_message: "no recognizable error pattern".into(),
        target_file: None,
        line_number: None,
        column_number: None,
        isolated_stack_trace: String::new(),
    }
}

pub fn parser_confident(core: &CrashAnalysisCore) -> bool {
    core.target_file.is_some()
        || core
            .isolated_stack_trace
            .lines()
            .any(parsers::looks_like_source_frame)
}

pub fn build_summary(core: &CrashAnalysisCore) -> String {
    let loc = match (&core.target_file, core.line_number) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        (Some(file), None) => file.clone(),
        _ => return core.error_message.clone(),
    };
    format!("{} at {loc} — {}", core.error_type, core.error_message)
}

pub fn to_report(
    core: CrashAnalysisCore,
    log_path: impl Into<String>,
    log_source: impl Into<String>,
    log_truncated: bool,
    summary: Option<String>,
    llm_fallback_error: Option<String>,
) -> LogAnalysisReport {
    let summary = summary
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| build_summary(&core));
    LogAnalysisReport {
        error_type: core.error_type,
        error_message: core.error_message,
        target_file: core.target_file,
        line_number: core.line_number,
        column_number: core.column_number,
        isolated_stack_trace: core.isolated_stack_trace,
        summary,
        log_path: log_path.into(),
        log_truncated,
        log_source: log_source.into(),
        llm_fallback_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_compile_error_extracts_location() {
        let log = "   Compiling foo\nerror[E0425]: cannot find value `syntax` in this scope\n --> src/foo.rs:10:5\n  |\n5 |     let x = syntax;\n  |             ^^^^^^";
        let core = analyze_crash_log(log);
        assert_eq!(core.error_type, "CompileError");
        assert_eq!(core.target_file.as_deref(), Some("src/foo.rs"));
        assert_eq!(core.line_number, Some(10));
        assert_eq!(core.column_number, Some(5));
        assert!(parser_confident(&core));
    }

    #[test]
    fn java_exception_extracts_file_and_line() {
        let log = include_str!("../../../tests/fixtures/logs/v2/java_null_pointer.log");
        let core = analyze_crash_log(log);
        assert_eq!(core.error_type, "NullPointerException");
        assert_eq!(core.target_file.as_deref(), Some("App.java"));
        assert_eq!(core.line_number, Some(14));
        assert_eq!(core.column_number, None);
        assert!(core.error_message.contains("String.length()"));
        assert!(parser_confident(&core));
    }

    #[test]
    fn rust_panic_quoted_message_uses_inline_text() {
        let log = "thread 'test_storage_roundtrip' panicked at 'index out of bounds: the len is 3 but the index is 9', src/storage.rs:201:9:\nnote: run with `RUST_BACKTRACE=1`";
        let core = analyze_crash_log(log);
        assert_eq!(core.error_type, "Panic");
        assert_eq!(core.target_file.as_deref(), Some("src/storage.rs"));
        assert_eq!(core.line_number, Some(201));
        assert_eq!(core.column_number, Some(9));
        assert_eq!(
            core.error_message,
            "index out of bounds: the len is 3 but the index is 9"
        );
        assert!(parser_confident(&core));
    }

    #[test]
    fn rust_panic_unquoted_path_extracts_coordinates() {
        let log = "thread 'worker' panicked at src/agent/triage.rs:796:14:\nboom: assertion failed\nstack backtrace:";
        let core = analyze_crash_log(log);
        assert_eq!(core.error_type, "Panic");
        assert_eq!(core.target_file.as_deref(), Some("src/agent/triage.rs"));
        assert_eq!(core.line_number, Some(796));
        assert_eq!(core.column_number, Some(14));
        assert_eq!(core.error_message, "boom: assertion failed");
        assert!(parser_confident(&core));
    }

    #[test]
    fn node_stack_preserves_frontend_in_path() {
        let log = "TypeError: Cannot read properties of undefined (reading 'map')\n    at renderList (frontend/src/modules/config-ui/ConfigApp.tsx:142:18)";
        let core = analyze_crash_log(log);
        assert_eq!(
            core.target_file.as_deref(),
            Some("frontend/src/modules/config-ui/ConfigApp.tsx")
        );
    }

    #[test]
    fn rust_panic_without_path_is_low_confidence() {
        let core = analyze_crash_log("thread 'worker' panicked at 'boom'");
        assert_eq!(core.error_type, "Panic");
        assert!(!parser_confident(&core));
    }

    #[test]
    fn node_type_error_parses_stack() {
        let log =
            "TypeError: Cannot read property 'x' of undefined\n    at Object.fn (src/app.ts:10:5)";
        let core = analyze_crash_log(log);
        assert_eq!(core.error_type, "TypeError");
        assert_eq!(core.target_file.as_deref(), Some("src/app.ts"));
        assert_eq!(core.line_number, Some(10));
    }
}
