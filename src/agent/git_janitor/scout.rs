use std::path::{Path, PathBuf};

use serde::Serialize;

use super::branch::{evaluate_branch_gate, git_current_branch, git_stdout, BranchGate};
use super::conventions::{
    conventions_toml_string, load_conventions, GitConventions, ADJUTANT_TOML,
};

const TEMPLATE_CANDIDATES: &[&str] = &[
    ".github/PULL_REQUEST_TEMPLATE.md",
    ".gitlab/merge_request_templates/default.md",
    ".commitlintrc",
    ".commitlintrc.json",
    ".commitlintrc.js",
    "commitlint.config.js",
    "CONTRIBUTING.md",
];

#[derive(Debug, Clone, Serialize)]
pub struct GitJanitorScout {
    pub conventions: GitConventions,
    pub conventions_from_disk: bool,
    pub conventions_path: Option<String>,
    pub templates: Vec<TemplateSnippet>,
    pub recent_commits: String,
    pub unstaged_diff: String,
    pub staged_diff: String,
    pub branch_gate: BranchGate,
    pub suggested_adjutant_toml: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateSnippet {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct ScoutInputs {
    pub feature_context: Option<String>,
    pub expected_ticket: Option<String>,
    pub user_instructions: Option<String>,
}

pub async fn gather_conventions_and_diff(
    root: &Path,
    inputs: &ScoutInputs,
) -> Result<GitJanitorScout, String> {
    let (conventions, path, from_disk) = load_conventions(root);
    let templates = load_templates(root, &conventions);
    let current_branch = git_current_branch(root)
        .await
        .unwrap_or_else(|_| "(unknown)".into());
    let recent_commits = git_stdout(root, &["log", "-n", "10", "--oneline"])
        .await
        .unwrap_or_default();
    let unstaged_diff = git_stdout(root, &["diff"])
        .await
        .unwrap_or_default();
    let staged_diff = git_stdout(root, &["diff", "--staged"])
        .await
        .unwrap_or_default();

    let branch_gate = evaluate_branch_gate(
        &current_branch,
        &conventions,
        inputs.feature_context.as_deref(),
        inputs.expected_ticket.as_deref(),
        inputs.user_instructions.as_deref(),
    );

    let suggested_adjutant_toml = conventions_toml_string(&conventions)?;

    Ok(GitJanitorScout {
        conventions,
        conventions_from_disk: from_disk,
        conventions_path: path.map(|p| display_rel(root, &p)),
        templates,
        recent_commits,
        unstaged_diff,
        staged_diff,
        branch_gate,
        suggested_adjutant_toml,
    })
}

fn load_templates(root: &Path, conventions: &GitConventions) -> Vec<TemplateSnippet> {
    let mut paths: Vec<PathBuf> = TEMPLATE_CANDIDATES
        .iter()
        .map(|p| root.join(p))
        .collect();
    let configured = root.join(&conventions.pr.template_file);
    if !paths.iter().any(|p| p == &configured) {
        paths.insert(0, configured);
    }
    let mut out = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let content = if raw.len() > 4_000 {
            format!("{}…\n(truncated)", &raw[..4_000])
        } else {
            raw
        };
        out.push(TemplateSnippet {
            path: display_rel(root, &path),
            content,
        });
    }
    out
}

fn display_rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn format_scout_block(scout: &GitJanitorScout) -> String {
    let mut buf = String::new();
    buf.push_str("## Conventions\n");
    if scout.conventions_from_disk {
        buf.push_str(&format!(
            "Loaded from {}\n",
            scout.conventions_path.as_deref().unwrap_or(ADJUTANT_TOML)
        ));
    } else {
        buf.push_str("No local file — using Conventional Commits defaults.\n");
    }
    buf.push_str(&format!(
        "style={} ticket_regex={} require_ticket={} pattern={}\n",
        scout.conventions.git_rules.commit_style,
        scout.conventions.git_rules.ticket_regex,
        scout.conventions.git_rules.require_ticket_in_commit,
        scout.conventions.commit_format.pattern
    ));
    buf.push_str("\n## Branch gate\n");
    buf.push_str(&serde_json::to_string_pretty(&scout.branch_gate).unwrap_or_default());
    buf.push_str("\n\n## Recent commits\n");
    buf.push_str(if scout.recent_commits.is_empty() {
        "(none)"
    } else {
        &scout.recent_commits
    });
    buf.push_str("\n\n## Staged diff\n");
    buf.push_str(truncate_diff(&scout.staged_diff));
    buf.push_str("\n\n## Unstaged diff\n");
    buf.push_str(truncate_diff(&scout.unstaged_diff));
    for tmpl in &scout.templates {
        buf.push_str(&format!("\n\n## Template {}\n{}", tmpl.path, tmpl.content));
    }
    buf.push_str("\n\n## Suggested .adjutant.toml\n```toml\n");
    buf.push_str(&scout.suggested_adjutant_toml);
    buf.push_str("\n```\n");
    buf
}

fn truncate_diff(diff: &str) -> &str {
    if diff.is_empty() {
        "(empty)"
    } else if diff.len() > 12_000 {
        // ponytail: hard truncate; raise if LLM misses hunks
        &diff[..12_000]
    } else {
        diff
    }
}
