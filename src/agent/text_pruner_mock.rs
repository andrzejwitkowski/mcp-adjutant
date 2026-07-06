use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};

const MAX_OUTPUT_CHARS: usize = 100;
const ENRICHMENT_SUFFIX: &str = "\n[MUST BE LESS THAN 100 CHARS]";
const MUTATION_SUFFIX: &str = "\nWciąż za długie, wykonaj bardziej agresywny prunining";

pub struct TextPrunerMock;

#[async_trait]
impl AutonomousAgent for TextPrunerMock {
    fn name(&self) -> &'static str {
        "text_pruner_mock"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        context.input_prompt.push_str(ENRICHMENT_SUFFIX);
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let source = if context.accumulated_data.is_empty() {
            &context.input_prompt
        } else {
            &context.accumulated_data
        };

        let target_len = prune_target_len(source.len(), context.iterations);
        let pruned = source.chars().take(target_len).collect::<String>();

        context.accumulated_data = pruned;
        context.is_finished = context.accumulated_data.len() < MAX_OUTPUT_CHARS;

        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context.input_prompt.push_str(MUTATION_SUFFIX);
        Ok(())
    }
}

fn prune_target_len(current_len: usize, iteration: u32) -> usize {
    let keep_percent = 70u32.saturating_sub(iteration.saturating_mul(10)).max(20);
    (current_len * keep_percent as usize / 100).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_target_len_shrinks_progressively() {
        assert!(prune_target_len(300, 1) < 300);
        assert!(prune_target_len(300, 3) < prune_target_len(300, 1));
    }
}
