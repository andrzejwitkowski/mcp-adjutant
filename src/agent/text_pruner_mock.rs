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
        let source = if context.iterations <= 1 {
            &context.input_prompt
        } else {
            &context.accumulated_data
        };

        let source_char_len = source.chars().count();
        let feedback_rounds = mutation_rounds(&context.input_prompt);
        let target_len = prune_target_len(source_char_len, context.iterations, feedback_rounds);
        let pruned = source.chars().take(target_len).collect::<String>();

        context.is_finished = pruned.chars().count() < MAX_OUTPUT_CHARS;
        context.accumulated_data = pruned;

        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context.input_prompt.push_str(MUTATION_SUFFIX);
        Ok(())
    }
}

fn mutation_rounds(input_prompt: &str) -> u32 {
    input_prompt.matches(MUTATION_SUFFIX).count() as u32
}

fn prune_target_len(current_len: usize, iteration: u32, feedback_rounds: u32) -> usize {
    let keep_percent = 70u32
        .saturating_sub(iteration.saturating_mul(10))
        .saturating_sub(feedback_rounds.saturating_mul(5))
        .max(20);
    (current_len * keep_percent as usize / 100).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_target_len_shrinks_progressively() {
        assert!(prune_target_len(300, 1, 0) < 300);
        assert!(prune_target_len(300, 3, 0) < prune_target_len(300, 1, 0));
    }

    #[test]
    fn prune_target_len_responds_to_mutation_feedback() {
        assert!(prune_target_len(300, 2, 2) < prune_target_len(300, 2, 0));
    }
}
