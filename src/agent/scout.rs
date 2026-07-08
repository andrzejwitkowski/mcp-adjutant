mod tools;

use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall, LlmToolSet};

pub use tools::scout_tool_set;

pub const SCOUT_SYSTEM_PROMPT: &str = r#"You are an autonomous code scout (PHASE_1_SCOUT). Your goal is to gather and condense code context.

Available tools (tool calls):
- detect_language — detect file or project language (extension, manifests, content heuristics)
- ripgrep — broad text search across the repository
- ast_calls — precise AST lookup: call sites for a method in a file (Rust, TS, Python, Java, Kotlin, SQL, C, C++)
- read_file — file slice by line numbers
- finalize — end scouting with a condensed markdown report

Selection rule: If you do not know the language or repo layout, use detect_language. If you do not know where code lives, use ripgrep. When you know the files, use ast_calls. When you have the essence, call finalize.

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

    fn build_user_message(context: &AgentContext) -> String {
        if context.accumulated_data.is_empty() {
            context.input_prompt.clone()
        } else {
            format!(
                "{}\n\n---\nObservation history:\n{}",
                context.input_prompt, context.accumulated_data
            )
        }
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
        let user_message = Self::build_user_message(context);
        let request = LlmRequest::new(SCOUT_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn = self.client.complete(request)?;

        let tool_call = match model_turn.tool_calls.first() {
            Some(call) => call,
            None => {
                let thought = model_turn.content.unwrap_or_default();
                if thought.is_empty() {
                    return Err("model response missing tool call".to_string());
                }
                let step = format!("Thought:\n{thought}\nObservation:\n(model did not call a tool — continue)\n");
                context.accumulated_data.push_str(&step);
                return Ok(());
            }
        };

        let invocation = self.tools.invoke(&tool_call.name, &tool_call.arguments)?;

        let thought = model_turn.content.unwrap_or_default();
        let step = format!(
            "Thought:\n{thought}\nTool: {}({})\nObservation:\n{}\n",
            tool_call.name, tool_call.arguments, invocation.output
        );
        context.accumulated_data.push_str(&step);

        if invocation.is_terminal {
            context.accumulated_data = invocation.output;
            context.is_finished = true;
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
