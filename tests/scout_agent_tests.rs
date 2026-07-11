use std::collections::VecDeque;
use std::sync::Mutex;

use mcp_adjutant::agent::{AgentLoopOrchestrator, ScoutAgent, SCOUT_SYSTEM_PROMPT};
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct ScriptClient {
    responses: Mutex<VecDeque<LlmModelTurn>>,
}

impl ScriptClient {
    fn new(responses: Vec<LlmModelTurn>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

struct ReactiveScriptClient;

impl LlmClient for ReactiveScriptClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, SCOUT_SYSTEM_PROMPT);

        if request.user_message.contains("Call sites at lines") {
            return Ok(LlmModelTurn {
                content: Some("Report ready.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "finalize".to_string(),
                    arguments: serde_json::json!({ "report": "found invoke calls" }),
                }],
                ..Default::default()
            });
        }

        Ok(LlmModelTurn {
            content: Some("Searching for AST call sites.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "ast_calls".to_string(),
                arguments: serde_json::json!({
                    "file": "tests/fixtures/scout/sample.rs",
                    "method": "invoke"
                }),
            }],
            ..Default::default()
        })
    }
}

impl LlmClient for ScriptClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert!(
            request.system_prompt.contains("PHASE_1_SCOUT"),
            "system prompt should include scout contract"
        );
        assert_eq!(request.system_prompt, SCOUT_SYSTEM_PROMPT);

        let mut queue = self
            .responses
            .lock()
            .map_err(|_| "script client lock poisoned".to_string())?;

        queue
            .pop_front()
            .ok_or_else(|| "script client out of responses".to_string())
    }
}

#[tokio::test]
async fn scout_agent_executes_react_tools_then_finalizes() {
    let client = ScriptClient::new(vec![
        LlmModelTurn {
            content: Some("Broad file search.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "read_file".to_string(),
                arguments: serde_json::json!({
                    "file": "tests/fixtures/scout/readme.txt",
                    "start": 1,
                    "end": 2
                }),
            }],
            ..Default::default()
        },
        LlmModelTurn {
            content: Some("Report ready.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({ "report": "## Scout\n- alpha marker" }),
            }],
            ..Default::default()
        },
    ]);

    let agent = ScoutAgent::new(client);
    let result = AgentLoopOrchestrator::run(&agent, "Find marker".to_string(), 5)
        .await
        .expect("scout loop should complete");

    assert!(result.is_finished);
    assert!(result.accumulated_data.contains("alpha marker"));
    assert!(result.accumulated_data.contains("## Scout"));
    assert!(result.agent_completed);
    assert!(result
        .touched_files
        .iter()
        .any(|path| path.ends_with("tests/fixtures/scout/readme.txt")));
    assert!(
        result.input_prompt.contains("PHASE_1_SCOUT"),
        "enriched prompt should contain scout contract"
    );
}

#[tokio::test]
async fn scout_agent_parses_ast_calls_action() {
    let agent = ScoutAgent::new(ReactiveScriptClient);
    let result = AgentLoopOrchestrator::run(&agent, "call sites".to_string(), 5)
        .await
        .expect("scout loop should complete");

    assert!(result.is_finished);
    assert_eq!(result.accumulated_data, "found invoke calls");
    assert_eq!(result.iterations, 2);
    assert!(result.agent_completed);
    assert!(result
        .touched_files
        .iter()
        .any(|path| path.ends_with("tests/fixtures/scout/sample.rs")));
}

#[tokio::test]
async fn scout_agent_exposes_registered_tool_catalog() {
    let agent = ScoutAgent::new(ReactiveScriptClient);
    let names: Vec<_> = agent
        .tools()
        .definitions()
        .into_iter()
        .map(|tool| tool.name.clone())
        .collect();

    assert!(names.contains(&"detect_language".to_string()));
    assert!(names.contains(&"finalize".to_string()));
}
