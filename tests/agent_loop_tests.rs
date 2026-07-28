use async_trait::async_trait;
use mcp_adjutant::agent::{AgentContext, AgentLoopOrchestrator, AutonomousAgent};

/// Local stub: shrinks prompt each turn until under 100 chars (replaces TextPrunerMock).
struct ShrinkStub;

#[async_trait]
impl AutonomousAgent for ShrinkStub {
    fn name(&self) -> &'static str {
        "shrink_stub"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        context.input_prompt.push_str("\n[MUST BE LESS THAN 100 CHARS]");
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let source = if context.iterations <= 1 {
            &context.input_prompt
        } else {
            &context.accumulated_data
        };
        let keep = (source.chars().count() * 70 / 100).max(1);
        let pruned: String = source.chars().take(keep).collect();
        context.is_finished = pruned.chars().count() < 100;
        context.accumulated_data = pruned;
        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context
            .input_prompt
            .push_str("\nStill too long, apply more aggressive pruning");
        Ok(())
    }
}

#[tokio::test]
async fn shrink_stub_loops_mutates_and_stops_when_short_enough() {
    let agent = ShrinkStub;
    let long_prompt = "x".repeat(300);

    let result = AgentLoopOrchestrator::run(&agent, long_prompt, 10)
        .await
        .expect("orchestrator should complete without error");

    assert!(
        result.is_finished,
        "loop should stop when output is short enough"
    );
    assert!(
        result.accumulated_data.chars().count() < 100,
        "pruned output should be under 100 characters, got {}",
        result.accumulated_data.chars().count()
    );
    assert!(
        result.iterations > 1,
        "loop should iterate multiple times before finishing, got {}",
        result.iterations
    );
    assert!(
        result
            .input_prompt
            .contains("[MUST BE LESS THAN 100 CHARS]"),
        "enrichment requirement should be present in context"
    );
    assert!(
        result.input_prompt.contains("Still too long"),
        "mutation feedback should be present after failed iterations"
    );
}

#[tokio::test]
async fn shrink_stub_respects_max_iterations() {
    let agent = ShrinkStub;
    let long_prompt = "y".repeat(10_000);

    let result = AgentLoopOrchestrator::run(&agent, long_prompt, 2)
        .await
        .expect("orchestrator should complete without error");

    assert_eq!(result.iterations, 2, "loop should stop at max_iterations");
    assert!(
        result.is_finished,
        "orchestrator should hard-stop and mark finished at max_iterations"
    );
    assert!(
        result.accumulated_data.contains("iteration limit"),
        "hard stop should wrap accumulated observations"
    );
}
