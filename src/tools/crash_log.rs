use std::path::Path;

use serde::Serialize;

pub const MAX_LOG_BYTES: usize = 512 * 1024;
pub const LLM_LOG_BYTES: usize = 24_000;

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

pub fn truncate_log_text(text: &str) -> (String, bool) {
    truncate_log_bytes(text.as_bytes(), MAX_LOG_BYTES)
}

pub fn truncate_for_llm(text: &str) -> String {
    truncate_log_bytes(text.as_bytes(), LLM_LOG_BYTES).0
}

fn truncate_log_bytes(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    if bytes.len() <= max_bytes {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    let slice = &bytes[bytes.len().saturating_sub(max_bytes)..];
    let start = slice
        .iter()
        .position(|b| *b < 128 || *b >= 192)
        .unwrap_or(0);
    (String::from_utf8_lossy(&slice[start..]).into_owned(), true)
}

pub fn read_log_file(path: &Path) -> Result<(String, bool), String> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut file =
        File::open(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?
        .len() as usize;
    if len <= MAX_LOG_BYTES {
        let mut bytes = Vec::with_capacity(len);
        file.read_to_end(&mut bytes)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        return Ok(truncate_log_bytes(&bytes, MAX_LOG_BYTES));
    }
    file.seek(SeekFrom::End(-(MAX_LOG_BYTES as i64)))
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let mut buf = vec![0u8; MAX_LOG_BYTES];
    file.read_exact(&mut buf)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let (content, _) = truncate_log_bytes(&buf, MAX_LOG_BYTES);
    Ok((content, true))
}

pub fn strip_file_url(path: &str) -> &str {
    path.strip_prefix("file://").unwrap_or(path)
}

pub fn analyze_crash_log(log: &str) -> CrashAnalysisCore {
    if let Some(core) = detect_rust_compile_error(log) {
        return core;
    }
    if let Some(core) = detect_rust_panic(log) {
        return core;
    }
    if let Some(core) = detect_python_traceback(log) {
        return core;
    }
    if let Some(core) = detect_node_error(log) {
        return core;
    }
    if let Some(core) = detect_java_exception(log) {
        return core;
    }
    if let Some(core) = detect_generic_runtime(log) {
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
            .any(looks_like_source_frame)
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

fn detect_rust_compile_error(log: &str) -> Option<CrashAnalysisCore> {
    let lines: Vec<&str> = log.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with("error[E") {
            continue;
        }
        let message = trimmed
            .split_once(':')
            .map(|(_, rest)| rest.trim())
            .filter(|rest| !rest.is_empty())
            .unwrap_or(trimmed)
            .to_string();

        let mut stack = vec![trimmed.to_string()];
        let mut file = None;
        let mut line_no = None;
        let mut col = None;

        for next in lines.iter().skip(i + 1).take(6) {
            let t = next.trim();
            if t.starts_with("error[") {
                break;
            }
            if let Some((path, ln, c)) = parse_rust_location_arrow(t) {
                file = Some(normalize_path(path));
                line_no = Some(ln);
                col = Some(c);
                stack.push(t.to_string());
                break;
            }
            if t.starts_with('|') || t.starts_with('^') {
                stack.push(t.to_string());
            }
        }

        return Some(CrashAnalysisCore {
            error_type: "CompileError".into(),
            error_message: message,
            target_file: file,
            line_number: line_no,
            column_number: col,
            isolated_stack_trace: stack.join("\n"),
        });
    }
    None
}

fn detect_rust_panic(log: &str) -> Option<CrashAnalysisCore> {
    for (i, line) in log.lines().enumerate() {
        if !line.contains("panicked at") {
            continue;
        }
        let message = line
            .split("panicked at")
            .nth(1)
            .map(str::trim)
            .unwrap_or("panic")
            .trim_matches('\'')
            .to_string();

        if let Some((path, ln, c)) = parse_rust_panic_location(line) {
            let message = parse_rust_panic_quoted_message(line)
                .unwrap_or_else(|| panic_message_after_line(log, i));
            let stack: Vec<_> = log
                .lines()
                .skip(i)
                .take(5)
                .map(strip_timestamp_noise)
                .collect();
            return Some(CrashAnalysisCore {
                error_type: "Panic".into(),
                error_message: message,
                target_file: Some(normalize_path(path)),
                line_number: Some(ln),
                column_number: Some(c),
                isolated_stack_trace: stack.join("\n"),
            });
        }

        return Some(CrashAnalysisCore {
            error_type: "Panic".into(),
            error_message: message,
            target_file: None,
            line_number: None,
            column_number: None,
            isolated_stack_trace: strip_timestamp_noise(line),
        });
    }
    None
}

fn detect_python_traceback(log: &str) -> Option<CrashAnalysisCore> {
    if !log.contains("Traceback") {
        return None;
    }
    let lines: Vec<&str> = log.lines().collect();
    let mut error_line = "python exception".to_string();
    let mut error_type = "Traceback".to_string();

    for line in lines.iter().rev() {
        let t = line.trim();
        if let Some((name, msg)) = t.split_once(':') {
            if name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && !t.starts_with("File ")
            {
                error_type = name.to_string();
                error_line = msg.trim().to_string();
                break;
            }
        }
    }

    let mut last_frame = None;
    let mut last_idx = 0;
    for (i, line) in lines.iter().enumerate() {
        if let Some((path, ln)) = parse_python_file_line(line) {
            last_frame = Some((path, ln));
            last_idx = i;
        }
    }
    let (path, ln) = last_frame?;
    let stack: Vec<_> = lines
        .iter()
        .copied()
        .skip(last_idx.saturating_sub(1))
        .take(5)
        .map(strip_timestamp_noise)
        .collect();
    Some(CrashAnalysisCore {
        error_type,
        error_message: error_line,
        target_file: Some(normalize_path(path)),
        line_number: Some(ln),
        column_number: None,
        isolated_stack_trace: stack.join("\n"),
    })
}

fn detect_node_error(log: &str) -> Option<CrashAnalysisCore> {
    let lines: Vec<&str> = log.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let Some((etype, msg)) = parse_js_error_line(t) else {
            continue;
        };
        for next in lines.iter().skip(i + 1).take(6) {
            if let Some((path, ln, c)) = parse_js_stack_frame(next) {
                let stack: Vec<_> = lines
                    .iter()
                    .copied()
                    .skip(i)
                    .take(5)
                    .map(strip_timestamp_noise)
                    .collect();
                return Some(CrashAnalysisCore {
                    error_type: etype,
                    error_message: msg,
                    target_file: Some(normalize_path(path)),
                    line_number: Some(ln),
                    column_number: Some(c),
                    isolated_stack_trace: stack.join("\n"),
                });
            }
        }
        return Some(CrashAnalysisCore {
            error_type: etype,
            error_message: msg,
            target_file: None,
            line_number: None,
            column_number: None,
            isolated_stack_trace: strip_timestamp_noise(t),
        });
    }
    None
}

fn detect_java_exception(log: &str) -> Option<CrashAnalysisCore> {
    let lines: Vec<&str> = log.lines().collect();
    let (header_idx, error_type, error_message) =
        lines.iter().enumerate().find_map(|(i, line)| {
            parse_java_exception_header(line).map(|(etype, msg)| (i, etype, msg))
        })?;

    for line in lines.iter().skip(header_idx + 1).take(8) {
        let Some((path, ln, col)) = parse_java_stack_frame(line) else {
            continue;
        };
        let stack: Vec<_> = lines
            .iter()
            .copied()
            .skip(header_idx)
            .take(5)
            .map(strip_timestamp_noise)
            .collect();
        return Some(CrashAnalysisCore {
            error_type,
            error_message,
            target_file: Some(normalize_path(path)),
            line_number: Some(ln),
            column_number: col,
            isolated_stack_trace: stack.join("\n"),
        });
    }
    None
}

fn parse_java_exception_header(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if !(t.contains("Exception in thread") || t.contains("java.lang.")) {
        return None;
    }
    let marker = t.find("java.lang.").or_else(|| {
        t.split_whitespace()
            .find(|token| token.contains("Exception"))
            .map(|token| t.find(token).unwrap_or(0))
    })?;
    let rest = &t[marker..];
    let (etype_part, msg) = rest.split_once(':')?;
    let etype = etype_part
        .trim()
        .rsplit('.')
        .next()
        .filter(|name| name.contains("Exception"))
        .unwrap_or(etype_part.trim())
        .to_string();
    let msg = msg.trim();
    if msg.is_empty() {
        return None;
    }
    Some((etype, msg.to_string()))
}

fn parse_java_stack_frame(line: &str) -> Option<(&str, u32, Option<u32>)> {
    let t = line.trim();
    if !t.starts_with("at ") {
        return None;
    }
    let location = t
        .rsplit_once('(')
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .filter(|inside| inside.contains(".java"))?;
    let location = location.trim();
    if let Some((path, ln, col)) = parse_path_line_col(location) {
        if path.ends_with(".java") {
            return Some((path, ln, Some(col)));
        }
    }
    let (file, line_str) = location.rsplit_once(':')?;
    if !file.ends_with(".java") {
        return None;
    }
    Some((file, line_str.parse().ok()?, None))
}

fn detect_generic_runtime(log: &str) -> Option<CrashAnalysisCore> {
    for line in log.lines() {
        let t = line.trim();
        if t.contains("FATAL")
            || t.contains("Exception in thread")
            || (t.contains("Exception") && t.contains(':'))
        {
            return Some(CrashAnalysisCore {
                error_type: "RuntimeError".into(),
                error_message: t.to_string(),
                target_file: None,
                line_number: None,
                column_number: None,
                isolated_stack_trace: strip_timestamp_noise(t),
            });
        }
    }
    None
}

fn parse_rust_location_arrow(line: &str) -> Option<(&str, u32, u32)> {
    let rest = line.trim().strip_prefix("-->")?.trim();
    parse_path_line_col(rest)
}

fn parse_path_line_col(location: &str) -> Option<(&str, u32, u32)> {
    let (line_col, col_str) = location.rsplit_once(':')?;
    let (path, line_str) = line_col.rsplit_once(':')?;
    Some((path.trim(), line_str.parse().ok()?, col_str.parse().ok()?))
}

fn parse_rust_panic_location(line: &str) -> Option<(&str, u32, u32)> {
    let after = line.split("panicked at").nth(1)?.trim();
    if let Some(rest) = after.strip_prefix('\'') {
        if let Some((_, tail)) = rest.split_once('\'') {
            if let Some(loc) = extract_path_line_col(tail) {
                return Some(loc);
            }
        }
    }
    extract_path_line_col(after)
}

fn extract_path_line_col(s: &str) -> Option<(&str, u32, u32)> {
    let trimmed = s.trim().trim_end_matches(':');
    for token in trimmed.split([',', ' ']) {
        let token = token.trim().trim_end_matches(':');
        if token.is_empty() {
            continue;
        }
        if let Some(parsed) = parse_path_line_col(token) {
            if token.contains('.') {
                return Some(parsed);
            }
        }
    }
    parse_path_line_col(trimmed)
}

fn parse_rust_panic_quoted_message(line: &str) -> Option<String> {
    let after = line.split("panicked at").nth(1)?.trim();
    let rest = after.strip_prefix('\'')?;
    let (msg, _) = rest.split_once('\'')?;
    let msg = msg.trim();
    (!msg.is_empty()).then(|| msg.to_string())
}

fn panic_message_after_line(log: &str, panic_line_idx: usize) -> String {
    log.lines()
        .skip(panic_line_idx + 1)
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with("stack backtrace")
                && !line.chars().all(|c| c.is_ascii_digit() || c == ':')
        })
        .map(str::to_string)
        .unwrap_or_else(|| "panic".into())
}

fn parse_python_file_line(line: &str) -> Option<(&str, u32)> {
    let t = line.trim();
    if !t.starts_with("File \"") {
        return None;
    }
    let inner = t.strip_prefix("File \"")?.split('"').next()?;
    let line_no = t
        .split("line ")
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((inner, line_no))
}

fn parse_js_error_line(line: &str) -> Option<(String, String)> {
    let (name, msg) = line.split_once(':')?;
    if !name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return None;
    }
    if matches!(
        name,
        "TypeError" | "ReferenceError" | "SyntaxError" | "RangeError" | "Error"
    ) {
        Some((name.to_string(), msg.trim().to_string()))
    } else {
        None
    }
}

fn parse_js_stack_frame(line: &str) -> Option<(&str, u32, u32)> {
    let t = line.trim();
    let paren = t.find('(')?;
    let inside = t[paren + 1..].strip_suffix(')')?;
    parse_path_line_col(inside)
}

fn normalize_path(path: &str) -> String {
    let p = path.trim().trim_start_matches("./");
    if p.starts_with("frontend/") || p.starts_with("lib/") {
        return p.to_string();
    }
    for marker in ["/frontend/", "/lib/", "/src/", "/tests/"] {
        if let Some(idx) = p.find(marker) {
            return p[idx + 1..].to_string();
        }
    }
    p.to_string()
}

fn strip_timestamp_noise(line: &str) -> String {
    let t = line.trim();
    if let Some(rest) = t
        .strip_prefix('[')
        .and_then(|s| s.find(']').map(|i| &s[i + 1..]))
    {
        return rest.trim().to_string();
    }
    t.to_string()
}

fn looks_like_source_frame(line: &str) -> bool {
    line.contains(".rs:")
        || line.contains(".ts:")
        || line.contains(".tsx:")
        || line.contains(".js:")
        || line.contains(".py:")
        || line.contains(".java:")
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
        let log = include_str!("../../tests/fixtures/logs/v2/java_null_pointer.log");
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
    fn normalize_path_keeps_frontend_prefix() {
        assert_eq!(
            normalize_path("frontend/src/modules/config-ui/ConfigApp.tsx"),
            "frontend/src/modules/config-ui/ConfigApp.tsx"
        );
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

    #[test]
    fn truncate_for_llm_caps_large_input() {
        let log = "a".repeat(LLM_LOG_BYTES + 500);
        let capped = truncate_for_llm(&log);
        assert!(capped.len() <= LLM_LOG_BYTES + 4);
        assert!(capped.ends_with('a'));
    }

    #[test]
    fn read_log_file_truncates_large_files() {
        let dir = std::env::temp_dir().join(format!("crash-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let path = dir.join("big.log");
        let payload = "x".repeat(MAX_LOG_BYTES + 1_000);
        std::fs::write(&path, &payload).expect("write");
        let (content, truncated) = read_log_file(&path).expect("read");
        assert!(truncated);
        assert!(content.len() <= MAX_LOG_BYTES + 4);
        std::fs::remove_dir_all(dir).ok();
    }
}
