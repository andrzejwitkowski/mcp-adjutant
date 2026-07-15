mod emit;
mod validate;

#[cfg(test)]
mod tests;

use crate::agent::planner::constraints::CoordinatorConstraints;
use crate::agent::read_only_tools::ReadFileTool;
use crate::llm::LlmToolSet;

pub use validate::{
    extract_json_object, validate_blueprint, validate_blueprint_coordinator,
    validate_blueprint_grounding,
};

pub use crate::agent::read_only_tools::planner_scout_tool_set;

pub fn planner_emit_tool_set(coordinator: CoordinatorConstraints) -> LlmToolSet {
    LlmToolSet::new()
        .register(ReadFileTool::new())
        .register(emit::EmitBlueprintTool::new(coordinator))
}
