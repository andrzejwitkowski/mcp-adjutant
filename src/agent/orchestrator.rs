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
        Self::run_loop(agent, &mut context).await?;
        Self::apply_iteration_cap(agent, &mut context);
        Ok(context)
    }

    /// Continue an in-flight loop (e.g. planner JSON fix rounds) without resetting context.
    pub async fn resume(
        agent: &impl AutonomousAgent,
        mut context: AgentContext,
        extra_iters: u32,
    ) -> Result<AgentContext, String> {
        context.is_finished = false;
        context.max_iterations = context.iterations.saturating_add(extra_iters);
        Self::run_loop(agent, &mut context).await?;
        Self::apply_iteration_cap(agent, &mut context);
        Ok(context)
    }

    async fn run_loop(
        agent: &impl AutonomousAgent,
        context: &mut AgentContext,
    ) -> Result<(), String> {
        let started = std::time::Instant::now();
        let wall = std::time::Duration::from_secs(crate::jobs::JOB_WALL_CLOCK_SECS);
        while !context.is_finished && context.iterations < context.max_iterations {
            if crate::jobs::job_cancel_requested() {
                return Err("job cancelled".to_string());
            }
            // Prefer job-registry age so nested scout/triage loops share one budget.
            if crate::jobs::job_wall_clock_exceeded() || started.elapsed() > wall {
                return Err(format!(
                    "job wall-clock limit exceeded ({}s)",
                    crate::jobs::JOB_WALL_CLOCK_SECS
                ));
            }
            context.iterations += 1;
            crate::jobs::publish_job_action(format!(
                "{} turn {}",
                agent.name(),
                context.iterations
            ));

            agent.process_and_evaluate(context).await?;

            if context.is_finished {
                break;
            }

            agent.mutate_next_iteration(context).await?;
        }
        Ok(())
    }

    fn apply_iteration_cap(agent: &impl AutonomousAgent, context: &mut AgentContext) {
        // ponytail: hard stop — treat accumulated observations as the final report when capped
        if context.is_finished {
            return;
        }

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
        let observations = strip_prior_iteration_cap(&context.accumulated_data);
        let observations = last_evidence_chunk(observations);
        let header = format!(
            "## {agent_name} report (iteration limit after {} of {} turns)\n\n{agent_name} did not finalize; partial evidence only.\nWorkspace: {workspace}\nTouched files:\n- {touched}",
            context.iterations, context.max_iterations
        );
        context.accumulated_data = if observations.is_empty() {
            format!("{header}\n")
        } else {
            format!("{header}\n\n{observations}")
        };
        context.is_finished = true;
    }
}

/// Drop a prior iteration-cap header so `resume()` does not nest cap blocks.
fn strip_prior_iteration_cap(data: &str) -> &str {
    const MARKER: &str = "iteration limit after";
    if !data.contains(MARKER) {
        return data;
    }
    let Some(idx) = data.find("Touched files:\n") else {
        return data;
    };
    let tail = &data[idx..];
    let Some(blank) = tail.find("\n\n") else {
        return data;
    };
    tail[blank + 2..].trim_start()
}

/// ponytail: keep densest recent evidence when the dump is huge
fn last_evidence_chunk(observations: &str) -> &str {
    const MAX: usize = 2048;
    if observations.len() <= MAX {
        return observations;
    }
    let mut start = observations.len() - MAX;
    while start < observations.len() && !observations.is_char_boundary(start) {
        start += 1;
    }
    &observations[start..]
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
        return Ok(None);
    }
    context.last_tool_call = Some(call_key);

    let invocation = match tools.invoke(&tool_call.name, &tool_call.arguments) {
        Ok(result) => result,
        // ponytail: emit_blueprint validates in invoke — rejection is a retry nudge, not a job killer
        Err(err) if tool_call.name == "emit_blueprint" => {
            let thought = model_turn.content.unwrap_or_default();
            let step = format!(
                "Thought:\n{thought}\nTool: {}({})\nObservation:\n{err}\n",
                tool_call.name, tool_call.arguments
            );
            context.accumulated_data.push_str(&step);
            return Ok(Some((tool_call.name.clone(), tool_call.arguments.clone())));
        }
        Err(err) => return Err(err),
    };
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

    struct RejectBlueprintTool;

    impl LlmTool for RejectBlueprintTool {
        fn definition(&self) -> &ToolDefinition {
            static DEF: std::sync::OnceLock<ToolDefinition> = std::sync::OnceLock::new();
            DEF.get_or_init(|| ToolDefinition::new("emit_blueprint", "emit_blueprint"))
        }

        fn invoke(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Err("Blueprint rejected: placeholder patch\nFix the JSON and call emit_blueprint again.".to_string())
        }

        fn is_terminal(&self) -> bool {
            true
        }
    }

    struct EmitBlueprintClient;

    impl LlmClient for EmitBlueprintClient {
        fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
            use crate::llm::LlmToolCall;
            Ok(LlmModelTurn {
                content: Some("emit plan".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "emit_blueprint".to_string(),
                    arguments: serde_json::json!({ "blueprint": "{}" }),
                }],
                usage: None,
            })
        }
    }

    #[test]
    fn emit_blueprint_rejection_appends_observation_without_finishing() {
        let tools = LlmToolSet::new().register(RejectBlueprintTool);
        let mut context = AgentContext {
            input_prompt: "plan feature".to_string(),
            accumulated_data: "prior scout\n".to_string(),
            iterations: 1,
            max_iterations: 12,
            is_finished: false,
            agent_completed: false,
            touched_files: Vec::new(),
            last_tool_call: None,
        };

        run_single_tool_turn(&EmitBlueprintClient, &tools, "system", &mut context)
            .expect("blueprint rejection should not abort the loop");

        assert!(!context.is_finished);
        assert!(!context.agent_completed);
        assert!(context.accumulated_data.starts_with("prior scout\n"));
        assert!(context.accumulated_data.contains("Blueprint rejected"));
        assert!(context.accumulated_data.contains("emit_blueprint"));
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

    struct CountingAgent {
        max_process: u32,
    }

    #[async_trait::async_trait]
    impl AutonomousAgent for CountingAgent {
        fn name(&self) -> &'static str {
            "counting_agent"
        }

        async fn enrich_context(&self, _context: &mut AgentContext) -> Result<(), String> {
            Ok(())
        }

        async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
            if context.iterations >= self.max_process {
                context.is_finished = true;
                context.agent_completed = true;
                context.accumulated_data = "done".to_string();
            }
            Ok(())
        }

        async fn mutate_next_iteration(&self, _context: &mut AgentContext) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn resume_extends_iteration_budget_without_resetting_context() {
        let agent = CountingAgent { max_process: 3 };
        let context = AgentContext {
            input_prompt: "plan".to_string(),
            accumulated_data: "scout notes".to_string(),
            iterations: 1,
            max_iterations: 1,
            is_finished: true,
            agent_completed: false,
            touched_files: Vec::new(),
            last_tool_call: None,
        };

        let result = AgentLoopOrchestrator::resume(&agent, context, 1)
            .await
            .expect("resume should succeed");

        assert!(!result.agent_completed);
        assert_eq!(result.iterations, 2);
        assert!(result.accumulated_data.contains("scout notes"));
    }

    #[test]
    fn strip_prior_iteration_cap_removes_nested_header() {
        let capped = "## builder report (iteration limit after 2 of 3 turns)\n\nbuilder did not finalize; partial evidence only.\nWorkspace: /tmp\nTouched files:\n- (none)\n\nTool: read_file\nObservation:\nline 1\n";
        let stripped = strip_prior_iteration_cap(capped);
        assert!(stripped.starts_with("Tool: read_file"));
        assert!(!stripped.contains("iteration limit after"));
    }

    #[test]
    fn last_evidence_chunk_keeps_tail() {
        let big = "x".repeat(3000);
        let chunk = last_evidence_chunk(&big);
        assert_eq!(chunk.len(), 2048);
        assert!(chunk.chars().all(|c| c == 'x'));
        assert_eq!(last_evidence_chunk("short"), "short");
    }

    #[test]
    fn last_evidence_chunk_respects_utf8_boundaries() {
        // 3000 'é' (2 bytes each) so a naive byte cut at len-2048 lands mid-char
        let big = "é".repeat(3000);
        let chunk = last_evidence_chunk(&big);
        assert!(chunk.len() <= 2048);
        assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        assert!(!chunk.is_empty());
    }
}
