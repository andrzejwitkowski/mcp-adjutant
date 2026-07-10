mod cache_flow;
mod tools;

use async_trait::async_trait;
use serde_json::Value;

use super::orchestrator::run_single_tool_turn;
use super::traits::{AgentContext, AutonomousAgent};
use crate::cache::resolve_workspace_path;
use crate::llm::{LlmClient, LlmModelTurn, LlmToolCall, LlmToolSet};

pub use cache_flow::{run_scout_with_cache, ScoutCacheOutcome};
pub use tools::scout_tool_set;

pub const SCOUT_SYSTEM_PROMPT: &str = r#"You are an autonomous code scout (PHASE_1_SCOUT). Your goal is to gather and condense code context.

Available tools (tool calls):
- detect_language — detect file or project language (extension, manifests, content heuristics)
- ripgrep — broad text search across the repository
- ast_calls — precise AST lookup: call sites for a method in a file (Rust, TS, Python, Java, Kotlin, SQL, C, C++)
- read_file — file slice by line numbers
- finalize — end scouting with a condensed markdown report

Selection rule: If you do not know the language or repo layout, use detect_language. If you do not know where code lives, use ripgrep. When you know the files, use ast_calls. When you have the essence, call finalize.

Efficiency: Finalize within 6 tool turns once you can answer. Do not repeat the same tool with identical arguments.

Reply with a short rationale (Thought), then call exactly one tool."#;

pub type ScoutToolCall = LlmToolCall;
pub type ScoutModelTurn = LlmModelTurn;

pub struct ScoutAgent<C: LlmClient> {
    client: C,
    tools: LlmToolSet,
}

impl<C: LlmClient> ScoutAgent<C> {
    pub fn new(client: C) -> Self {
        Self {
            client,
            tools: scout_tool_set(),
        }
    }

    pub fn with_tools(client: C, tools: LlmToolSet) -> Self {
        Self { client, tools }
    }

    pub fn tools(&self) -> &LlmToolSet {
        &self.tools
    }

    fn record_touched_file(context: &mut AgentContext, tool_name: &str, args: &Value) {
        let Some(path) = (match tool_name {
            "read_file" | "ast_calls" => args.get("file").and_then(Value::as_str),
            "detect_language" if args.get("scope").and_then(Value::as_str) == Some("file") => {
                args.get("path").and_then(Value::as_str)
            }
            _ => None,
        }) else {
            return;
        };
        context.touched_files.push(resolve_workspace_path(path));
    }
}

#[async_trait]
impl<C: LlmClient> AutonomousAgent for ScoutAgent<C> {
    fn name(&self) -> &'static str {
        "scout_agent"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("PHASE_1_SCOUT") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(SCOUT_SYSTEM_PROMPT);
        }
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let called = run_single_tool_turn(&self.client, &self.tools, SCOUT_SYSTEM_PROMPT, context)?;
        if let Some((tool_name, args)) = called {
            Self::record_touched_file(context, &tool_name, &args);
        }
        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context
            .input_prompt
            .push_str("\nContinue scouting based on the latest observation.");
        Ok(())
    }
}
