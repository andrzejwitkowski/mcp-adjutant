mod common;

use mcp_adjutant::agent::git_janitor::conventions::{extract_ticket, find_jira_ticket};

#[test]
fn find_jira_ticket_and_extract() {
    assert_eq!(find_jira_ticket("ABC-123"), Some("ABC-123".into()));
    assert_eq!(
        extract_ticket("Fix DEF-456", r"([A-Z]+-\d+)"),
        Some("DEF-456".into())
    );
    assert_eq!(find_jira_ticket("no ticket here"), None);
}
