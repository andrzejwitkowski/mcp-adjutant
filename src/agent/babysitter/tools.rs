use serde_json::Value;

use crate::llm::{required_str, LlmTool, LlmToolSet, ToolDefinition};

struct HarnessTool {
    definition: ToolDefinition,
    terminal: bool,
}

impl HarnessTool {
    fn new(definition: ToolDefinition, terminal: bool) -> Self {
        Self {
            definition,
            terminal,
        }
    }
}

impl LlmTool for HarnessTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        if self.terminal {
            let summary = arguments
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("session finalized");
            return Ok(summary.to_string());
        }
        Err(format!(
            "{} is executed by BabysitterAgent",
            self.definition.name
        ))
    }

    fn is_terminal(&self) -> bool {
        self.terminal
    }
}

pub fn babysitter_tool_set() -> LlmToolSet {
    LlmToolSet::new()
        .register(HarnessTool::new(
            ToolDefinition::new(
                "github_get_pr_state",
                "Fetches remote CI statuses and unresolved PR review comments.",
            ),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "run_log_analyzer",
                "Analyzes a CI log (local path, https:// URL, or gh-run:<run_id>) and returns root-cause JSON.",
            )
            .string_param(
                "log_path",
                "Log source: file path, https:// URL, or gh-run:<run_id>.",
                true,
            ),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "invoke_child_triage",
                "Runs a nested TriageAgent loop on target files with error context from CI or review.",
            )
            .string_array_param("target_paths", "Files to fix.", true)
            .string_param("error_context", "Compile/lint error or review fix context.", true),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "git_push_changes",
                "Pushes the current branch to origin after child triage reported green builds.",
            ),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "github_post_final_report",
                "Posts the structured babysitter markdown report as a PR comment.",
            )
            .string_param("report", "Markdown report body.", true),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new("finalize_session", "Ends the babysitter session (terminal).")
                .string_param("summary", "Optional one-line session summary.", false)
                .string_array_param(
                    "skipped_review_paths",
                    "Review comment file paths intentionally not triaged ([NITPICK_OR_IGNORE] / [ARCHITECTURAL_DISCUSSION]).",
                    false,
                ),
            true,
        ))
}

pub fn parse_log_path(arguments: &Value) -> Result<String, String> {
    required_str(arguments, "log_path")
}

pub fn parse_triage_arguments(arguments: &Value) -> Result<(Vec<String>, String), String> {
    let paths = arguments
        .get("target_paths")
        .and_then(Value::as_array)
        .ok_or_else(|| "target_paths array is required".to_string())?
        .iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Err("target_paths must not be empty".to_string());
    }
    let error_context = required_str(arguments, "error_context")?;
    Ok((paths, error_context))
}

pub fn parse_report_body(arguments: &Value) -> Result<String, String> {
    required_str(arguments, "report")
}

pub fn parse_finalize_arguments(
    arguments: &Value,
) -> Result<(Option<String>, Vec<String>), String> {
    let summary = arguments
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_string);
    let skipped_review_paths = arguments
        .get("skipped_review_paths")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok((summary, skipped_review_paths))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_finalize_arguments_reads_optional_fields() {
        let args = json!({
            "summary": "done",
            "skipped_review_paths": ["src/a.rs"]
        });
        let (summary, skipped) = parse_finalize_arguments(&args).expect("parse");
        assert_eq!(summary.as_deref(), Some("done"));
        assert_eq!(skipped, vec!["src/a.rs"]);
    }

    #[test]
    fn parse_finalize_arguments_defaults_skipped_to_empty() {
        let (summary, skipped) = parse_finalize_arguments(&json!({})).expect("parse");
        assert!(summary.is_none());
        assert!(skipped.is_empty());
    }
}
