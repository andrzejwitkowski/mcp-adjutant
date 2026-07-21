mod common;

use mcp_adjutant::agent::git_janitor::branch::BranchGate;
use mcp_adjutant::agent::git_janitor::conventions::GitConventions;
use mcp_adjutant::agent::git_janitor::scout::{format_scout_block, GitJanitorScout};

#[test]
fn format_scout_block_includes_sections() {
    let scout_data = GitJanitorScout {
        conventions: GitConventions::default(),
        conventions_from_disk: false,
        conventions_path: None,
        templates: vec![],
        recent_commits: "abc123 feat: add foo\n".into(),
        unstaged_diff: "--- a/foo\n+++ b/foo\n".into(),
        staged_diff: "--- a/bar\n+++ b/bar\n".into(),
        branch_gate: BranchGate::default(),
        suggested_adjutant_toml: "[git_rules]\n".into(),
    };
    let output = format_scout_block(&scout_data);
    assert!(output.contains("## Conventions"));
    assert!(output.contains("## Branch gate"));
    assert!(output.contains("## Recent commits"));
    assert!(output.contains("## Suggested .adjutant.toml"));
}
