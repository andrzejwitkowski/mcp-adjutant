use std::collections::HashSet;

use mcp_adjutant::agent::{check_finalize_allowed, parse_finalize_arguments, BabysitterSession};
use mcp_adjutant::tools::{PrReviewComment, PrState};

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
