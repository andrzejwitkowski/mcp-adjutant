mod common;

use std::sync::{Arc, Mutex};

use mcp_adjutant::agent::{AgentLoopOrchestrator, EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT};
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest};
use rusqlite::{params, Connection};

struct MockEvaluatorLlm;

impl LlmClient for MockEvaluatorLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, EVALUATOR_SYSTEM_PROMPT);
        assert!(request.tools.definitions().is_empty());
        assert!(request.user_message.contains("Phase_1_Scout"));
        assert!(request.user_message.contains("Find invoke call sites"));
        assert!(request.user_message.contains("report without evidence"));

        Ok(LlmModelTurn {
            content: Some(
                r#"{"score": 6, "critique": "Too many comments in the code."}"#.to_string(),
            ),
            tool_calls: vec![],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn evaluator_agent_stores_judgment_in_sqlite() {
    let project_root = common::unique_temp_project("evaluator");
    std::fs::create_dir_all(&project_root).expect("create temp project");
    common::write_demo_cargo_manifest(&project_root);

    let cache_manager = Arc::new(Mutex::new(common::open_cache_manager(&project_root)));
    let agent = EvaluatorAgent::new(
        MockEvaluatorLlm,
        Arc::clone(&cache_manager),
        "Phase_1_Scout",
        "Find invoke call sites in sample.rs",
        "report without evidence",
    );

    let result = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect("evaluator orchestrator run");

    assert!(result.is_finished);
    assert_eq!(result.accumulated_data, "Evaluation saved. QA score: 6/10");

    let db_path = project_root.join(".adjutant/cache.db");
    let conn = Connection::open(&db_path).expect("open cache.db");
    let (agent_name, score, feedback): (String, i32, String) = conn
        .query_row(
            "SELECT agent_name, score, feedback_notes FROM agent_evaluations WHERE agent_name = ?1",
            params!["Phase_1_Scout"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("evaluation row");

    assert_eq!(agent_name, "Phase_1_Scout");
    assert_eq!(score, 6);
    assert_eq!(feedback, "Too many comments in the code.");

    std::fs::remove_dir_all(&project_root).ok();
}
