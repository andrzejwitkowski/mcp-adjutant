mod common;

use mcp_adjutant::tools::{extract_run_id_from_link, failed_run_ids, PrCheck};

#[test]
fn extract_run_id_from_link_valid_url() {
    let link = "https://github.com/owner/repo/actions/runs/1234567890/job/123";
    let run_id = extract_run_id_from_link(link);
    assert_eq!(run_id, Some(1234567890));
}

#[test]
fn extract_run_id_from_link_no_run_id() {
    let link = "https://github.com/owner/repo/actions/workflows/ci.yml";
    let run_id = extract_run_id_from_link(link);
    assert_eq!(run_id, None);
}

#[test]
fn extract_run_id_from_link_empty_string() {
    let link = "";
    let run_id = extract_run_id_from_link(link);
    assert_eq!(run_id, None);
}

#[test]
fn failed_run_ids_empty_checks() {
    let checks: Vec<PrCheck> = vec![];
    let run_ids = failed_run_ids(&checks);
    assert!(run_ids.is_empty());
}

#[test]
fn failed_run_ids_no_failed_checks() {
    let checks = vec![
        PrCheck {
            name: "build".to_string(),
            bucket: "success".to_string(),
            state: "SUCCESS".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/111".to_string()),
        },
        PrCheck {
            name: "test".to_string(),
            bucket: "success".to_string(),
            state: "SUCCESS".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/222".to_string()),
        },
    ];
    let run_ids = failed_run_ids(&checks);
    assert!(run_ids.is_empty());
}

#[test]
fn failed_run_ids_with_failed_checks() {
    let checks = vec![
        PrCheck {
            name: "build".to_string(),
            bucket: "fail".to_string(),
            state: "FAILURE".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/123".to_string()),
        },
        PrCheck {
            name: "test".to_string(),
            bucket: "success".to_string(),
            state: "SUCCESS".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/456".to_string()),
        },
        PrCheck {
            name: "lint".to_string(),
            bucket: "fail".to_string(),
            state: "FAILED".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/789".to_string()),
        },
    ];
    let run_ids = failed_run_ids(&checks);
    assert_eq!(run_ids, vec![123, 789]);
}

#[test]
fn failed_run_ids_with_failed_checks_and_no_link() {
    let checks = vec![
        PrCheck {
            name: "build".to_string(),
            bucket: "fail".to_string(),
            state: "FAILURE".to_string(),
            workflow: None,
            link: None,
        },
        PrCheck {
            name: "test".to_string(),
            bucket: "success".to_string(),
            state: "SUCCESS".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/456".to_string()),
        },
    ];
    let run_ids = failed_run_ids(&checks);
    assert!(run_ids.is_empty());
}