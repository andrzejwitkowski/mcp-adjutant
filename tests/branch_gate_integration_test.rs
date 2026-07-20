mod common;

use mcp_adjutant::agent::git_janitor::branch::{
    evaluate_branch_gate, suggest_branch_name, BranchAction, BranchStatus,
};
use mcp_adjutant::agent::git_janitor::conventions::GitConventions;

#[test]
fn test_evaluate_branch_gate_on_default_branch_main() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate("main", &conventions, None, None, None);

    assert_eq!(gate.branch_status, BranchStatus::OnDefault);
    assert_eq!(gate.action_required, BranchAction::CreateBranch);
    assert!(!gate.commit_allowed);
    assert_eq!(gate.current_branch, "main");
}

#[test]
fn test_evaluate_branch_gate_on_default_branch_master() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate("master", &conventions, None, None, None);

    assert_eq!(gate.branch_status, BranchStatus::OnDefault);
    assert_eq!(gate.action_required, BranchAction::CreateBranch);
    assert!(!gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_on_default_branch_develop() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate("develop", &conventions, None, None, None);

    assert_eq!(gate.branch_status, BranchStatus::OnDefault);
    assert_eq!(gate.action_required, BranchAction::CreateBranch);
    assert!(!gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_on_default_branch_trunk() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate("trunk", &conventions, None, None, None);

    assert_eq!(gate.branch_status, BranchStatus::OnDefault);
    assert_eq!(gate.action_required, BranchAction::CreateBranch);
    assert!(!gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_case_insensitive_default() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate("MAIN", &conventions, None, None, None);

    assert_eq!(gate.branch_status, BranchStatus::OnDefault);
    assert_eq!(gate.action_required, BranchAction::CreateBranch);
    assert!(!gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_matching_ticket() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate(
        "feat/JIRA-123-some-feature",
        &conventions,
        None,
        Some("JIRA-123"),
        None,
    );

    assert_eq!(gate.branch_status, BranchStatus::Ok);
    assert_eq!(gate.action_required, BranchAction::None);
    assert!(gate.commit_allowed);
    assert_eq!(gate.ticket_id, Some("JIRA-123".to_string()));
}

#[test]
fn test_evaluate_branch_gate_ticket_mismatch() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate(
        "feat/JIRA-456-other-feature",
        &conventions,
        None,
        Some("JIRA-123"),
        None,
    );

    assert_eq!(gate.branch_status, BranchStatus::StaleFeature);
    assert_eq!(gate.action_required, BranchAction::CreateBranch);
    assert!(!gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_no_ticket_in_branch() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate(
        "feat/some-feature",
        &conventions,
        None,
        Some("JIRA-123"),
        None,
    );

    assert_eq!(gate.branch_status, BranchStatus::TicketMismatch);
    assert_eq!(gate.action_required, BranchAction::CreateBranch);
    assert!(!gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_with_feature_context() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate(
        "feat/JIRA-789-context-feature",
        &conventions,
        Some("JIRA-789 context feature"),
        None,
        None,
    );

    assert_eq!(gate.branch_status, BranchStatus::Ok);
    assert_eq!(gate.action_required, BranchAction::None);
    assert!(gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_with_user_instructions() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate(
        "feat/JIRA-999-instructions-feature",
        &conventions,
        None,
        None,
        Some("Work on JIRA-999 instructions feature"),
    );

    assert_eq!(gate.branch_status, BranchStatus::Ok);
    assert_eq!(gate.action_required, BranchAction::None);
    assert!(gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_expected_ticket_takes_precedence() {
    let conventions = GitConventions::default();
    // expected_ticket should be used when provided
    let gate = evaluate_branch_gate(
        "feat/JIRA-111",
        &conventions,
        Some("JIRA-222 different context"),
        Some("JIRA-111"),
        None,
    );

    assert_eq!(gate.branch_status, BranchStatus::Ok);
    assert_eq!(gate.action_required, BranchAction::None);
    assert!(gate.commit_allowed);
}

#[test]
fn test_evaluate_branch_gate_ticket_id_extraction() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate("feat/JIRA-42-my-task", &conventions, None, None, None);

    // When no expected_ticket or context, ticket_id should come from branch
    assert_eq!(gate.ticket_id, Some("JIRA-42".to_string()));
}

#[test]
fn test_suggest_branch_name_with_ticket() {
    let result = suggest_branch_name(Some("JIRA-123"), Some("my awesome feature"));
    assert_eq!(result, "feat/JIRA-123-my-awesome-feature");
}

#[test]
fn test_suggest_branch_name_without_ticket() {
    let result = suggest_branch_name(None, Some("my awesome feature"));
    assert_eq!(result, "feat/my-awesome-feature");
}

#[test]
fn test_suggest_branch_name_with_empty_ticket() {
    let result = suggest_branch_name(Some(""), Some("my awesome feature"));
    assert_eq!(result, "feat/my-awesome-feature");
}

#[test]
fn test_suggest_branch_name_without_context() {
    let result = suggest_branch_name(Some("JIRA-456"), None);
    assert_eq!(result, "feat/JIRA-456-work");
}

#[test]
fn test_suggest_branch_name_without_anything() {
    let result = suggest_branch_name(None, None);
    assert_eq!(result, "feat/work");
}

#[test]
fn test_suggest_branch_name_slug_truncation() {
    let long_text = "a".repeat(100);
    let result = suggest_branch_name(Some("JIRA-1"), Some(&long_text));
    // Should be truncated at 40 chars for the slug part
    assert!(result.len() < 100);
    assert!(result.starts_with("feat/JIRA-1-"));
}

#[test]
fn test_suggest_branch_name_special_chars() {
    let result = suggest_branch_name(Some("JIRA-1"), Some("Hello World! @#$%"));
    assert_eq!(result, "feat/JIRA-1-hello-world");
}

#[test]
fn test_evaluate_branch_gate_empty_expected_ticket_ignored() {
    let conventions = GitConventions::default();
    let gate = evaluate_branch_gate("feat/JIRA-123", &conventions, None, Some(""), None);

    // Empty expected_ticket should be ignored, falls through to context_ticket check
    // Since no context_ticket either, should return Ok with no action
    assert_eq!(gate.branch_status, BranchStatus::Ok);
    assert_eq!(gate.action_required, BranchAction::None);
    assert!(gate.commit_allowed);
}
