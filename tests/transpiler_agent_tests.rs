use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mcp_adjutant::agent::{
    AgentLoopOrchestrator, SystemBuildRunner, TranspilerAgent, TriageAgent,
    TRANSPILER_MAX_ITERATIONS, TRANSPILER_SYSTEM_PROMPT,
};
use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct MockTranspilerLlm {
    turns: Mutex<Vec<LlmModelTurn>>,
}

impl MockTranspilerLlm {
    fn report_error_only() -> Self {
        Self {
            turns: Mutex::new(vec![LlmModelTurn {
                content: Some("giving up".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "report_error".to_string(),
                    arguments: serde_json::json!({"reason": "triage never passed"}),
                }],
                usage: None,
            }]),
        }
    }
}

impl LlmClient for MockTranspilerLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, TRANSPILER_SYSTEM_PROMPT);
        let mut turns = self.turns.lock().map_err(|_| "lock poisoned")?;
        turns.pop().ok_or_else(|| "no mock turns left".to_string())
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
async fn transpiler_report_error_completes_loop() {
    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        NoopTriageLlm,
        Vec::new(),
        Arc::clone(&config),
        SystemBuildRunner,
    );
    let agent = TranspilerAgent::new(
        MockTranspilerLlm::report_error_only(),
        triage_agent,
        PathBuf::from("frontend/src/modules/config-ui/api-types.generated.ts"),
        vec![PathBuf::from("frontend/src/modules/config-ui/types.ts")],
        PathBuf::from("frontend"),
        Some("npm run typecheck".to_string()),
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "transpile_types\ntarget api types".to_string(),
        TRANSPILER_MAX_ITERATIONS,
    )
    .await
    .expect("transpiler loop");

    assert!(result.is_finished);
    assert!(!result.agent_completed);
    assert!(result.accumulated_data.contains("triage never passed"));
}
