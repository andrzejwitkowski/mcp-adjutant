mod common;

use std::sync::{Arc, Mutex};

use mcp_adjutant::agent::{AgentLoopOrchestrator, EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT};
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest};
use mcp_adjutant::metrics::{init, list_evaluations, MetricsStore};
use rusqlite::params;

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
                "{\"score\": 6, \"critique\": \"Too many comments in the code.\", \"desired_output\": \"## scout report\\nsrc/sample.rs:10 invoke()\"}".to_string(),
            ),
            tool_calls: vec![],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn evaluator_agent_stores_judgment_in_metrics_db() {
    let dir = std::env::temp_dir().join(format!(
        "evaluator-metrics-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmpdir");
    let db_path = dir.join("metrics.db");
    let store = Arc::new(Mutex::new(MetricsStore::open(&db_path).expect("open")));
    init(
        format!("session-eval-{}", std::process::id()),
        Arc::clone(&store),
    );

    let agent = EvaluatorAgent::new(
        MockEvaluatorLlm,
        "Phase_1_Scout",
        "Find invoke call sites in sample.rs",
        "report without evidence",
    );

    let result = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect("evaluator orchestrator run");

    assert!(result.is_finished);
    assert!(result.accumulated_data.contains("QA score: 6/10"));
    assert!(result
        .accumulated_data
        .contains("Too many comments in the code."));
    assert!(result
        .accumulated_data
        .contains("Desired output (10/10 exemplar):"));
    assert!(result
        .accumulated_data
        .contains("src/sample.rs:10 invoke()"));

    let guard = store.lock().expect("lock");
    let rows = list_evaluations(guard.connection()).expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent_name, "Phase_1_Scout");
    assert_eq!(rows[0].score, 6);
    assert_eq!(rows[0].feedback_notes, "Too many comments in the code.");
    assert_eq!(
        rows[0].desired_output,
        "## scout report\nsrc/sample.rs:10 invoke()"
    );

    // also verify via raw SQL
    let (agent_name, score): (String, i32) = guard
        .connection()
        .query_row(
            "SELECT agent_name, score FROM agent_evaluations WHERE agent_name = ?1",
            params!["Phase_1_Scout"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("row");
    assert_eq!(agent_name, "Phase_1_Scout");
    assert_eq!(score, 6);

    let _ = std::fs::remove_dir_all(&dir);
}
