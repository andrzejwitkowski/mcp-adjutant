use std::path::{Path, PathBuf};

use super::traits::AgentContext;

pub const TRIAGE_PASS_MARKER: &str = "[TRIAGE PASS]";
pub const BUILDER_GREEN_MARKER: &str = "[BUILDER GREEN OK]";
const BUILDER_FAIL_MARKER: &str = "[BUILDER FAIL EVIDENCE]";
const TRIAGE_RESULT_MARKER: &str = "\n[TRIAGE RESULT]: ";
const DEBUG_TRACE_MAX: usize = 2_000;
const LOG_EXCERPT_LINES: usize = 40;

pub struct BuilderReportInput<'a> {
    pub accumulated_data: &'a str,
    pub project_root: &'a Path,
    pub source_file_path: &'a str,
    pub test_type: &'a str,
    pub green_ok: bool,
    pub verify_summary: Option<&'a str>,
}

pub fn format_builder_report(input: &BuilderReportInput<'_>) -> String {
    let test_path = extract_last_triage_test_path(input.accumulated_data);
    let rel_path = test_path
        .as_ref()
        .and_then(|path| relativize_under_root(path, input.project_root));
    let test_source = rel_path
        .as_ref()
        .map(|path| input.project_root.join(path))
        .and_then(|abs| std::fs::read_to_string(&abs).ok());

    let triage_block = extract_last_triage_block(input.accumulated_data);
    let (command, exit_code, build_log) =
        triage_block.map(parse_triage_evidence).unwrap_or_default();

    let mut report = String::new();
    report.push_str(&format!(
        "## PHASE_4_BUILDER Report: {source} ({test_type})\n\n",
        source = input.source_file_path,
        test_type = input.test_type
    ));

    report.push_str("[TEST SOURCE]\n");
    if let Some(path) = &rel_path {
        report.push_str(path);
        report.push('\n');
        if let Some(source) = &test_source {
            report.push_str(source.trim());
            report.push('\n');
        } else {
            report.push_str("(test file not readable on disk)\n");
        }
    } else {
        report.push_str("(no test file path in builder log)\n");
    }

    report.push_str("\n[BUILD COMMAND & EXIT CODE]\n");
    if let Some(summary) = input.verify_summary.filter(|s| !s.is_empty()) {
        report.push_str(summary);
        report.push('\n');
    } else if let Some(cmd) = &command {
        report.push_str(&format!("$ {cmd}\n"));
        if let Some(code) = exit_code {
            report.push_str(&format!("exit {code}\n"));
        }
        if !build_log.is_empty() {
            report.push_str(&build_log);
            report.push('\n');
        }
    } else {
        report.push_str("(no build command captured)\n");
    }

    report.push_str("\n[LOG EXCERPT]\n");
    let excerpt = if !build_log.is_empty() {
        tail_lines(&build_log, LOG_EXCERPT_LINES)
    } else {
        tail_lines(input.accumulated_data, LOG_EXCERPT_LINES)
    };
    if excerpt.trim().is_empty() {
        report.push_str("(no log excerpt)\n");
    } else {
        report.push_str(excerpt.trim());
        report.push('\n');
    }

    if input.green_ok
        && input.verify_summary.is_some()
        && input.accumulated_data.contains(BUILDER_GREEN_MARKER)
    {
        report.push('\n');
        report.push_str(BUILDER_GREEN_MARKER);
        report.push('\n');
    } else if input.accumulated_data.contains(BUILDER_FAIL_MARKER) {
        report.push_str("\n[BUILDER FAIL EVIDENCE]\n");
        if let Some(block) = input.accumulated_data.split(BUILDER_FAIL_MARKER).nth(1) {
            report.push_str(block.trim());
            report.push('\n');
        }
    } else if !input.green_ok {
        report.push_str("\n(no GREEN — see debug trace)\n");
    }

    report.push_str("\n## Debug trace\n");
    report.push_str(&truncate_debug_trace(
        input.accumulated_data,
        DEBUG_TRACE_MAX,
    ));
    report
}

fn extract_last_triage_test_path(log: &str) -> Option<String> {
    log.lines()
        .filter(|line| line.contains("[SYSTEM]: Launching Triage"))
        .filter_map(|line| line.rsplit(" for ").next())
        .map(str::trim)
        .rfind(|path| !path.is_empty())
        .map(str::to_string)
}

fn extract_last_triage_block(log: &str) -> Option<&str> {
    let start = log.rfind(TRIAGE_RESULT_MARKER)? + TRIAGE_RESULT_MARKER.len();
    let rest = &log[start..];
    let end = rest
        .find("\n[SYSTEM]:")
        .or_else(|| rest.find("\n[RED OK]:"))
        .or_else(|| rest.find("\n[BUILDER GREEN OK]"))
        .or_else(|| rest.find("\n[TRIAGE FAILURE]"))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn parse_triage_evidence(block: &str) -> (Option<String>, Option<i32>, String) {
    let mut command = None;
    let mut exit_code = None;
    let mut build_log = String::new();
    let mut capture = false;

    for line in block.lines() {
        if let Some(cmd) = line
            .strip_prefix("Command: `")
            .and_then(|s| s.strip_suffix('`'))
        {
            command = Some(cmd.to_string());
            capture = false;
        } else if let Some(cmd) = backtick_command(line) {
            command.get_or_insert(cmd);
            capture = false;
        } else if let Some(rest) = line.strip_prefix("Exit code: ") {
            exit_code = rest.trim().parse().ok();
            capture = true;
            continue;
        } else if line == "Build output:" || line.starts_with("Build FAILED:") {
            capture = true;
            continue;
        }
        if capture {
            build_log.push_str(line);
            build_log.push('\n');
        }
    }

    (command, exit_code, build_log)
}

fn backtick_command(line: &str) -> Option<String> {
    let start = line.find("(`")? + 2;
    let rest = line.get(start..)?;
    let end = rest.find("`)")?;
    Some(rest[..end].to_string())
}

fn relativize_under_root(path: &str, root: &Path) -> Option<String> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
            .strip_prefix(root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
    } else {
        Some(path.replace('\\', "/"))
    }
}

fn tail_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        text.to_string()
    } else {
        lines[lines.len() - max_lines..].join("\n")
    }
}

fn truncate_debug_trace(log: &str, max_chars: usize) -> String {
    if log.len() <= max_chars {
        log.to_string()
    } else {
        format!(
            "(truncated to last {max_chars} chars)\n{}",
            &log[log.len() - max_chars..]
        )
    }
}

pub fn triage_passed(context: &AgentContext) -> bool {
    context.input_prompt.contains(TRIAGE_PASS_MARKER)
        || context
            .input_prompt
            .contains("All builds/tests completed successfully.")
}

pub fn format_triage_success(
    context: &AgentContext,
    target_paths: &[std::path::PathBuf],
) -> String {
    let mut report = format!("## Triage: PASS\n\n{TRIAGE_PASS_MARKER}\n\n");

    if !context.accumulated_data.is_empty() {
        report.push_str("### Verification log\n\n");
        report.push_str(context.accumulated_data.trim());
        report.push('\n');
    } else {
        report.push_str("### Verification log\n\n(no build output captured)\n");
    }

    report.push_str("\n### Summary\n\n");
    report.push_str(&format!(
        "- Status: all configured build/test commands exited successfully\n- Iterations: {}\n",
        context.iterations
    ));
    if !target_paths.is_empty() {
        let list = target_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        report.push_str(&format!("- Target files verified: {list}\n"));
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_triage_evidence_handles_tdd_red_lines() {
        let block = "TDD RED assertion failure (expected) in /repo (`cargo test --test foo`):\nExit code: 101\nassertion failed\n";
        let (cmd, code, log) = parse_triage_evidence(block);
        assert_eq!(cmd.as_deref(), Some("cargo test --test foo"));
        assert_eq!(code, Some(101));
        assert!(log.contains("assertion failed"));
    }

    #[test]
    fn format_builder_report_structured_green() {
        let fixture = "\
Tool: write_test_suite({\"path\":\"tests/foo_integration_test.rs\"})\n\
\n[SYSTEM]: Launching Triage (green) for tests/foo_integration_test.rs\n\
\n[TRIAGE RESULT]: Workspace: /repo\nCommand: `cargo test --test foo_integration_test`\nExit code: 0\nBuild output:\nrunning 1 test\ntest foo ... ok\n\n\
[BUILDER GREEN OK]\n";
        let dir = std::env::temp_dir().join(format!("builder-report-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tests")).expect("mkdir");
        std::fs::write(dir.join("tests/foo_integration_test.rs"), "fn test_x() {}").expect("write");
        let report = format_builder_report(&BuilderReportInput {
            accumulated_data: fixture,
            project_root: &dir,
            source_file_path: "src/foo.rs",
            test_type: "unit",
            green_ok: true,
            verify_summary: Some("cargo test --test foo_integration_test: all tests passed"),
        });
        assert!(report.contains("[TEST SOURCE]"));
        assert!(report.contains("tests/foo_integration_test.rs"));
        assert!(report.contains("fn test_x()"));
        assert!(report.contains("[BUILD COMMAND & EXIT CODE]"));
        assert!(report.contains("cargo test --test foo_integration_test"));
        assert!(report.contains("[BUILDER GREEN OK]"));
        assert!(!report.starts_with("Tool: write_test_suite"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_triage_success_includes_build_evidence() {
        let context = AgentContext {
            input_prompt: TRIAGE_PASS_MARKER.to_string(),
            accumulated_data: "Triage targets (1 modules):\nbackend => npm run typecheck\n\nWorkspace: /tmp/backend\nCommand: `npm run typecheck`\nExit code: 0\nBuild output:\ntest ok\n".to_string(),
            iterations: 1,
            max_iterations: 3,
            is_finished: true,
            agent_completed: false,
            touched_files: vec![],
            last_tool_call: None,
        };
        let paths = vec![
            std::path::PathBuf::from("src/foo.ts"),
            std::path::PathBuf::from("src/bar.ts"),
        ];

        let report = format_triage_success(&context, &paths);
        assert!(report.contains("## Triage: PASS"));
        assert!(report.contains("npm run typecheck"));
        assert!(report.contains("Exit code: 0"));
        assert!(report.contains("Target files verified: src/foo.ts, src/bar.ts"));
    }

    #[test]
    fn format_triage_success_empty_paths_omits_line() {
        let context = AgentContext {
            input_prompt: TRIAGE_PASS_MARKER.to_string(),
            accumulated_data: "Build OK".to_string(),
            iterations: 1,
            max_iterations: 3,
            is_finished: true,
            agent_completed: false,
            touched_files: vec![],
            last_tool_call: None,
        };

        let report = format_triage_success(&context, &[]);
        assert!(report.contains("## Triage: PASS"));
        assert!(!report.contains("Target files verified"));
    }
}
