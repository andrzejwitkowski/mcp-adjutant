use std::collections::VecDeque;
use std::sync::Mutex;

use mcp_adjutant::agent::{AgentLoopOrchestrator, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT};
use mcp_adjutant::domain::WebFetcherProfile;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct ReasoningScript {
    responses: Mutex<VecDeque<LlmModelTurn>>,
}

impl ReasoningScript {
    fn new(responses: Vec<LlmModelTurn>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

impl LlmClient for ReasoningScript {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, WEB_FETCHER_SYSTEM_PROMPT);
        self.responses
            .lock()
            .map_err(|_| "lock poisoned".to_string())?
            .pop_front()
            .ok_or_else(|| "reasoning script out of responses".to_string())
    }
}

fn profile_with_budget(token_budget: u32) -> WebFetcherProfile {
    WebFetcherProfile {
        token_budget,
        ..Default::default()
    }
}

#[tokio::test]
async fn web_fetcher_finalizes_report() {
    let reasoning = ReasoningScript::new(vec![LlmModelTurn {
        content: Some("Report ready.".to_string()),
        tool_calls: vec![LlmToolCall {
            name: "finalize".to_string(),
            arguments: serde_json::json!({
                "report": "## Tokio async runtime\n- spawn tasks\n- channels"
            }),
        }],
        ..Default::default()
    }]);

    let agent = WebFetcherAgent::new(reasoning, profile_with_budget(8_000));
    let result = AgentLoopOrchestrator::run(&agent, "latest tokio docs".to_string(), 5)
        .await
        .expect("web fetcher loop should complete");

    assert!(result.is_finished);
    assert!(result.agent_completed);
    assert!(result.accumulated_data.contains("Tokio async runtime"));
}

#[tokio::test]
async fn web_fetcher_truncates_overlong_report_to_budget() {
    let long_body = "x".repeat(5_000);
    let reasoning = ReasoningScript::new(vec![LlmModelTurn {
        content: Some("Report ready.".to_string()),
        tool_calls: vec![LlmToolCall {
            name: "finalize".to_string(),
            arguments: serde_json::json!({ "report": long_body }),
        }],
        ..Default::default()
    }]);

    let agent = WebFetcherAgent::new(reasoning, profile_with_budget(1_000));
    let result = AgentLoopOrchestrator::run(&agent, "topic".to_string(), 5)
        .await
        .expect("loop should complete");

    assert!(result.is_finished);
    assert!(result.accumulated_data.chars().count() < 4_500);
    assert!(result.accumulated_data.contains("[truncated"));
}
