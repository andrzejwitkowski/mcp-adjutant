use super::CrashAnalysisCore;

pub(crate) fn detect_rust_compile_error(log: &str) -> Option<CrashAnalysisCore> {
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

pub(crate) fn detect_rust_panic(log: &str) -> Option<CrashAnalysisCore> {
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

pub(crate) fn detect_python_traceback(log: &str) -> Option<CrashAnalysisCore> {
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

pub(crate) fn detect_node_error(log: &str) -> Option<CrashAnalysisCore> {
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

pub(crate) fn detect_java_exception(log: &str) -> Option<CrashAnalysisCore> {
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

pub(crate) fn detect_generic_runtime(log: &str) -> Option<CrashAnalysisCore> {
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

#[cfg(test)]
mod tests {
    use super::normalize_path;

    #[test]
    fn normalize_path_keeps_frontend_prefix() {
        assert_eq!(
            normalize_path("frontend/src/modules/config-ui/ConfigApp.tsx"),
            "frontend/src/modules/config-ui/ConfigApp.tsx"
        );
    }
}
