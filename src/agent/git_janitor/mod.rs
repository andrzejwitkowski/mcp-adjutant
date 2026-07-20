pub mod branch;
pub mod conventions;
pub mod scout;
pub mod tools;

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use super::traits::{AgentContext, AutonomousAgent};
use super::{build_tool_loop_message, AgentLoopOrchestrator};
use crate::cache::mcp_workspace_root;
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolSet};

pub use branch::{
    create_git_branch, evaluate_branch_gate, suggest_branch_name, BranchAction, BranchGate,
    BranchStatus,
};
pub use conventions::{
    conventions_toml_string, extract_ticket, find_jira_ticket, merge_conventions_patch,
    write_adjutant_toml, GitConventions, GitRules, ADJUTANT_TOML,
};
pub use scout::{format_scout_block, gather_conventions_and_diff, GitJanitorScout, ScoutInputs};
pub use tools::{build_emit_json, git_janitor_tool_set, parse_emit_fields, parse_patch_json};

pub const GIT_JANITOR_MAX_ITERATIONS: u32 = 4;

pub const GIT_JANITOR_SYSTEM_PROMPT: &str = r#"You are GitJanitorAgent. Produce commit/PR/changelog copy that matches scouting conventions.

Rules:
1. Call exactly one tool per turn.
2. Prefer emit_git_copy when you have enough context.
3. Use propose_conventions_patch to refine rules without writing disk.
4. Use update_git_conventions ONLY when the user prompt says persist is allowed.
5. Respect branch gate: if commit_allowed is false, still emit draft copy but do not claim commit is safe.
6. Follow commit_format.pattern and commit_style. Default to Conventional Commits when unsure.
7. Fill PR body using any template sections present.
8. changelog_entry must be two short sentences for end users.

Available tools: emit_git_copy, propose_conventions_patch, update_git_conventions."#;

pub struct GitJanitorAgent<C: LlmClient> {
    llm_client: C,
    tools: LlmToolSet,
    conventions: Mutex<GitConventions>,
    branch_gate: BranchGate,
    suggested_toml: Mutex<String>,
    persist_allowed: bool,
    workspace_root: PathBuf,
    last_persist_path: Mutex<Option<String>>,
}

impl<C: LlmClient> GitJanitorAgent<C> {
    pub fn new(
        llm_client: C,
        scout: GitJanitorScout,
        persist_allowed: bool,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            llm_client,
            tools: git_janitor_tool_set(),
            conventions: Mutex::new(scout.conventions),
            branch_gate: scout.branch_gate,
            suggested_toml: Mutex::new(scout.suggested_adjutant_toml),
            persist_allowed,
            workspace_root,
            last_persist_path: Mutex::new(None),
        }
    }

    fn apply_patch(&self, patch: &Value) -> Result<String, String> {
        let mut guard = self
            .conventions
            .lock()
            .map_err(|_| "conventions lock poisoned".to_string())?;
        let merged = merge_conventions_patch(&guard, patch)?;
        let toml = conventions_toml_string(&merged)?;
        *guard = merged;
        if let Ok(mut sug) = self.suggested_toml.lock() {
            *sug = toml.clone();
        }
        Ok(format!(
            "conventions updated (in-memory)\n```toml\n{toml}\n```"
        ))
    }

    fn persist_patch(&self, patch: &Value) -> Result<String, String> {
        if !self.persist_allowed {
            return Err(
                "update_git_conventions refused: persist_conventions is false / mode does not allow write"
                    .into(),
            );
        }
        let msg = self.apply_patch(patch)?;
        let guard = self
            .conventions
            .lock()
            .map_err(|_| "conventions lock poisoned".to_string())?;
        let path = write_adjutant_toml(&self.workspace_root, &guard)?;
        let path_str = path.display().to_string();
        if let Ok(mut last) = self.last_persist_path.lock() {
            *last = Some(path_str.clone());
        }
        Ok(format!("{msg}\nwrote {path_str}"))
    }

    fn emit(&self, arguments: &Value, context: &mut AgentContext) -> Result<String, String> {
        let fields = parse_emit_fields(arguments)?;
        let conventions = self
            .conventions
            .lock()
            .map_err(|_| "conventions lock poisoned".to_string())?
            .clone();
        let suggested = self
            .suggested_toml
            .lock()
            .map_err(|_| "suggested_toml lock poisoned".to_string())?
            .clone();
        let persisted = self
            .last_persist_path
            .lock()
            .map_err(|_| "persist lock poisoned".to_string())?
            .clone();
        let json = build_emit_json(
            &fields,
            &self.branch_gate,
            &conventions,
            &suggested,
            persisted.as_deref(),
        );
        let text = serde_json::to_string_pretty(&json)
            .map_err(|err| format!("serialize emit_git_copy: {err}"))?;
        context.is_finished = true;
        context.agent_completed = true;
        context.accumulated_data = text.clone();
        Ok(text)
    }

    async fn dispatch_tool(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &mut AgentContext,
    ) -> Result<String, String> {
        match tool_name {
            "emit_git_copy" => self.emit(arguments, context),
            "propose_conventions_patch" => {
                let patch = parse_patch_json(arguments)?;
                self.apply_patch(&patch)
            }
            "update_git_conventions" => {
                let patch = parse_patch_json(arguments)?;
                self.persist_patch(&patch)
            }
            other => Err(format!("unsupported git janitor tool: {other}")),
        }
    }
}

#[async_trait]
impl<C: LlmClient> AutonomousAgent for GitJanitorAgent<C> {
    fn name(&self) -> &'static str {
        "GitJanitorAgent"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("GitJanitorAgent") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(GIT_JANITOR_SYSTEM_PROMPT);
        }
        context.input_prompt.push_str(&format!(
            "\n\npersist_allowed={} commit_allowed={} workspace={}\n",
            self.persist_allowed,
            self.branch_gate.commit_allowed,
            self.workspace_root.display()
        ));
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let user_message = build_tool_loop_message(context);
        let request = LlmRequest::new(GIT_JANITOR_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn: LlmModelTurn = self.llm_client.complete(request)?;

        let tool_call = match model_turn.tool_calls.first() {
            Some(call) => call,
            None => {
                let thought = model_turn.content.unwrap_or_default();
                context.accumulated_data.push_str(&format!(
                    "Thought:\n{thought}\nObservation:\n(model did not call a tool — call exactly one tool)\n"
                ));
                return Ok(());
            }
        };

        let args_key = tool_call.arguments.to_string();
        let call_key = (tool_call.name.clone(), args_key);
        if context.last_tool_call.as_ref() == Some(&call_key) {
            context.accumulated_data.push_str(
                "Observation:\nduplicate tool call blocked — change approach or emit_git_copy.\n",
            );
            return Ok(());
        }
        context.last_tool_call = Some(call_key);

        let thought = model_turn.content.unwrap_or_default();
        let observation = self
            .dispatch_tool(&tool_call.name, &tool_call.arguments, context)
            .await?;

        if tool_call.name == "emit_git_copy" {
            return Ok(());
        }

        context.accumulated_data.push_str(&format!(
            "Thought:\n{thought}\nTool: {}({})\nObservation:\n{observation}\n",
            tool_call.name, tool_call.arguments
        ));
        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        if context.iterations >= context.max_iterations.saturating_sub(1) {
            context
                .input_prompt
                .push_str("\nFinal turn: call emit_git_copy now.");
        } else {
            context
                .input_prompt
                .push_str("\nContinue. Call exactly one tool.");
        }
        Ok(())
    }
}

pub async fn run_git_janitor<C: LlmClient>(
    agent: &GitJanitorAgent<C>,
    prompt: String,
) -> Result<AgentContext, String> {
    AgentLoopOrchestrator::run(agent, prompt, GIT_JANITOR_MAX_ITERATIONS).await
}

pub fn default_workspace_root() -> PathBuf {
    mcp_workspace_root()
}
