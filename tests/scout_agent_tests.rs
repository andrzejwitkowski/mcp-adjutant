use std::collections::VecDeque;
use std::sync::Mutex;

use mcp_adjutant::agent::{
    AgentLoopOrchestrator, ChatClient, ScoutAgent, ScoutModelTurn, ScoutToolCall,
    SCOUT_SYSTEM_PROMPT,
};

struct ScriptClient {
    responses: Mutex<VecDeque<ScoutModelTurn>>,
}

impl ScriptClient {
    fn new(responses: Vec<ScoutModelTurn>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

struct ReactiveScriptClient;

impl ChatClient for ReactiveScriptClient {
    fn complete(&self, system_prompt: &str, user_message: &str) -> Result<ScoutModelTurn, String> {
        assert_eq!(system_prompt, SCOUT_SYSTEM_PROMPT);

        if user_message.contains("Call sites at lines") {
            return Ok(ScoutModelTurn {
                content: Some("Raport gotowy.".to_string()),
                tool_calls: vec![ScoutToolCall {
                    name: "finalize".to_string(),
                    arguments: serde_json::json!({ "report": "found invoke calls" }),
                }],
            });
        }

        Ok(ScoutModelTurn {
            content: Some("Szukam wywołań AST.".to_string()),
            tool_calls: vec![ScoutToolCall {
                name: "ast_calls".to_string(),
                arguments: serde_json::json!({
                    "file": "tests/fixtures/scout/sample.rs",
                    "method": "invoke"
                }),
            }],
        })
    }
}

impl ChatClient for ScriptClient {
    fn complete(&self, system_prompt: &str, _user_message: &str) -> Result<ScoutModelTurn, String> {
        assert!(
            system_prompt.contains("PHASE_1_SCOUT"),
            "system prompt should include scout contract"
        );
        assert_eq!(system_prompt, SCOUT_SYSTEM_PROMPT);

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
        ScoutModelTurn {
            content: Some("Szeroki zwiad pliku.".to_string()),
            tool_calls: vec![ScoutToolCall {
                name: "read_file".to_string(),
                arguments: serde_json::json!({
                    "file": "tests/fixtures/scout/readme.txt",
                    "start": 1,
                    "end": 2
                }),
            }],
        },
        ScoutModelTurn {
            content: Some("Raport gotowy.".to_string()),
            tool_calls: vec![ScoutToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({ "report": "## Scout\n- alpha marker" }),
            }],
        },
    ]);

    let agent = ScoutAgent::new(client);
    let result = AgentLoopOrchestrator::run(&agent, "Znajdź marker".to_string(), 5)
        .await
        .expect("scout loop should complete");

    assert!(result.is_finished);
    assert!(result.accumulated_data.contains("alpha marker"));
    assert!(result.accumulated_data.contains("## Scout"));
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
}
