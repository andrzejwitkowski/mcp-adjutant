use super::traits::AgentContext;

pub const TRIAGE_PASS_MARKER: &str = "[TRIAGE PASS]";

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
