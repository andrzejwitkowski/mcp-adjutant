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
            if self.definition.name == "report_error" {
                return required_str(arguments, "reason");
            }
            let summary = arguments
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("session finalized");
            return Ok(summary.to_string());
        }
        Err(format!(
            "{} is executed by TranspilerAgent",
            self.definition.name
        ))
    }

    fn is_terminal(&self) -> bool {
        self.terminal
    }
}

pub fn transpiler_tool_set() -> LlmToolSet {
    LlmToolSet::new()
        .register(HarnessTool::new(
            ToolDefinition::new(
                "write_target_file",
                "Writes or overwrites a target-language file with transpiled types/DTOs.",
            )
            .string_param("path", "Target file path (relative to repo root).", true)
            .string_param("content", "Full file contents to write.", true),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "invoke_child_triage",
                "Runs a nested TriageAgent loop on target files with compile/type error context.",
            )
            .string_array_param("target_paths", "Files to fix in the target stack.", true)
            .string_param(
                "error_context",
                "Compiler, type-checker, or linter errors to repair.",
                true,
            ),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "finalize_sync",
                "Ends the transpiler session after verification passes (terminal).",
            )
            .string_param("summary", "Optional one-line sync summary.", false),
            true,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "report_error",
                "Ends the transpiler session with a failure reason (terminal).",
            )
            .string_param("reason", "Why sync could not complete.", true),
            true,
        ))
}

pub fn required_string_array(arguments: &Value, key: &str) -> Result<Vec<String>, String> {
    let array = arguments
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key} array is required"))?;
    parse_string_array_items(key, array)
}

pub fn optional_string_array(arguments: &Value, key: &str) -> Result<Vec<String>, String> {
    match arguments.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => parse_string_array_items(key, items),
        Some(_) => Err(format!("{key} must be an array")),
    }
}

fn parse_string_array_items(key: &str, items: &[Value]) -> Result<Vec<String>, String> {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let s = item
                .as_str()
                .ok_or_else(|| format!("{key}[{i}] must be a string"))?;
            if s.is_empty() {
                return Err(format!("{key}[{i}] must not be empty"));
            }
            Ok(s.to_string())
        })
        .collect()
}

pub fn parse_write_arguments(arguments: &Value) -> Result<(String, String), String> {
    let path = required_str(arguments, "path")?;
    let content = required_str(arguments, "content")?;
    Ok((path, content))
}

pub fn parse_triage_arguments(arguments: &Value) -> Result<(Vec<String>, String), String> {
    let paths = required_string_array(arguments, "target_paths")?;
    if paths.is_empty() {
        return Err("target_paths must not be empty".to_string());
    }
    let error_context = required_str(arguments, "error_context")?;
    Ok((paths, error_context))
}

pub fn parse_report_reason(arguments: &Value) -> Result<String, String> {
    required_str(arguments, "reason")
}
