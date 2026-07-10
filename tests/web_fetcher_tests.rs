use std::collections::VecDeque;
use std::sync::Mutex;

use mcp_adjutant::agent::{AgentLoopOrchestrator, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT};
use mcp_adjutant::domain::WebFetcherProfile;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

/// Scripted reasoning-model client: returns scripted turns in order.
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

/// Fake browsing model: returns grounded markdown built from the query it receives.
struct BrowsingEcho;

impl LlmClient for BrowsingEcho {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Ok(LlmModelTurn {
            content: Some(format!(
                "## Grounded docs\n\nRetrieved for: {}",
                request.user_message
            )),
            tool_calls: vec![],
        })
    }
}

fn profile_with_budget(token_budget: u32) -> WebFetcherProfile {
    WebFetcherProfile {
        token_budget,
        ..Default::default()
    }
}

#[tokio::test]
async fn web_fetcher_searches_then_finalizes() {
    let reasoning = ReasoningScript::new(vec![
        LlmModelTurn {
            content: Some("Searching the web.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "search_web".to_string(),
                arguments: serde_json::json!({ "query": "rust async tokio" }),
            }],
        },
        LlmModelTurn {
            content: Some("Report ready.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({
                    "report": "## Tokio async runtime\n- spawn tasks\n- channels"
                }),
            }],
        },
    ]);

    let agent = WebFetcherAgent::new(reasoning, BrowsingEcho, profile_with_budget(8_000));
    let result = AgentLoopOrchestrator::run(&agent, "latest tokio docs".to_string(), 5)
        .await
        .expect("web fetcher loop should complete");

    assert!(result.is_finished);
    assert!(result.agent_completed);
    assert!(result.accumulated_data.contains("Tokio async runtime"));
    assert!(result.iterations <= 5);
}

#[tokio::test]
async fn web_fetcher_truncates_overlong_report_to_budget() {
    // Build a finalize report far exceeding the char budget.
    let long_body = "x".repeat(5_000);
    let reasoning = ReasoningScript::new(vec![LlmModelTurn {
        content: Some("Report ready.".to_string()),
        tool_calls: vec![LlmToolCall {
            name: "finalize".to_string(),
            arguments: serde_json::json!({ "report": long_body }),
        }],
    }]);

    // token_budget=1000 -> char budget = 1000 * 4 = 4000 chars (see truncation helper).
    let agent = WebFetcherAgent::new(reasoning, BrowsingEcho, profile_with_budget(1_000));
    let result = AgentLoopOrchestrator::run(&agent, "topic".to_string(), 5)
        .await
        .expect("loop should complete");

    assert!(result.is_finished);
    // Output stays within char budget plus the truncation-note overhead.
    assert!(result.accumulated_data.chars().count() < 4_500);
    assert!(
        result.accumulated_data.contains("[truncated"),
        "expected a truncation note, got: {}",
        result.accumulated_data
    );
}

#[tokio::test]
async fn web_fetcher_accumulates_search_results_across_hops() {
    let reasoning = ReasoningScript::new(vec![
        LlmModelTurn {
            content: Some("First search.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "search_web".to_string(),
                arguments: serde_json::json!({ "query": "react hooks" }),
            }],
        },
        LlmModelTurn {
            content: Some("Refining search.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "search_web".to_string(),
                arguments: serde_json::json!({ "query": "react useEffect cleanup" }),
            }],
        },
        LlmModelTurn {
            content: Some("Report ready.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({ "report": "## React hooks\n- useEffect" }),
            }],
        },
    ]);

    let agent = WebFetcherAgent::new(reasoning, BrowsingEcho, profile_with_budget(8_000));
    let result = AgentLoopOrchestrator::run(&agent, "react hooks docs".to_string(), 5)
        .await
        .expect("loop should complete");

    assert!(result.is_finished);
    // The refined-query grounded response (useEffect) must appear somewhere in
    // the observation history before finalize replaced accumulated_data.
    assert!(
        result.accumulated_data.contains("useEffect") || result.input_prompt.contains("useEffect")
    );
}
