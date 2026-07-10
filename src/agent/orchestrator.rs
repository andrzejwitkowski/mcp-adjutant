use super::traits::{AgentContext, AutonomousAgent};
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

        // ponytail: hard stop — treat accumulated observations as the scout report when capped
        if !context.is_finished {
            if context.accumulated_data.is_empty() {
                context.accumulated_data = format!(
                    "Scout stopped after {} iteration(s) (max {}).",
                    context.iterations, context.max_iterations
                );
            } else {
                context.accumulated_data = format!(
                    "## Scout report (iteration limit after {} of {} turns)\n\n{}",
                    context.iterations, context.max_iterations, context.accumulated_data
                );
            }
            context.is_finished = true;
        }

        Ok(context)
    }
}

/// Build the user message for a tool-loop turn: the input prompt, plus the
/// accumulated observation history once there is any.
/// Shared by single-tool-loop agents (scout, web_fetcher).
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

/// Run one turn of a single-tool-loop agent: ask the model for exactly one tool
/// call, invoke it, append the observation, and finish if it is terminal.
///
/// Returns `Some((name, args))` for the tool that was invoked (so the caller can
/// observe side effects like scout's touched-file tracking), or `None` if the
/// model produced no tool call. Agents that take the *first* tool call and treat
/// it as the whole turn (scout, web_fetcher) share this body; multi-tool agents
/// (builder) and non-loop agents (evaluator) do not.
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
            if thought.is_empty() {
                return Err("model response missing tool call".to_string());
            }
            let step = format!(
                "Thought:\n{thought}\nObservation:\n(model did not call a tool — continue)\n"
            );
            context.accumulated_data.push_str(&step);
            return Ok(None);
        }
    };

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
