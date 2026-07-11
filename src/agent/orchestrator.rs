use super::traits::{AgentContext, AutonomousAgent};
use crate::cache::mcp_workspace_root;
use crate::llm::{LlmClient, LlmRequest};

pub struct AgentLoopOrchestrator;

impl AgentLoopOrchestrator {
    pub async fn run(
        agent: &impl AutonomousAgent,
        initial_prompt: String,
        max_iters: u32,
    ) -> Result<AgentContext, String> {
        let mut context = AgentContext {
            input_prompt: initial_prompt,
            accumulated_data: String::new(),
            iterations: 0,
            max_iterations: max_iters,
            is_finished: false,
            agent_completed: false,
            touched_files: Vec::new(),
            last_tool_call: None,
        };

        agent.enrich_context(&mut context).await?;

        while !context.is_finished && context.iterations < context.max_iterations {
            context.iterations += 1;

            agent.process_and_evaluate(&mut context).await?;

            if context.is_finished {
                break;
            }

            agent.mutate_next_iteration(&mut context).await?;
        }

        // ponytail: hard stop — treat accumulated observations as the final report when capped
        if !context.is_finished {
            let agent_name = agent.name();
            let workspace = mcp_workspace_root().display().to_string();
            let touched = if context.touched_files.is_empty() {
                "(none)".to_string()
            } else {
                context
                    .touched_files
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n- ")
            };
            if context.accumulated_data.is_empty() {
                context.accumulated_data = format!(
                    "## {agent_name} report (iteration limit after {} of {} turns)\n\n{agent_name} did not finalize; partial evidence only.\nWorkspace: {workspace}\nTouched files:\n- {touched}\n",
                    context.iterations, context.max_iterations
                );
            } else {
                context.accumulated_data = format!(
                    "## {agent_name} report (iteration limit after {} of {} turns)\n\n{agent_name} did not finalize; partial evidence only.\nWorkspace: {workspace}\nTouched files:\n- {touched}\n\n{}",
                    context.iterations, context.max_iterations, context.accumulated_data
                );
            }
            context.is_finished = true;
        }

        Ok(context)
    }
}

pub fn build_tool_loop_message(context: &AgentContext) -> String {
    if context.accumulated_data.is_empty() {
        context.input_prompt.clone()
    } else {
        format!(
            "{}\n\n---\nObservation history:\n{}",
            context.input_prompt, context.accumulated_data
        )
    }
}

pub fn run_single_tool_turn<C: LlmClient>(
    client: &C,
    tools: &crate::llm::LlmToolSet,
    system_prompt: &str,
    context: &mut AgentContext,
) -> Result<Option<(String, serde_json::Value)>, String> {
    let user_message = build_tool_loop_message(context);
    let request = LlmRequest::new(system_prompt, &user_message, tools);
    let model_turn = client.complete(request)?;

    let tool_call = match model_turn.tool_calls.first() {
        Some(call) => call,
        None => {
            let thought = model_turn.content.unwrap_or_default();
            let step = if thought.is_empty() {
                "Observation:\n(model returned no tool call — call exactly one tool)\n".to_string()
            } else {
                format!(
                    "Thought:\n{thought}\nObservation:\n(model did not call a tool — call exactly one tool)\n"
                )
            };
            context.accumulated_data.push_str(&step);
            return Ok(None);
        }
    };

    let args_key = tool_call.arguments.to_string();
    let call_key = (tool_call.name.clone(), args_key);
    if context.last_tool_call.as_ref() == Some(&call_key) {
        let thought = model_turn.content.unwrap_or_default();
        let step = format!(
            "Thought:\n{thought}\nTool: {}({})\nObservation:\nduplicate tool call blocked — change pattern or call finalize.\n",
            tool_call.name, tool_call.arguments
        );
        context.accumulated_data.push_str(&step);
        return Ok(Some((tool_call.name.clone(), tool_call.arguments.clone())));
    }
    context.last_tool_call = Some(call_key);

    let invocation = tools.invoke(&tool_call.name, &tool_call.arguments)?;
    let called = Some((tool_call.name.clone(), tool_call.arguments.clone()));

    let thought = model_turn.content.unwrap_or_default();
    let step = format!(
        "Thought:\n{thought}\nTool: {}({})\nObservation:\n{}\n",
        tool_call.name, tool_call.arguments, invocation.output
    );
    context.accumulated_data.push_str(&step);

    if invocation.is_terminal {
        context.accumulated_data = invocation.output;
        context.is_finished = true;
        context.agent_completed = true;
    }

    Ok(called)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::traits::AgentContext;
    use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmTool, LlmToolSet, ToolDefinition};

    struct NoToolClient;

    impl LlmClient for NoToolClient {
        fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
            Ok(LlmModelTurn {
                content: None,
                tool_calls: vec![],
                usage: None,
            })
        }
    }

    struct DoneTool;

    impl LlmTool for DoneTool {
        fn definition(&self) -> &ToolDefinition {
            static DEF: std::sync::OnceLock<ToolDefinition> = std::sync::OnceLock::new();
            DEF.get_or_init(|| ToolDefinition::new("done", "done"))
        }

        fn invoke(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("finished".to_string())
        }

        fn is_terminal(&self) -> bool {
            true
        }
    }

    struct RepeatRipgrepClient {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl LlmClient for RepeatRipgrepClient {
        fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
            use crate::llm::LlmToolCall;
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(LlmModelTurn {
                content: Some("search again".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "ripgrep".to_string(),
                    arguments: serde_json::json!({"pattern": "token metrics"}),
                }],
                usage: None,
            })
        }
    }

    struct RipgrepTool;

    impl LlmTool for RipgrepTool {
        fn definition(&self) -> &ToolDefinition {
            static DEF: std::sync::OnceLock<ToolDefinition> = std::sync::OnceLock::new();
            DEF.get_or_init(|| ToolDefinition::new("ripgrep", "ripgrep"))
        }

        fn invoke(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("(no matches)".to_string())
        }
    }

    #[test]
    fn duplicate_tool_call_is_blocked_on_second_identical_invoke() {
        let tools = LlmToolSet::new().register(RipgrepTool);
        let client = RepeatRipgrepClient {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut context = AgentContext {
            input_prompt: "find metrics".to_string(),
            accumulated_data: String::new(),
            iterations: 1,
            max_iterations: 3,
            is_finished: false,
            agent_completed: false,
            touched_files: Vec::new(),
            last_tool_call: None,
        };

        run_single_tool_turn(&client, &tools, "system", &mut context).expect("first call");
        assert!(context.accumulated_data.contains("(no matches)"));

        run_single_tool_turn(&client, &tools, "system", &mut context).expect("second call");
        assert!(
            context
                .accumulated_data
                .contains("duplicate tool call blocked"),
            "expected duplicate guard message: {}",
            context.accumulated_data
        );
    }

    #[test]
    fn empty_tool_response_nudges_instead_of_error() {
        let tools = LlmToolSet::new().register(DoneTool);
        let mut context = AgentContext {
            input_prompt: "research topic".to_string(),
            accumulated_data: String::new(),
            iterations: 1,
            max_iterations: 3,
            is_finished: false,
            agent_completed: false,
            touched_files: Vec::new(),
            last_tool_call: None,
        };

        let result = run_single_tool_turn(&NoToolClient, &tools, "system", &mut context)
            .expect("should continue after empty tool response");

        assert!(result.is_none());
        assert!(context.accumulated_data.contains("call exactly one tool"));
    }
}
