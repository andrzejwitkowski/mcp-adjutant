use mcp_adjutant::agent::{AgentLoopOrchestrator, TextPrunerMock};

#[tokio::test]
async fn text_pruner_mock_loops_mutates_and_stops_when_short_enough() {
    let agent = TextPrunerMock;
    let long_prompt = "x".repeat(300);

    let result = AgentLoopOrchestrator::run(&agent, long_prompt, 10)
        .await
        .expect("orchestrator should complete without error");

    assert!(
        result.is_finished,
        "loop should stop when output is short enough"
    );
    assert!(
        result.accumulated_data.len() < 100,
        "pruned output should be under 100 characters, got {}",
        result.accumulated_data.len()
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
        result.input_prompt.contains("Wciąż za długie"),
        "mutation feedback should be present after failed iterations"
    );
}

#[tokio::test]
async fn text_pruner_mock_respects_max_iterations() {
    let agent = TextPrunerMock;
    let long_prompt = "y".repeat(10_000);

    let result = AgentLoopOrchestrator::run(&agent, long_prompt, 2)
        .await
        .expect("orchestrator should complete without error");

    assert_eq!(result.iterations, 2, "loop should stop at max_iterations");
    assert!(
        !result.is_finished,
        "output should not be short enough within only 2 iterations"
    );
}
