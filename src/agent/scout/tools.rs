use serde_json::Value;

use crate::agent::read_only_tools::read_only_tool_set;
use crate::llm::{required_str, LlmTool, LlmToolSet, ToolDefinition};

pub fn scout_tool_set() -> LlmToolSet {
    read_only_tool_set().register(FinalizeTool::new())
}

struct FinalizeTool {
    definition: ToolDefinition,
}

impl FinalizeTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "finalize",
                "Ends scouting and returns a condensed markdown report.",
            )
            .string_param("report", "Final condensed markdown report.", true),
        }
    }
}

impl LlmTool for FinalizeTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        required_str(arguments, "report")
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scout_tool_set_registers_all_tools() {
        let tools = scout_tool_set();
        let names: Vec<_> = tools
            .definitions()
            .into_iter()
            .map(|tool| tool.name.clone())
            .collect();

        assert_eq!(
            names,
            vec![
                "detect_language".to_string(),
                "ripgrep".to_string(),
                "ast_calls".to_string(),
                "read_file".to_string(),
                "finalize".to_string(),
            ]
        );
    }
}
