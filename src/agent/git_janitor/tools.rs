use serde_json::{json, Value};

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
            return Ok(arguments.to_string());
        }
        Err(format!(
            "{} is executed by GitJanitorAgent",
            self.definition.name
        ))
    }

    fn is_terminal(&self) -> bool {
        self.terminal
    }
}

pub fn git_janitor_tool_set() -> LlmToolSet {
    LlmToolSet::new()
        .register(HarnessTool::new(
            ToolDefinition::new(
                "emit_git_copy",
                "Terminal: emit final JSON with commit_message, pr_title, pr_body, changelog_entry, branch fields.",
            )
            .string_param("commit_message", "Commit message per conventions.", true)
            .string_param("pr_title", "Short PR title.", true)
            .string_param("pr_body", "PR body matching template when present.", true)
            .string_param(
                "changelog_entry",
                "Two-sentence end-user changelog summary.",
                true,
            ),
            true,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "propose_conventions_patch",
                "Propose a JSON patch for git conventions (merged into suggested .adjutant.toml; does not write disk).",
            )
            .string_param(
                "patch_json",
                "JSON object patch for git_rules / commit_format / pr.",
                true,
            ),
            false,
        ))
        .register(HarnessTool::new(
            ToolDefinition::new(
                "update_git_conventions",
                "Write merged conventions to .adjutant.toml when persist is allowed.",
            )
            .string_param(
                "patch_json",
                "JSON object patch for git_rules / commit_format / pr.",
                true,
            ),
            false,
        ))
}

pub fn parse_patch_json(arguments: &Value) -> Result<Value, String> {
    let raw = required_str(arguments, "patch_json")?;
    serde_json::from_str(&raw).map_err(|err| format!("patch_json must be JSON object: {err}"))
}

pub fn parse_emit_fields(arguments: &Value) -> Result<EmitFields, String> {
    Ok(EmitFields {
        commit_message: required_str(arguments, "commit_message")?,
        pr_title: required_str(arguments, "pr_title")?,
        pr_body: required_str(arguments, "pr_body")?,
        changelog_entry: required_str(arguments, "changelog_entry")?,
    })
}

#[derive(Debug, Clone)]
pub struct EmitFields {
    pub commit_message: String,
    pub pr_title: String,
    pub pr_body: String,
    pub changelog_entry: String,
}

pub fn build_emit_json(
    fields: &EmitFields,
    gate: &super::branch::BranchGate,
    conventions: &super::conventions::GitConventions,
    suggested_toml: &str,
    persist_wrote: Option<&str>,
) -> Value {
    json!({
        "commit_message": fields.commit_message,
        "pr_title": fields.pr_title,
        "pr_body": fields.pr_body,
        "changelog_entry": fields.changelog_entry,
        "ticket_id": gate.ticket_id,
        "branch_status": gate.branch_status,
        "action_required": gate.action_required,
        "commit_allowed": gate.commit_allowed,
        "suggested_branch_name": gate.suggested_branch_name,
        "current_branch": gate.current_branch,
        "conventions": conventions,
        "suggested_adjutant_toml": suggested_toml,
        "persisted_adjutant_toml": persist_wrote,
    })
}
