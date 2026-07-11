use std::path::PathBuf;

use serde_json::Value;

use crate::llm::{required_str, LlmTool, LlmToolSet, ToolDefinition};

pub fn triage_tool_set() -> LlmToolSet {
    LlmToolSet::new()
        .register(EditFileTool::new())
        .register(ReportArchitecturalErrorTool::new())
}

struct EditFileTool {
    definition: ToolDefinition,
}

impl EditFileTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "edit_file",
                "Replaces one file line (1-based numbering). Use only for trivial fixes.",
            )
            .string_param("path", "File path (relative to module or absolute).", true)
            .integer_param("line", "Line number to replace (>= 1).", true)
            .string_param("content", "New line contents.", true),
        }
    }
}

impl LlmTool for EditFileTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, _arguments: &Value) -> Result<String, String> {
        Err("edit_file is executed by TriageAgent".to_string())
    }
}

struct ReportArchitecturalErrorTool {
    definition: ToolDefinition,
}

impl ReportArchitecturalErrorTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "report_architectural_error",
                "Escalates a complex compile error to an architect when a local edit is not enough.",
            )
            .string_param("msg", "Problem description for the architect.", true),
        }
    }
}

impl LlmTool for ReportArchitecturalErrorTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        required_str(arguments, "msg")
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

pub fn parse_edit_file_arguments(arguments: &Value) -> Result<(PathBuf, usize, String), String> {
    let path = PathBuf::from(required_str(arguments, "path")?);
    let line = required_usize(arguments, "line")?;
    let content = required_str(arguments, "content")?;
    Ok((path, line, content))
}

pub fn parse_report_error_arguments(arguments: &Value) -> Result<String, String> {
    required_str(arguments, "msg")
}

fn required_usize(arguments: &Value, key: &str) -> Result<usize, String> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("tool argument '{key}' must be a positive integer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn triage_tool_set_registers_both_tools() {
        let tools = triage_tool_set();
        let names: Vec<_> = tools
            .definitions()
            .into_iter()
            .map(|tool| tool.name.clone())
            .collect();

        assert_eq!(
            names,
            vec![
                "edit_file".to_string(),
                "report_architectural_error".to_string(),
            ]
        );
    }

    #[test]
    fn parse_edit_file_arguments_extracts_fields() {
        let (path, line, content) = parse_edit_file_arguments(&json!({
            "path": "src/main.rs",
            "line": 42,
            "content": "pub struct NewName;"
        }))
        .expect("args");

        assert_eq!(path, PathBuf::from("src/main.rs"));
        assert_eq!(line, 42);
        assert_eq!(content, "pub struct NewName;");
    }
}
