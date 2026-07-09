use super::traits::{AgentContext, AutonomousAgent};

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
