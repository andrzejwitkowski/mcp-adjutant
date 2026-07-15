use std::collections::HashSet;

use crate::tools::{ci_checks_blocking, PrState};

#[derive(Debug, Clone, Default)]
pub struct BabysitterSession {
    pub report_posted: bool,
    pub review_paths_seen: HashSet<String>,
    pub review_paths_handled: HashSet<String>,
}

pub fn uncovered_review_paths(
    seen: &HashSet<String>,
    handled: &HashSet<String>,
    skipped: &[String],
) -> Vec<String> {
    let mut uncovered: Vec<_> = seen
        .iter()
        .filter(|path| !handled.contains(*path) && !skipped.iter().any(|skip| skip == *path))
        .cloned()
        .collect();
    uncovered.sort();
    uncovered
}

pub fn check_finalize_allowed(
    state: &PrState,
    session: &BabysitterSession,
    skipped_review_paths: &[String],
) -> Result<(), String> {
    let blocking = ci_checks_blocking(&state.checks);
    if !blocking.is_empty() {
        return Err(format!(
            "refusing finalize_session: CI not green ({}) — wait for checks to pass or fix failures first",
            blocking.join(", ")
        ));
    }

    if !session.report_posted {
        return Err(
            "refusing finalize_session: call github_post_final_report before finalize_session"
                .to_string(),
        );
    }

    for path in skipped_review_paths {
        if !session.review_paths_seen.contains(path) {
            return Err(format!(
                "refusing finalize_session: skipped_review_paths contains {path:?} which was not in review comments — only cite paths from github_get_pr_state"
            ));
        }
    }

    let uncovered = uncovered_review_paths(
        &session.review_paths_seen,
        &session.review_paths_handled,
        skipped_review_paths,
    );
    if !uncovered.is_empty() {
        return Err(format!(
            "refusing finalize_session: review paths not triaged or skipped: {} — invoke_child_triage on each path or list nitpicks in skipped_review_paths",
            uncovered.join(", ")
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{review_comment_paths, PrCheck, PrReviewComment};

    fn seed_review_paths(session: &mut BabysitterSession, state: &PrState) {
        for path in review_comment_paths(&state.review_comments) {
            session.review_paths_seen.insert(path);
        }
    }

    #[test]
    fn uncovered_paths_excludes_handled_and_skipped() {
        let seen = HashSet::from([
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
            "src/c.rs".to_string(),
        ]);
        let handled = HashSet::from(["src/a.rs".to_string()]);
        let skipped = vec!["src/c.rs".to_string()];
        assert_eq!(
            uncovered_review_paths(&seen, &handled, &skipped),
            vec!["src/b.rs".to_string()]
        );
    }

    #[test]
    fn check_finalize_rejects_pending_ci() {
        let state = PrState {
            number: 1,
            title: "t".into(),
            state: "OPEN".into(),
            mergeable: None,
            head_ref_name: "feat".into(),
            base_ref_name: "main".into(),
            url: "u".into(),
            checks: vec![PrCheck {
                name: "Rust Backend".into(),
                bucket: "pending".into(),
                state: "IN_PROGRESS".into(),
                workflow: None,
                link: None,
            }],
            review_comments: vec![],
        };
        let session = BabysitterSession {
            report_posted: true,
            ..Default::default()
        };
        let err = check_finalize_allowed(&state, &session, &[]).unwrap_err();
        assert!(err.contains("CI not green"));
    }

    #[test]
    fn check_finalize_rejects_without_report() {
        let state = PrState {
            number: 1,
            title: "t".into(),
            state: "OPEN".into(),
            mergeable: None,
            head_ref_name: "feat".into(),
            base_ref_name: "main".into(),
            url: "u".into(),
            checks: vec![],
            review_comments: vec![],
        };
        let session = BabysitterSession::default();
        let err = check_finalize_allowed(&state, &session, &[]).unwrap_err();
        assert!(err.contains("github_post_final_report"));
    }

    #[test]
    fn check_finalize_allows_handled_paths() {
        let state = PrState {
            number: 1,
            title: "t".into(),
            state: "OPEN".into(),
            mergeable: None,
            head_ref_name: "feat".into(),
            base_ref_name: "main".into(),
            url: "u".into(),
            checks: vec![],
            review_comments: vec![PrReviewComment {
                path: Some("src/foo.rs".into()),
                line: Some(1),
                body: "fix".into(),
            }],
        };
        let mut session = BabysitterSession {
            report_posted: true,
            ..Default::default()
        };
        seed_review_paths(&mut session, &state);
        session
            .review_paths_handled
            .insert("src/foo.rs".to_string());
        assert!(check_finalize_allowed(&state, &session, &[]).is_ok());
    }

    #[test]
    fn check_finalize_rejects_unhandled_review_paths() {
        let state = PrState {
            number: 1,
            title: "t".into(),
            state: "OPEN".into(),
            mergeable: None,
            head_ref_name: "feat".into(),
            base_ref_name: "main".into(),
            url: "u".into(),
            checks: vec![],
            review_comments: vec![PrReviewComment {
                path: Some("src/foo.rs".into()),
                line: Some(1),
                body: "fix".into(),
            }],
        };
        let mut session = BabysitterSession {
            report_posted: true,
            ..Default::default()
        };
        seed_review_paths(&mut session, &state);
        let err = check_finalize_allowed(&state, &session, &[]).unwrap_err();
        assert!(err.contains("src/foo.rs"));
    }

    #[test]
    fn check_finalize_allows_skipped_paths() {
        let state = PrState {
            number: 1,
            title: "t".into(),
            state: "OPEN".into(),
            mergeable: None,
            head_ref_name: "feat".into(),
            base_ref_name: "main".into(),
            url: "u".into(),
            checks: vec![],
            review_comments: vec![PrReviewComment {
                path: Some("src/foo.rs".into()),
                line: None,
                body: "nit".into(),
            }],
        };
        let mut session = BabysitterSession {
            report_posted: true,
            ..Default::default()
        };
        seed_review_paths(&mut session, &state);
        assert!(check_finalize_allowed(&state, &session, &["src/foo.rs".to_string()]).is_ok());
    }
}
