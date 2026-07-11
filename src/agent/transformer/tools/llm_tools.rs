use serde_json::Value;

use crate::llm::{LlmTool, LlmToolSet, ToolDefinition};

pub fn transformer_tool_set() -> LlmToolSet {
    LlmToolSet::new()
        .register(GatherRefactorTargetsTool::new())
        .register(ApplyStructuralCodemodTool::new())
}

struct GatherRefactorTargetsTool {
    definition: ToolDefinition,
}

impl GatherRefactorTargetsTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "gather_refactor_targets",
                "Runs a Scout sub-agent (ripgrep, ast_calls, ast_constructions) to collect call sites for a method or struct before refactoring.",
            )
            .string_param(
                "method_name",
                "Method or struct name whose call sites to locate.",
                true,
            ),
        }
    }
}

impl LlmTool for GatherRefactorTargetsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, _arguments: &Value) -> Result<String, String> {
        Err("gather_refactor_targets is executed by TransformerAgent".to_string())
    }
}

struct ApplyStructuralCodemodTool {
    definition: ToolDefinition,
}

impl ApplyStructuralCodemodTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "apply_structural_codemod",
                "Applies a structural code change to specific files and lines from Scout (e.g. add a parameter to method calls).",
            )
            .string_param(
                "transformation_rule",
                "Transformation instruction, e.g. 'Add context as the first argument to this method call'.",
                true,
            )
            .string_param(
                "refactor_targets_json",
                "JSON array: [{\"file_path\":\"src/foo.rs\",\"lines\":[3]}] or {\"ranges\":[{\"start\":25,\"end\":32}]}",
                true,
            ),
        }
    }
}

impl LlmTool for ApplyStructuralCodemodTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, _arguments: &Value) -> Result<String, String> {
        Err("apply_structural_codemod is executed by TransformerAgent".to_string())
    }
}
