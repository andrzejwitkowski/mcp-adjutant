mod gates;
mod tools;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use super::traits::{AgentContext, AutonomousAgent};
use super::{
    analyze_log_at_path, build_tool_loop_message, format_triage_success, triage_passed,
    AgentLoopOrchestrator, SystemBuildRunner, TriageAgent, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::resolve_workspace_path;
use crate::domain::AdjutantConfig;
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolSet};
use crate::tools::{
    assert_on_pr_head_branch, format_pr_state_markdown, gh_post_comment, gh_pr_state,
    git_push_origin_head, LlmBuildDiscoverer,
};
pub use gates::{check_finalize_allowed, BabysitterSession};
pub use tools::{
    babysitter_tool_set, parse_finalize_arguments, parse_log_path, parse_report_body,
    parse_triage_arguments,
};

pub const BABYSITTER_SYSTEM_PROMPT: &str = r#"You are the BabysitterAgent (PHASE_BABYSITTER), a high-level orchestrator inside mcp-adjutant. Drive the assigned GitHub PR to mergeable state (green CI, resolved actionable reviews).

Every turn: reply with a short Thought, then call exactly ONE tool.

Orchestration rules:
1. Start with github_get_pr_state.
2. CI failure -> run_log_analyzer on gh-run:<id> from state, then invoke_child_triage for straightforward compile/lint errors.
2b. CI green but review line comments exist -> invoke_child_triage on cited paths (CodeRabbit/bot inline comments are FIXABLE_ACTION by default).
3. Review comments: [FIXABLE_ACTION] -> invoke_child_triage; [ARCHITECTURAL_DISCUSSION] / [NITPICK_OR_IGNORE] -> skip (note in finalize report).
4. Never git_push_changes until the latest invoke_child_triage observation contains [TRIAGE PASS].
5. When done: github_post_final_report, then finalize_session with skipped_review_paths for any review paths not triaged ([NITPICK_OR_IGNORE] / [ARCHITECTURAL_DISCUSSION]).

Available tools: github_get_pr_state, run_log_analyzer, invoke_child_triage, git_push_changes, github_post_final_report, finalize_session."#;

pub const BABYSITTER_MAX_ITERATIONS: u32 = 20;
const CHILD_TRIAGE_MAX_ITERATIONS: u32 = 5;

pub struct BabysitterAgent<C, TC, SC> {
    llm_client: C,
    config: Arc<AdjutantConfig>,
    triage_agent: TriageAgent<TC, SystemBuildRunner, LlmBuildDiscoverer<SC>>,
    pr_number: u64,
    tools: LlmToolSet,
    triage_green: Mutex<bool>,
    session: Mutex<BabysitterSession>,
}

impl<C: LlmClient, TC: LlmClient, SC: LlmClient> BabysitterAgent<C, TC, SC> {
    pub fn new(
        llm_client: C,
        config: Arc<AdjutantConfig>,
        triage_agent: TriageAgent<TC, SystemBuildRunner, LlmBuildDiscoverer<SC>>,
        pr_number: u64,
    ) -> Self {
        Self {
            llm_client,
            config,
            triage_agent,
            pr_number,
            tools: babysitter_tool_set(),
            triage_green: Mutex::new(false),
            session: Mutex::new(BabysitterSession::default()),
        }
    }

    async fn run_child_triage(
        &self,
        target_paths: Vec<PathBuf>,
        error_context: &str,
    ) -> Result<String, String> {
        self.triage_agent.retarget(target_paths)?;
        let prompt = format!(
            "invoke_child_triage\n\n{TRIAGE_SYSTEM_PROMPT}\n\nError context:\n{error_context}"
        );
        let triage_ctx =
            AgentLoopOrchestrator::run(&self.triage_agent, prompt, CHILD_TRIAGE_MAX_ITERATIONS)
                .await?;

        let passed = triage_passed(&triage_ctx);
        if let Ok(mut guard) = self.triage_green.lock() {
            *guard = passed;
        }

        if passed {
            Ok(format_triage_success(&triage_ctx))
        } else {
            Ok(format!(
                "[TRIAGE INCOMPLETE] finished={} iterations={}\n{}",
                triage_ctx.is_finished, triage_ctx.iterations, triage_ctx.accumulated_data
            ))
        }
    }

    async fn dispatch_tool(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &mut AgentContext,
    ) -> Result<String, String> {
        match tool_name {
            "github_get_pr_state" => {
                let state = gh_pr_state(self.pr_number)?;
                assert_on_pr_head_branch(&state.head_ref_name)?;
                if let Ok(mut guard) = self.session.lock() {
                    guard.record_pr_state(&state);
                }
                Ok(format_pr_state_markdown(&state))
            }
            "run_log_analyzer" => {
                let log_path = tools::parse_log_path(arguments)?;
                analyze_log_at_path(&self.config, &log_path, true)
            }
            "invoke_child_triage" => {
                let (paths, error_context) = tools::parse_triage_arguments(arguments)?;
                if let Ok(mut guard) = self.session.lock() {
                    guard.record_triage_paths(&paths);
                }
                let resolved = paths
                    .into_iter()
                    .map(|path| resolve_workspace_path(&path))
                    .collect();
                self.run_child_triage(resolved, &error_context).await
            }
            "git_push_changes" => {
                let green = self
                    .triage_green
                    .lock()
                    .map(|guard| *guard)
                    .unwrap_or(false);
                if !green {
                    return Err(
                        "refusing git_push_changes: child triage has not passed locally yet"
                            .to_string(),
                    );
                }
                git_push_origin_head()
            }
            "github_post_final_report" => {
                let body = tools::parse_report_body(arguments)?;
                gh_post_comment(self.pr_number, &body)?;
                if let Ok(mut guard) = self.session.lock() {
                    guard.mark_report_posted();
                }
                Ok("report posted to PR".to_string())
            }
            "finalize_session" => {
                let (summary, skipped_review_paths) = tools::parse_finalize_arguments(arguments)?;
                let state = gh_pr_state(self.pr_number)?;
                let session = self
                    .session
                    .lock()
                    .map_err(|_| "session lock poisoned".to_string())?
                    .clone();
                check_finalize_allowed(&state, &session, &skipped_review_paths)?;
                let summary = summary.unwrap_or_else(|| "babysitter session complete".to_string());
                context.is_finished = true;
                context.agent_completed = true;
                Ok(summary)
            }
            other => Err(format!("unsupported babysitter tool: {other}")),
        }
    }
}

#[async_trait]
impl<C: LlmClient, TC: LlmClient, SC: LlmClient> AutonomousAgent for BabysitterAgent<C, TC, SC> {
    fn name(&self) -> &'static str {
        "PHASE_BABYSITTER"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("PHASE_BABYSITTER") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(BABYSITTER_SYSTEM_PROMPT);
        }
        context.input_prompt.push_str(&format!(
            "\n\nPR number: {}\nMax iterations: {}\n",
            self.pr_number, context.max_iterations
        ));
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let user_message = build_tool_loop_message(context);
        let request = LlmRequest::new(BABYSITTER_SYSTEM_PROMPT, &user_message, &self.tools);
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
                "Observation:\nduplicate tool call blocked — change approach or finalize.\n",
            );
            return Ok(());
        }
        context.last_tool_call = Some(call_key);

        let thought = model_turn.content.unwrap_or_default();
        let observation = self
            .dispatch_tool(&tool_call.name, &tool_call.arguments, context)
            .await?;

        if tool_call.name == "finalize_session" {
            context.accumulated_data = observation.clone();
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
                .push_str("\nFinal turn: post report if needed, then call finalize_session.");
        } else {
            context
                .input_prompt
                .push_str("\nContinue babysitting. Call exactly one harness tool.");
        }
        Ok(())
    }
}
