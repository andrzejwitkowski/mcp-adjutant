use std::collections::VecDeque;
use std::sync::Mutex;

use mcp_adjutant::agent::{AgentLoopOrchestrator, ChatClient, ScoutAgent, SCOUT_SYSTEM_PROMPT};

struct ScriptClient {
    responses: Mutex<VecDeque<String>>,
}

impl ScriptClient {
    fn new(responses: Vec<&str>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
        }
    }
}

struct ReactiveScriptClient;

impl ChatClient for ReactiveScriptClient {
    fn complete(&self, system_prompt: &str, user_message: &str) -> Result<String, String> {
        assert_eq!(system_prompt, SCOUT_SYSTEM_PROMPT);

        if user_message.contains("Call sites at lines") {
            return Ok("ACTION: finalize(report=\"found invoke calls\")\n".to_string());
        }

        Ok(
            "ACTION: ast_calls(file=\"tests/fixtures/scout/sample.rs\", method=\"invoke\")\n"
                .to_string(),
        )
    }
}

impl ChatClient for ScriptClient {
    fn complete(&self, system_prompt: &str, _user_message: &str) -> Result<String, String> {
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
        "Thought: szeroki zwiad\nACTION: read_file(file=\"tests/fixtures/scout/readme.txt\", start=1, end=2)\n",
        "Thought: raport gotowy\nACTION: finalize(report=\"## Scout\\n- alpha marker\")\n",
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
