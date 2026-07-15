use std::process::Command;

use serde::Deserialize;

use crate::cache::mcp_workspace_root;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrCheck {
    pub name: String,
    pub bucket: String,
    pub state: String,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhPrView {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    mergeable: Option<String>,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrReviewComment {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct PrState {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub mergeable: Option<String>,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub url: String,
    pub checks: Vec<PrCheck>,
    pub review_comments: Vec<PrReviewComment>,
}

fn run_gh_capture(args: &[&str]) -> Result<String, String> {
    let repo_root = mcp_workspace_root();
    let output = Command::new("gh")
        .current_dir(&repo_root)
        .args(args)
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "gh CLI not found; install GitHub CLI".into()
            } else {
                format!("failed to spawn gh: {err}")
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let hint = if stderr.contains("auth") || stderr.contains("login") {
            " (run `gh auth login`)"
        } else {
            ""
        };
        return Err(format!(
            "gh {} failed (exit {}): {}{}",
            args.first().copied().unwrap_or(""),
            output.status,
            if stderr.is_empty() {
                "no stderr".to_string()
            } else {
                stderr
            },
            hint
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn extract_run_id_from_link(link: &str) -> Option<u64> {
    let segment = link.split("/actions/runs/").nth(1)?;
    segment
        .split('/')
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

pub fn failed_run_ids(checks: &[PrCheck]) -> Vec<u64> {
    checks
        .iter()
        .filter(|check| {
            check.bucket.eq_ignore_ascii_case("fail")
                || check.state.eq_ignore_ascii_case("FAILURE")
                || check.state.eq_ignore_ascii_case("FAILED")
        })
        .filter_map(|check| check.link.as_deref().and_then(extract_run_id_from_link))
        .collect()
}

fn check_is_blocking(check: &PrCheck) -> bool {
    if check.bucket.eq_ignore_ascii_case("fail") {
        return true;
    }
    if check.state.eq_ignore_ascii_case("FAILURE") || check.state.eq_ignore_ascii_case("FAILED") {
        return true;
    }
    if check.bucket.eq_ignore_ascii_case("pending") {
        return true;
    }
    matches!(
        check.state.to_ascii_uppercase().as_str(),
        "IN_PROGRESS" | "PENDING" | "QUEUED" | "WAITING"
    )
}

/// CI check names that block babysitter finalize (failed or still running).
pub fn ci_checks_blocking(checks: &[PrCheck]) -> Vec<String> {
    checks
        .iter()
        .filter(|check| check_is_blocking(check))
        .map(|check| check.name.clone())
        .collect()
}

/// Unique non-empty file paths from inline PR review comments.
pub fn review_comment_paths(comments: &[PrReviewComment]) -> Vec<String> {
    let mut paths = Vec::new();
    for comment in comments {
        let Some(path) = comment.path.as_deref() else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_string());
        }
    }
    paths
}

pub fn gh_pr_state(pr_number: u64) -> Result<PrState, String> {
    let view_json = run_gh_capture(&[
        "pr",
        "view",
        &pr_number.to_string(),
        "--json",
        "number,title,state,mergeable,headRefName,baseRefName,url",
    ])?;
    let view: GhPrView =
        serde_json::from_str(&view_json).map_err(|err| format!("parse pr view json: {err}"))?;

    let checks_json = run_gh_capture(&[
        "pr",
        "checks",
        &pr_number.to_string(),
        "--json",
        "name,bucket,state,workflow,link",
    ])
    .unwrap_or_else(|_| "[]".to_string());
    let checks: Vec<PrCheck> = serde_json::from_str(&checks_json).unwrap_or_default();

    let comments_json = run_gh_capture(&[
        "api",
        &format!("repos/{{owner}}/{{repo}}/pulls/{pr_number}/comments"),
        "--paginate",
    ])
    .unwrap_or_else(|_| "[]".to_string());
    let review_comments: Vec<PrReviewComment> =
        serde_json::from_str(&comments_json).unwrap_or_default();

    Ok(PrState {
        number: view.number,
        title: view.title,
        state: view.state,
        mergeable: view.mergeable,
        head_ref_name: view.head_ref_name,
        base_ref_name: view.base_ref_name,
        url: view.url,
        checks,
        review_comments,
    })
}

pub fn format_pr_state_markdown(state: &PrState) -> String {
    let mut out = format!(
        "## PR #{} — {}\n\n- URL: {}\n- State: {}\n- Head: `{}` → base `{}`\n- Mergeable: {}\n\n### CI checks\n",
        state.number,
        state.title,
        state.url,
        state.state,
        state.head_ref_name,
        state.base_ref_name,
        state.mergeable.as_deref().unwrap_or("unknown"),
    );

    if state.checks.is_empty() {
        out.push_str("(no checks reported)\n");
    } else {
        for check in &state.checks {
            out.push_str(&format!(
                "- **{}** [{}] {} — {}\n",
                check.name,
                check.bucket,
                check.state,
                check.link.as_deref().unwrap_or("(no link)")
            ));
        }
    }

    let failed_runs = failed_run_ids(&state.checks);
    if !failed_runs.is_empty() {
        out.push_str("\n### Failed workflow run ids\n");
        for run_id in failed_runs {
            out.push_str(&format!("- gh-run:{run_id}\n"));
        }
    }

    if !state.review_comments.is_empty() {
        out.push_str(
            "\n### Actionable review comments (treat as FIXABLE_ACTION unless clearly nitpick)\n",
        );
        for comment in &state.review_comments {
            let path = comment.path.as_deref().unwrap_or("(general)");
            let loc = match comment.line {
                Some(line) => format!("{path}:{line}"),
                None => path.to_string(),
            };
            let body = comment.body.trim();
            let mut chars = body.chars();
            let preview: String = chars
                .by_ref()
                .take(300)
                .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
                .collect();
            let suffix = if chars.next().is_some() { "…" } else { "" };
            out.push_str(&format!("- `{loc}` — {preview}{suffix}\n"));
        }
    }

    out
}

pub fn gh_post_comment(pr_number: u64, body: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("babysitter-report-{pr_number}.md"));
    std::fs::write(&tmp, body).map_err(|err| format!("write comment body: {err}"))?;
    let path = tmp.to_string_lossy();
    run_gh_capture(&[
        "pr",
        "comment",
        &pr_number.to_string(),
        "--body-file",
        &path,
    ])?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

pub fn assert_on_pr_head_branch(expected_head_ref: &str) -> Result<(), String> {
    let repo_root = mcp_workspace_root();
    let output = Command::new("git")
        .current_dir(&repo_root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map_err(|err| format!("failed to run git: {err}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let current = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if current != expected_head_ref {
        return Err(format!(
            "workspace branch is `{current}` but PR head is `{expected_head_ref}` — checkout the PR branch before babysit_pr"
        ));
    }
    Ok(())
}

pub fn git_push_origin_head() -> Result<String, String> {
    let repo_root = mcp_workspace_root();
    let output = Command::new("git")
        .current_dir(&repo_root)
        .args(["push", "origin", "HEAD"])
        .output()
        .map_err(|err| format!("failed to run git push: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        return Ok(format!("{stdout}{stderr}").trim().to_string());
    }
    Err(format!("git push failed:\n{stdout}{stderr}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_review_comment_includes_line_and_preview() {
        let state = PrState {
            number: 27,
            title: "Hybrid planner".to_string(),
            state: "OPEN".to_string(),
            mergeable: Some("MERGEABLE".to_string()),
            head_ref_name: "feat/x".to_string(),
            base_ref_name: "main".to_string(),
            url: "https://example.com/pull/27".to_string(),
            checks: Vec::new(),
            review_comments: vec![PrReviewComment {
                path: Some("src/foo.rs".to_string()),
                line: Some(166),
                body: "fix this bug".repeat(50),
            }],
        };
        let md = format_pr_state_markdown(&state);
        assert!(md.contains("Actionable review comments"));
        assert!(md.contains("src/foo.rs:166"));
        assert!(md.contains('…'));
    }

    #[test]
    fn extract_run_id_from_link_parses_actions_url() {
        let link = "https://github.com/owner/repo/actions/runs/123456789/job/1";
        assert_eq!(extract_run_id_from_link(link), Some(123456789));
    }

    #[test]
    fn ci_checks_blocking_includes_fail_and_pending() {
        let checks = vec![
            PrCheck {
                name: "ok".into(),
                bucket: "pass".into(),
                state: "SUCCESS".into(),
                workflow: None,
                link: None,
            },
            PrCheck {
                name: "rust".into(),
                bucket: "fail".into(),
                state: "FAILURE".into(),
                workflow: None,
                link: None,
            },
            PrCheck {
                name: "lint".into(),
                bucket: "pending".into(),
                state: "IN_PROGRESS".into(),
                workflow: None,
                link: None,
            },
        ];
        let blocking = ci_checks_blocking(&checks);
        assert_eq!(blocking, vec!["rust".to_string(), "lint".to_string()]);
    }

    #[test]
    fn review_comment_paths_dedupes_file_paths() {
        let comments = vec![
            PrReviewComment {
                path: Some("src/a.rs".into()),
                line: Some(1),
                body: "a".into(),
            },
            PrReviewComment {
                path: Some("src/a.rs".into()),
                line: Some(2),
                body: "b".into(),
            },
            PrReviewComment {
                path: None,
                line: None,
                body: "general".into(),
            },
        ];
        assert_eq!(
            review_comment_paths(&comments),
            vec!["src/a.rs".to_string()]
        );
    }
}
