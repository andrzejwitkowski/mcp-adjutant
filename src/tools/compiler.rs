use std::path::Path;
use std::process::Command;

pub fn run_build_command(dir: &Path, command: &str) -> Result<String, String> {
    // ponytail: sh -c keeps one spawn path for arbitrary compiler commands
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .map_err(|err| format!("failed to spawn build command: {err}"))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if output.status.success() {
        Ok(combined)
    } else {
        Err(combined)
    }
}

/// Truncates long build logs by keeping the tail (errors usually appear at the end).
/// No language- or compiler-specific filtering — avoids dropping relevant lines for
/// Java, Python, nvcc, etc.
pub fn truncate_build_log(output: &str, max_lines: usize, max_bytes: usize) -> (String, bool) {
    let lines: Vec<&str> = output.lines().collect();
    let line_truncated = lines.len() > max_lines;
    let mut tail = if line_truncated {
        lines[lines.len() - max_lines..].join("\n")
    } else {
        output.to_string()
    };

    let byte_truncated = tail.len() > max_bytes;
    if byte_truncated {
        let mut start = tail.len().saturating_sub(max_bytes);
        while start < tail.len() && !tail.is_char_boundary(start) {
            start += 1;
        }
        tail = tail[start..].to_string();
    }

    (tail, line_truncated || byte_truncated)
}

pub fn edit_file_line(path: &Path, line_number: usize, new_content: &str) -> Result<(), String> {
    if line_number == 0 {
        return Err("line_number must be >= 1".to_string());
    }

    let content = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let had_trailing_newline = content.ends_with('\n');
    let uses_crlf = content.contains("\r\n");
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();

    if line_number > lines.len() {
        return Err(format!(
            "line {line_number} out of range (file has {} lines)",
            lines.len()
        ));
    }

    lines[line_number - 1] = new_content.to_string();
    let separator = if uses_crlf { "\r\n" } else { "\n" };
    let mut updated = lines.join(separator);
    if had_trailing_newline {
        updated.push_str(separator);
    }

    std::fs::write(path, updated)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn edit_file_line_replaces_target_line() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mcp-adjutant-edit-{nanos}.txt"));
        fs::write(&path, "alpha\nbeta\ngamma\n").expect("write");

        edit_file_line(&path, 2, "bravo").expect("edit");
        let updated = fs::read_to_string(&path).expect("read");
        assert_eq!(updated, "alpha\nbravo\ngamma\n");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn edit_file_line_preserves_crlf_line_endings() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mcp-adjutant-edit-crlf-{nanos}.txt"));
        fs::write(&path, "alpha\r\nbeta\r\n").expect("write");

        edit_file_line(&path, 2, "bravo").expect("edit");
        let updated = fs::read_to_string(&path).expect("read");
        assert_eq!(updated, "alpha\r\nbravo\r\n");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn truncate_build_log_keeps_tail_without_language_filtering() {
        let log = (0..150)
            .map(|i| format!("noise line {i}"))
            .chain([
                "error: expected ';'".to_string(),
                "  --> src/main.java:42:5".to_string(),
                "kernel.cu(12): error: identifier not found".to_string(),
            ])
            .collect::<Vec<_>>()
            .join("\n");

        let (truncated, was_truncated) = truncate_build_log(&log, 10, 4096);
        assert!(was_truncated);
        assert!(truncated.contains("main.java"));
        assert!(truncated.contains("kernel.cu"));
        assert!(!truncated.contains("noise line 0"));
    }

    #[test]
    fn truncate_build_log_passes_short_output_unchanged() {
        let log = "error: one failure\nnote: at foo.py:3";
        let (truncated, was_truncated) = truncate_build_log(log, 120, 16_384);
        assert!(!was_truncated);
        assert_eq!(truncated, log);
    }
}
