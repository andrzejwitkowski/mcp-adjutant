use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::conventions::{extract_ticket, GitConventions};

const DEFAULT_BRANCHES: &[&str] = &["main", "master", "develop", "trunk"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BranchStatus {
    #[default]
    Ok,
    OnDefault,
    TicketMismatch,
    StaleFeature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BranchAction {
    #[default]
    None,
    CreateBranch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BranchGate {
    pub current_branch: String,
    pub branch_status: BranchStatus,
    pub action_required: BranchAction,
    pub commit_allowed: bool,
    pub suggested_branch_name: String,
    pub ticket_id: Option<String>,
}

pub fn evaluate_branch_gate(
    current_branch: &str,
    conventions: &GitConventions,
    feature_context: Option<&str>,
    expected_ticket: Option<&str>,
    user_instructions: Option<&str>,
) -> BranchGate {
    let regex = &conventions.git_rules.ticket_regex;
    let branch_ticket = extract_ticket(current_branch, regex);
    let context_ticket = feature_context
        .and_then(|t| extract_ticket(t, regex))
        .or_else(|| {
            expected_ticket
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| user_instructions.and_then(|t| extract_ticket(t, regex)));

    let on_default = DEFAULT_BRANCHES
        .iter()
        .any(|name| current_branch.eq_ignore_ascii_case(name));

    let (branch_status, action) = if on_default {
        (BranchStatus::OnDefault, BranchAction::CreateBranch)
    } else if let Some(expected) = expected_ticket.filter(|s| !s.is_empty()) {
        match &branch_ticket {
            Some(bt) if bt.eq_ignore_ascii_case(expected) => (BranchStatus::Ok, BranchAction::None),
            Some(_) => (BranchStatus::StaleFeature, BranchAction::CreateBranch),
            None => (BranchStatus::TicketMismatch, BranchAction::CreateBranch),
        }
    } else if let Some(ctx) = &context_ticket {
        match &branch_ticket {
            Some(bt) if bt.eq_ignore_ascii_case(ctx) => (BranchStatus::Ok, BranchAction::None),
            Some(_) => (BranchStatus::TicketMismatch, BranchAction::CreateBranch),
            None => (BranchStatus::TicketMismatch, BranchAction::CreateBranch),
        }
    } else {
        (BranchStatus::Ok, BranchAction::None)
    };

    let ticket_id = context_ticket
        .clone()
        .or(expected_ticket
            .map(str::to_string)
            .filter(|s| !s.is_empty()))
        .or(branch_ticket.clone());

    let suggested =
        suggest_branch_name(ticket_id.as_deref(), feature_context.or(user_instructions));

    BranchGate {
        current_branch: current_branch.to_string(),
        branch_status,
        action_required: action,
        commit_allowed: action == BranchAction::None,
        suggested_branch_name: suggested,
        ticket_id,
    }
}

pub fn suggest_branch_name(ticket: Option<&str>, context: Option<&str>) -> String {
    let slug = context
        .map(slugify)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "work".into());
    match ticket {
        Some(t) if !t.is_empty() => format!("feat/{t}-{slug}"),
        _ => format!("feat/{slug}"),
    }
}

fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true;
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash && out.len() < 40 {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

pub async fn git_current_branch(root: &Path) -> Result<String, String> {
    git_stdout(root, &["branch", "--show-current"]).await
}

pub async fn create_git_branch(root: &Path, branch_name: &str) -> Result<String, String> {
    let name = branch_name.trim();
    if name.is_empty() {
        return Err("branch_name must not be empty".into());
    }
    if name.contains("..") || name.contains('\0') {
        return Err("invalid branch_name".into());
    }
    let current = git_current_branch(root).await.unwrap_or_default();
    if current == name {
        return Err(format!("already on branch `{name}`"));
    }
    let output = Command::new("git")
        .current_dir(root)
        .args(["checkout", "-b", name])
        .output()
        .await
        .map_err(|err| format!("git checkout -b failed to start: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!("git checkout -b {name} failed:\n{stdout}{stderr}"));
    }
    Ok(format!(
        "{{\"branch\":\"{name}\",\"status\":\"created\",\"previous\":\"{current}\"}}"
    ))
}

pub async fn git_stdout(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .await
        .map_err(|err| format!("git {} failed to start: {err}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "git {} failed:\n{stdout}\n{stderr}",
            args.join(" ")
        ));
    }
    Ok(stdout)
}
