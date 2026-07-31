mod apply;
mod emit;
mod validate;

#[cfg(test)]
mod tests;

use crate::agent::planner::constraints::CoordinatorConstraints;
use crate::llm::LlmToolSet;

pub use apply::{apply_blueprint_file_steps, apply_hunks_to_body};
pub use validate::{
    extract_json_object, path_line_from_goal, source_under_test_from_goal, test_type_for_target,
    validate_blueprint, validate_blueprint_coordinator, validate_blueprint_grounding,
};

pub use crate::agent::read_only_tools::planner_scout_tool_set;

pub fn planner_emit_tool_set(coordinator: CoordinatorConstraints) -> LlmToolSet {
    emit::draft_emit_tools(coordinator)
}
