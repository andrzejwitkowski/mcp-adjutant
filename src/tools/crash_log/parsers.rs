use super::CrashAnalysisCore;

fn core(
    error_type: impl Into<String>,
    error_message: impl Into<String>,
    target_file: Option<String>,
    line_number: Option<u32>,
    column_number: Option<u32>,
    isolated_stack_trace: impl Into<String>,
) -> CrashAnalysisCore {
    CrashAnalysisCore {
        error_type: error_type.into(),
        error_message: error_message.into(),
        target_file,
        line_number,
        column_number,
        isolated_stack_trace: isolated_stack_trace.into(),
        failing_test: None,
        command: None,
        exit_code: None,
    }
}

pub(crate) fn detect_cargo_fmt_check(log: &str) -> Option<CrashAnalysisCore> {
    let mentions_fmt = log.contains("cargo fmt") || log.contains("rustfmt");
    let has_diff = log.lines().any(|line| {
        let cleaned = strip_timestamp_noise(line);
        cleaned.contains("Diff in ") && cleaned.contains(" at line ")
    });
    if !mentions_fmt && !has_diff {
        return None;
    }

    let lines: Vec<String> = log.lines().map(strip_timestamp_noise).collect();
    for (i, cleaned) in lines.iter().enumerate() {
        let Some(rest) = find_diff_in_suffix(cleaned) else {
            continue;
        };
        let Some((path_part, after_line)) = rest.split_once(" at line ") else {
            continue;
        };
        let line_no: u32 = after_line.trim().trim_end_matches(':').parse().ok()?;
        let path = normalize_path(path_part.trim());
        let mut stack = vec![format!("Diff in {path} at line {line_no}:")];
        for next in lines.iter().skip(i + 1).take(6) {
            let t = next.trim();
            if t.starts_with('+') || t.starts_with('-') || t.starts_with(' ') {
                stack.push(t.to_string());
            } else if t.starts_with("Diff in ") {
                break;
            }
        }
        return Some(fmt_check_core(Some(path), Some(line_no), stack.join("\n")));
    }

    mentions_fmt.then(|| fmt_check_core(None, None, String::new()))
}

fn fmt_check_core(
    target_file: Option<String>,
    line_number: Option<u32>,
    isolated_stack_trace: String,
) -> CrashAnalysisCore {
    CrashAnalysisCore {
        error_type: "FormattingCheckFailed".into(),
        error_message: "cargo fmt -- --check reported formatting differences (exit code 1)".into(),
        target_file,
        line_number,
        column_number: None,
        isolated_stack_trace,
        failing_test: None,
        command: Some("cargo fmt -- --check".into()),
        exit_code: Some(1),
    }
}

fn find_diff_in_suffix(line: &str) -> Option<&str> {
    let idx = line.find("Diff in ")?;
    Some(&line[idx + "Diff in ".len()..])
}

pub(crate) fn detect_rust_compile_error(log: &str) -> Option<CrashAnalysisCore> {
    let lines: Vec<&str> = log.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let cleaned = strip_timestamp_noise(line);
        let trimmed = cleaned.trim();
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
            let t = strip_timestamp_noise(next);
            let t = t.trim();
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

        return Some(core(
            "CompileError",
            message,
            file,
            line_no,
            col,
            stack.join("\n"),
        ));
    }
    None
}

pub(crate) fn detect_rust_panic(log: &str) -> Option<CrashAnalysisCore> {
    for (i, line) in log.lines().enumerate() {
        let cleaned = strip_timestamp_noise(line);
        if !cleaned.contains("panicked at") {
            continue;
        }
        let failing_test = extract_failing_test(&cleaned);

        if let Some((path, ln, c)) = parse_rust_panic_location(&cleaned) {
            let message = parse_rust_panic_quoted_message(&cleaned)
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
                failing_test,
                command: None,
                exit_code: None,
            });
        }

        let message = cleaned
            .split("panicked at")
            .nth(1)
            .map(str::trim)
            .unwrap_or("panic")
            .trim_matches('\'')
            .to_string();
        return Some(CrashAnalysisCore {
            error_type: "Panic".into(),
            error_message: strip_timestamp_noise(&message),
            target_file: None,
            line_number: None,
            column_number: None,
            isolated_stack_trace: cleaned,
            failing_test,
            command: None,
            exit_code: None,
        });
    }
    None
}

pub(crate) fn extract_failing_test(line: &str) -> Option<String> {
    let after = line.split("thread '").nth(1)?;
    let (name, _) = after.split_once('\'')?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub(crate) fn detect_python_traceback(log: &str) -> Option<CrashAnalysisCore> {
    if !log.contains("Traceback") {
        return None;
    }
    let lines: Vec<&str> = log.lines().collect();
    let mut error_line = "python exception".to_string();
    let mut error_type = "Traceback".to_string();

    for line in lines.iter().rev() {
        let t = strip_timestamp_noise(line);
        let t = t.trim();
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
        let cleaned = strip_timestamp_noise(line);
        if let Some((path, ln)) = parse_python_file_line(&cleaned) {
            last_frame = Some((path.to_string(), ln));
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
    Some(core(
        error_type,
        error_line,
        Some(normalize_path(&path)),
        Some(ln),
        None,
        stack.join("\n"),
    ))
}

pub(crate) fn detect_node_error(log: &str) -> Option<CrashAnalysisCore> {
    let lines: Vec<&str> = log.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let cleaned = strip_timestamp_noise(line);
        let t = cleaned.trim();
        let Some((etype, msg)) = parse_js_error_line(t) else {
            continue;
        };
        for next in lines.iter().skip(i + 1).take(6) {
            let cleaned_next = strip_timestamp_noise(next);
            if let Some((path, ln, c)) = parse_js_stack_frame(&cleaned_next) {
                let path = normalize_path(path);
                let stack: Vec<_> = lines
                    .iter()
                    .copied()
                    .skip(i)
                    .take(5)
                    .map(strip_timestamp_noise)
                    .collect();
                return Some(core(
                    etype,
                    msg,
                    Some(path),
                    Some(ln),
                    Some(c),
                    stack.join("\n"),
                ));
            }
        }
        return Some(core(etype, msg, None, None, None, t.to_string()));
    }
    None
}

pub(crate) fn detect_java_exception(log: &str) -> Option<CrashAnalysisCore> {
    let lines: Vec<&str> = log.lines().collect();
    let (header_idx, error_type, error_message) =
        lines.iter().enumerate().find_map(|(i, line)| {
            parse_java_exception_header(&strip_timestamp_noise(line))
                .map(|(etype, msg)| (i, etype, msg))
        })?;

    for line in lines.iter().skip(header_idx + 1).take(8) {
        let cleaned = strip_timestamp_noise(line);
        let Some((path, ln, col)) = parse_java_stack_frame(&cleaned) else {
            continue;
        };
        let path = normalize_path(path);
        let stack: Vec<_> = lines
            .iter()
            .copied()
            .skip(header_idx)
            .take(5)
            .map(strip_timestamp_noise)
            .collect();
        return Some(core(
            error_type,
            error_message,
            Some(path),
            Some(ln),
            col,
            stack.join("\n"),
        ));
    }
    None
}

pub(crate) fn detect_generic_runtime(log: &str) -> Option<CrashAnalysisCore> {
    for line in log.lines() {
        let t = strip_timestamp_noise(line);
        let t = t.trim();
        if t.contains("FATAL")
            || t.contains("Exception in thread")
            || (t.contains("Exception") && t.contains(':'))
        {
            return Some(core(
                "RuntimeError",
                t.to_string(),
                None,
                None,
                None,
                t.to_string(),
            ));
        }
    }
    None
}

pub(crate) fn looks_like_source_frame(line: &str) -> bool {
    line.contains(".rs:")
        || line.contains(".ts:")
        || line.contains(".tsx:")
        || line.contains(".js:")
        || line.contains(".py:")
        || line.contains(".java:")
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
        .map(strip_timestamp_noise)
        .map(|line| line.trim().to_string())
        .find(|line| {
            !line.is_empty()
                && !line.starts_with("stack backtrace")
                && !line.chars().all(|c| c.is_ascii_digit() || c == ':')
        })
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

/// Strip `[ts]` brackets and GitHub Actions `job\tstep\tISO8601Z ` prefixes.
pub(crate) fn strip_timestamp_noise(line: &str) -> String {
    let t = line.trim();
    let after_bracket = if let Some(rest) = t
        .strip_prefix('[')
        .and_then(|s| s.find(']').map(|i| &s[i + 1..]))
    {
        rest.trim()
    } else {
        t
    };
    strip_gh_actions_tsv(after_bracket)
}

fn strip_gh_actions_tsv(line: &str) -> String {
    let mut parts = line.splitn(4, '\t');
    let Some(_job) = parts.next() else {
        return line.to_string();
    };
    let Some(_step) = parts.next() else {
        return line.to_string();
    };
    let Some(third) = parts.next() else {
        return line.to_string();
    };
    if let Some(after_ts) = strip_iso8601z_prefix(third) {
        if !after_ts.is_empty() {
            return after_ts.to_string();
        }
        if let Some(msg) = parts.next() {
            return msg.trim().to_string();
        }
        return String::new();
    }
    line.to_string()
}

fn strip_iso8601z_prefix(s: &str) -> Option<&str> {
    let z = s.find('Z')?;
    let ts = &s[..=z];
    if !looks_like_iso8601_ts(ts) {
        return None;
    }
    Some(s[z + 1..].trim_start())
}

fn looks_like_iso8601_ts(ts: &str) -> bool {
    let bytes = ts.as_bytes();
    bytes.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && ts.ends_with('Z')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_keeps_frontend_prefix() {
        assert_eq!(
            normalize_path("frontend/src/modules/config-ui/ConfigApp.tsx"),
            "frontend/src/modules/config-ui/ConfigApp.tsx"
        );
    }

    #[test]
    fn strip_gh_actions_tsv_prefix() {
        let line = "Rust Backend (Check & Test)\tRun Backend Tests\t2026-07-22T12:28:56.3353494Z assertion failed: adjutant_dir.is_dir()";
        assert_eq!(
            strip_timestamp_noise(line),
            "assertion failed: adjutant_dir.is_dir()"
        );
    }
}
