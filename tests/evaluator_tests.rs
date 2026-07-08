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
        assert!(request.user_message.contains("Znajdź wywołania invoke"));
        assert!(request.user_message.contains("raport bez dowodów"));

        Ok(LlmModelTurn {
            content: Some(
                r#"{"score": 6, "critique": "Za dużo komentarzy w kodzie."}"#.to_string(),
            ),
            tool_calls: vec![],
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
        "Znajdź wywołania invoke w sample.rs",
        "raport bez dowodów",
    );

    let result = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect("evaluator orchestrator run");

    assert!(result.is_finished);
    assert_eq!(
        result.accumulated_data,
        "Ewaluacja zapisana. Ocena QA: 6/10"
    );

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
    assert_eq!(feedback, "Za dużo komentarzy w kodzie.");

    std::fs::remove_dir_all(&project_root).ok();
}

struct FixedScoreLlm(i32);

impl LlmClient for FixedScoreLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Ok(LlmModelTurn {
            content: Some(format!(
                r#"{{"score": {}, "critique": "test critique"}}"#,
                self.0
            )),
            tool_calls: vec![],
        })
    }
}

#[tokio::test]
async fn evaluator_agent_rejects_score_below_valid_range() {
    let project_root = common::unique_temp_project("evaluator-score-low");
    std::fs::create_dir_all(&project_root).expect("create temp project");
    common::write_demo_cargo_manifest(&project_root);

    let cache_manager = Arc::new(Mutex::new(common::open_cache_manager(&project_root)));
    let agent = EvaluatorAgent::new(
        FixedScoreLlm(0),
        Arc::clone(&cache_manager),
        "Phase_1_Scout",
        "task",
        "output",
    );

    let error = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect_err("score of 0 should be rejected");

    assert!(error.contains("evaluator score must be between 1 and 10"));

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn evaluator_agent_rejects_score_above_valid_range() {
    let project_root = common::unique_temp_project("evaluator-score-high");
    std::fs::create_dir_all(&project_root).expect("create temp project");
    common::write_demo_cargo_manifest(&project_root);

    let cache_manager = Arc::new(Mutex::new(common::open_cache_manager(&project_root)));
    let agent = EvaluatorAgent::new(
        FixedScoreLlm(11),
        Arc::clone(&cache_manager),
        "Phase_1_Scout",
        "task",
        "output",
    );

    let error = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect_err("score of 11 should be rejected");

    assert!(error.contains("evaluator score must be between 1 and 10"));

    std::fs::remove_dir_all(&project_root).ok();
}

struct MissingContentLlm;

impl LlmClient for MissingContentLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Ok(LlmModelTurn {
            content: None,
            tool_calls: vec![],
        })
    }
}

#[tokio::test]
async fn evaluator_agent_errors_when_model_response_has_no_content() {
    let project_root = common::unique_temp_project("evaluator-no-content");
    std::fs::create_dir_all(&project_root).expect("create temp project");
    common::write_demo_cargo_manifest(&project_root);

    let cache_manager = Arc::new(Mutex::new(common::open_cache_manager(&project_root)));
    let agent = EvaluatorAgent::new(
        MissingContentLlm,
        Arc::clone(&cache_manager),
        "Phase_1_Scout",
        "task",
        "output",
    );

    let error = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect_err("missing content should error");

    assert!(error.contains("evaluator model response missing content"));

    std::fs::remove_dir_all(&project_root).ok();
}

struct MalformedJsonLlm;

impl LlmClient for MalformedJsonLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Ok(LlmModelTurn {
            content: Some("this is not valid json".to_string()),
            tool_calls: vec![],
        })
    }
}

#[tokio::test]
async fn evaluator_agent_errors_on_malformed_json_response() {
    let project_root = common::unique_temp_project("evaluator-malformed-json");
    std::fs::create_dir_all(&project_root).expect("create temp project");
    common::write_demo_cargo_manifest(&project_root);

    let cache_manager = Arc::new(Mutex::new(common::open_cache_manager(&project_root)));
    let agent = EvaluatorAgent::new(
        MalformedJsonLlm,
        Arc::clone(&cache_manager),
        "Phase_1_Scout",
        "task",
        "output",
    );

    let error = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect_err("malformed json should error");

    assert!(error.contains("failed to parse evaluator JSON response"));

    std::fs::remove_dir_all(&project_root).ok();
}

struct FencedJsonLlm;

impl LlmClient for FencedJsonLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Ok(LlmModelTurn {
            content: Some(
                "```json\n{\"score\": 9, \"critique\": \"Świetna robota.\"}\n```".to_string(),
            ),
            tool_calls: vec![],
        })
    }
}

#[tokio::test]
async fn evaluator_agent_parses_response_wrapped_in_json_code_fence() {
    let project_root = common::unique_temp_project("evaluator-fenced-json");
    std::fs::create_dir_all(&project_root).expect("create temp project");
    common::write_demo_cargo_manifest(&project_root);

    let cache_manager = Arc::new(Mutex::new(common::open_cache_manager(&project_root)));
    let agent = EvaluatorAgent::new(
        FencedJsonLlm,
        Arc::clone(&cache_manager),
        "Phase_2_Builder",
        "task",
        "output",
    );

    let result = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect("evaluator orchestrator run");

    assert!(result.is_finished);
    assert_eq!(result.accumulated_data, "Ewaluacja zapisana. Ocena QA: 9/10");

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn evaluator_agent_enriches_prompt_with_system_prompt_when_missing_marker() {
    let project_root = common::unique_temp_project("evaluator-enrich");
    std::fs::create_dir_all(&project_root).expect("create temp project");
    common::write_demo_cargo_manifest(&project_root);

    let cache_manager = Arc::new(Mutex::new(common::open_cache_manager(&project_root)));
    let agent = EvaluatorAgent::new(
        FencedJsonLlm,
        Arc::clone(&cache_manager),
        "Phase_2_Builder",
        "task",
        "output",
    );

    let result = AgentLoopOrchestrator::run(&agent, "evaluate_agent_performance".to_string(), 1)
        .await
        .expect("evaluator orchestrator run");

    assert!(result.input_prompt.contains(EVALUATOR_SYSTEM_PROMPT));

    std::fs::remove_dir_all(&project_root).ok();
}

#[test]
fn public_api_exports_evaluator_symbols_from_crate_root() {
    let _system_prompt: &str = mcp_adjutant::EVALUATOR_SYSTEM_PROMPT;
    let _tool_name: &str = mcp_adjutant::EVALUATE_AGENT_PERFORMANCE_TOOL_NAME;

    let schema = mcp_adjutant::evaluate_agent_performance_schema();
    assert_eq!(schema["name"], mcp_adjutant::EVALUATE_AGENT_PERFORMANCE_TOOL_NAME);

    let _create_client: fn(
        &mcp_adjutant::AdjutantConfig,
    ) -> Result<mcp_adjutant::ConfiguredLlmClient, String> =
        mcp_adjutant::create_evaluator_llm_client;
}
