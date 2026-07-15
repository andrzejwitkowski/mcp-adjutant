use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mcp_adjutant::agent::{
    check_finalize_allowed, parse_finalize_arguments, AgentLoopOrchestrator, BabysitterAgent,
    BabysitterSession, SystemBuildRunner, TriageAgent, BABYSITTER_MAX_ITERATIONS,
    BABYSITTER_SYSTEM_PROMPT,
};
use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};
use mcp_adjutant::tools::{LlmBuildDiscoverer, PrReviewComment, PrState};

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

fn green_pr_state(review_comments: Vec<PrReviewComment>) -> PrState {
    PrState {
        number: 1,
        title: "test".into(),
        state: "OPEN".into(),
        mergeable: Some("MERGEABLE".into()),
        head_ref_name: "feat".into(),
        base_ref_name: "main".into(),
        url: "https://example.com/pr/1".into(),
        checks: vec![],
        review_comments,
    }
}

#[test]
fn parse_finalize_arguments_reads_summary_and_skipped_paths() {
    let args = serde_json::json!({
        "summary": "all done",
        "skipped_review_paths": ["src/a.rs", "src/b.rs"]
    });
    let (summary, skipped) = parse_finalize_arguments(&args).expect("parse");
    assert_eq!(summary.as_deref(), Some("all done"));
    assert_eq!(skipped, vec!["src/a.rs", "src/b.rs"]);
}

#[test]
fn check_finalize_allowed_passes_when_report_posted_and_paths_covered() {
    let state = green_pr_state(vec![PrReviewComment {
        path: Some("src/foo.rs".into()),
        line: Some(1),
        body: "fix".into(),
    }]);
    let session = BabysitterSession {
        report_posted: true,
        review_paths_seen: HashSet::from(["src/foo.rs".to_string()]),
        review_paths_handled: HashSet::from(["src/foo.rs".to_string()]),
    };
    check_finalize_allowed(&state, &session, &[]).expect("finalize allowed");
}

#[test]
fn check_finalize_allowed_rejects_without_report() {
    let state = green_pr_state(vec![]);
    let session = BabysitterSession::default();
    let err = check_finalize_allowed(&state, &session, &[]).unwrap_err();
    assert!(err.contains("github_post_final_report"));
}

#[tokio::test]
async fn babysitter_finalize_session_rejected_without_prerequisites() {
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
    .await;

    assert!(result.is_err(), "finalize without report/CI should fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("refusing finalize_session"),
        "unexpected error: {err}"
    );
}
