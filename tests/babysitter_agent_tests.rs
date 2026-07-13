use std::sync::{Arc, Mutex};

use mcp_adjutant::agent::{
    AgentLoopOrchestrator, BabysitterAgent, BABYSITTER_SYSTEM_PROMPT, SystemBuildRunner,
    TriageAgent, BABYSITTER_MAX_ITERATIONS,
};
use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};
use mcp_adjutant::tools::LlmBuildDiscoverer;

struct MockBabysitterLlm {
    turns: Mutex<Vec<LlmModelTurn>>,
}

impl MockBabysitterLlm {
    fn finalize_only() -> Self {
        Self {
            turns: Mutex::new(vec![LlmModelTurn {
                content: Some("done babysitting".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "finalize_session".to_string(),
                    arguments: serde_json::json!({"summary": "no blockers"}),
                }],
                usage: None,
            }]),
        }
    }
}

impl LlmClient for MockBabysitterLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, BABYSITTER_SYSTEM_PROMPT);
        let mut turns = self.turns.lock().map_err(|_| "lock poisoned")?;
        turns
            .pop()
            .ok_or_else(|| "no mock turns left".to_string())
    }
}

struct NoopTriageLlm;

impl LlmClient for NoopTriageLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Ok(LlmModelTurn {
            content: Some("noop".to_string()),
            tool_calls: vec![],
            usage: None,
        })
    }
}

#[tokio::test]
async fn babysitter_finalize_session_completes_loop() {
    let config = Arc::new(AdjutantConfig::default());
    let triage_client = NoopTriageLlm;
    let scout_client = NoopTriageLlm;
    let discoverer = LlmBuildDiscoverer::new(scout_client);
    let triage_agent = TriageAgent::with_build_runner_and_discoverer(
        triage_client,
        Vec::new(),
        Arc::clone(&config),
        SystemBuildRunner,
        discoverer,
    );
    let agent = BabysitterAgent::new(MockBabysitterLlm::finalize_only(), config, triage_agent, 1);

    let result = AgentLoopOrchestrator::run(
        &agent,
        "babysit_pr\nPR #1".to_string(),
        BABYSITTER_MAX_ITERATIONS,
    )
    .await
    .expect("babysitter loop");

    assert!(result.is_finished);
    assert!(result.agent_completed);
    assert!(result.accumulated_data.contains("no blockers"));
}
