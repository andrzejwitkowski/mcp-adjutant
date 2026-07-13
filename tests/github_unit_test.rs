mod common;

use mcp_adjutant::tools::{extract_run_id_from_link, failed_run_ids, PrCheck};

#[test]
fn test_extract_run_id_from_link() {
    assert_eq!(
        extract_run_id_from_link(
            "https://github.com/owner/repo/actions/runs/1234567890/job/123456789"
        ),
        Some(1234567890)
    );
    assert_eq!(
        extract_run_id_from_link("https://github.com/owner/repo/actions/runs/98765/"),
        Some(98765)
    );
    assert_eq!(extract_run_id_from_link("invalid-link"), None);
    assert_eq!(
        extract_run_id_from_link("https://github.com/owner/repo/actions/runs/abc/"),
        None
    );
    assert_eq!(
        extract_run_id_from_link("https://github.com/owner/repo/actions/runs/"),
        None
    );
}

#[test]
fn test_failed_run_ids() {
    let checks = vec![
        PrCheck {
            name: "build".to_string(),
            bucket: "success".to_string(),
            state: "SUCCESS".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/1".to_string()),
        },
        PrCheck {
            name: "test".to_string(),
            bucket: "fail".to_string(),
            state: "FAILURE".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/2".to_string()),
        },
        PrCheck {
            name: "lint".to_string(),
            bucket: "pending".to_string(),
            state: "PENDING".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/3".to_string()),
        },
        PrCheck {
            name: "deploy".to_string(),
            bucket: "success".to_string(),
            state: "FAILED".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/4".to_string()),
        },
        PrCheck {
            name: "no-link".to_string(),
            bucket: "fail".to_string(),
            state: "FAILURE".to_string(),
            workflow: None,
            link: None,
        },
        PrCheck {
            name: "invalid-link".to_string(),
            bucket: "fail".to_string(),
            state: "FAILURE".to_string(),
            workflow: None,
            link: Some("invalid-url".to_string()),
        },
    ];

    let failed = failed_run_ids(&checks);
    assert_eq!(failed, vec![2, 4]);
}
