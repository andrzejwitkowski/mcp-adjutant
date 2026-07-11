use super::traits::AgentContext;

pub const TRIAGE_PASS_MARKER: &str = "[TRIAGE PASS]";

pub fn triage_passed(context: &AgentContext) -> bool {
    context.input_prompt.contains(TRIAGE_PASS_MARKER)
        || context
            .input_prompt
            .contains("All builds/tests completed successfully.")
}

pub fn format_triage_success(context: &AgentContext) -> String {
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

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_triage_success_includes_build_evidence() {
        let context = AgentContext {
            input_prompt: TRIAGE_PASS_MARKER.to_string(),
            accumulated_data: "Triage targets (1 modules):\nbackend => cargo test\n\nBuild OK in /tmp/backend (`cargo test`):\ntest ok\n".to_string(),
            iterations: 1,
            max_iterations: 3,
            is_finished: true,
            agent_completed: false,
            touched_files: vec![],
            last_tool_call: None,
        };

        let report = format_triage_success(&context);
        assert!(report.contains("## Triage: PASS"));
        assert!(report.contains("Build OK in /tmp/backend"));
        assert!(report.contains("cargo test"));
        assert!(report.contains("test ok"));
    }
}
