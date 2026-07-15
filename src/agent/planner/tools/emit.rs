use serde_json::Value;

use crate::agent::planner::constraints::CoordinatorConstraints;
use crate::llm::{required_str, LlmTool, ToolDefinition};

use super::validate::{validate_blueprint, validate_blueprint_coordinator};

pub(crate) struct EmitBlueprintTool {
    definition: ToolDefinition,
    coordinator: CoordinatorConstraints,
}

impl EmitBlueprintTool {
    pub(crate) fn new(coordinator: CoordinatorConstraints) -> Self {
        Self {
            definition: ToolDefinition::new(
                "emit_blueprint",
                "Finalizes planning. Accepts the Blueprint JSON string, validates its shape, and returns it. Terminal.",
            )
            .string_param(
                "blueprint",
                "The complete Blueprint JSON object, serialized as a string.",
                true,
            ),
            coordinator,
        }
    }
}

impl LlmTool for EmitBlueprintTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        let raw = required_str(arguments, "blueprint")?;
        let blueprint = validate_blueprint(&raw).map_err(|err| {
            format!("Blueprint rejected: {err}\nFix the JSON and call emit_blueprint again.")
        })?;
        validate_blueprint_coordinator(&blueprint, &self.coordinator).map_err(|err| {
            format!("Blueprint rejected: {err}\nFix the JSON and call emit_blueprint again.")
        })?;
        serde_json::to_string_pretty(&blueprint)
            .map_err(|err| format!("failed to re-serialize blueprint: {err}"))
    }

    fn is_terminal(&self) -> bool {
        true
    }
}
