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
    let summary = first_line_commit_summary(&fields.commit_message);
    let suggested_branch_name =
        super::branch::suggest_branch_name(gate.ticket_id.as_deref(), Some(summary));
    let mut out = json!({
        "commit_message": fields.commit_message,
        "pr_title": fields.pr_title,
        "pr_body": fields.pr_body,
        "changelog_entry": fields.changelog_entry,
        "ticket_id": gate.ticket_id,
        "branch_status": gate.branch_status,
        "action_required": gate.action_required,
        "commit_allowed": gate.commit_allowed,
        "suggested_branch_name": suggested_branch_name,
        "current_branch": gate.current_branch,
    });
    if let Some(path) = persist_wrote {
        out["conventions"] = serde_json::to_value(conventions).unwrap_or(Value::Null);
        out["suggested_adjutant_toml"] = Value::String(suggested_toml.to_string());
        out["persisted_adjutant_toml"] = Value::String(path.to_string());
    }
    out
}

fn first_line_commit_summary(commit_message: &str) -> &str {
    let line = commit_message
        .lines()
        .next()
        .unwrap_or(commit_message)
        .trim();
    if let Some(rest) = line.strip_prefix('[') {
        if let Some((_, after)) = rest.split_once(']') {
            let after = after.trim().trim_start_matches(':').trim();
            if !after.is_empty() {
                return after;
            }
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::git_janitor::branch::{BranchAction, BranchGate, BranchStatus};
    use crate::agent::git_janitor::conventions::GitConventions;

    fn gate() -> BranchGate {
        BranchGate {
            current_branch: "feat/x".into(),
            branch_status: BranchStatus::Ok,
            action_required: BranchAction::None,
            commit_allowed: true,
            suggested_branch_name: "feat/stale".into(),
            ticket_id: Some("GIT-0".into()),
        }
    }

    #[test]
    fn emit_omits_conventions_without_persist() {
        let fields = EmitFields {
            commit_message: "[GIT-0] fix: rustfmt import order".into(),
            pr_title: "[GIT-0] fix: rustfmt import order".into(),
            pr_body: "body".into(),
            changelog_entry: "a. b.".into(),
        };
        let v = build_emit_json(&fields, &gate(), &GitConventions::default(), "toml", None);
        assert!(v.get("conventions").is_none());
        assert!(v.get("suggested_adjutant_toml").is_none());
        assert!(v.get("persisted_adjutant_toml").is_none());
        assert_eq!(
            v["suggested_branch_name"].as_str().unwrap(),
            "feat/GIT-0-fix-rustfmt-import-order"
        );
    }

    #[test]
    fn emit_includes_conventions_when_persisted() {
        let fields = EmitFields {
            commit_message: "feat: x".into(),
            pr_title: "feat: x".into(),
            pr_body: "b".into(),
            changelog_entry: "c. d.".into(),
        };
        let v = build_emit_json(
            &fields,
            &gate(),
            &GitConventions::default(),
            "toml-body",
            Some("/tmp/.adjutant.toml"),
        );
        assert!(v.get("conventions").is_some());
        assert_eq!(v["suggested_adjutant_toml"], "toml-body");
        assert_eq!(v["persisted_adjutant_toml"], "/tmp/.adjutant.toml");
    }

    #[test]
    fn first_line_commit_summary_strips_ticket_bracket() {
        assert_eq!(
            first_line_commit_summary("[GIT-42] fix: rustfmt import order\n\nBody"),
            "fix: rustfmt import order"
        );
        assert_eq!(first_line_commit_summary("  chore: tidy  "), "chore: tidy");
        assert_eq!(first_line_commit_summary("[TICKET]   \nbody"), "[TICKET]");
    }
}
