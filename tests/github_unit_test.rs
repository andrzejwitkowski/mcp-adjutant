#[cfg(test)]
mod tests {
    use mcp_adjutant::tools::{
        ci_checks_blocking, extract_run_id_from_link, failed_run_ids, PrCheck,
    };

    #[test]
    fn test_extract_run_id_from_link() {
        let link = "https://github.com/owner/repo/actions/runs/123456789/job/987654321";
        assert_eq!(extract_run_id_from_link(link), Some(123456789));

        let invalid = "https://github.com/owner/repo/pull/1";
        assert_eq!(extract_run_id_from_link(invalid), None);
    }

    #[test]
    fn test_ci_checks_blocking() {
        let checks = vec![
            PrCheck {
                name: "test-pass".to_string(),
                bucket: "pass".to_string(),
                state: "SUCCESS".to_string(),
                workflow: None,
                link: None,
            },
            PrCheck {
                name: "test-fail".to_string(),
                bucket: "fail".to_string(),
                state: "FAILURE".to_string(),
                workflow: None,
                link: None,
            },
            PrCheck {
                name: "test-pending".to_string(),
                bucket: "pending".to_string(),
                state: "PENDING".to_string(),
                workflow: None,
                link: None,
            },
        ];

        let blocking = ci_checks_blocking(&checks);
        assert_eq!(blocking, vec!["test-fail", "test-pending"]);
    }

    #[test]
    fn test_failed_run_ids() {
        let checks = vec![PrCheck {
            name: "test-fail".to_string(),
            bucket: "fail".to_string(),
            state: "FAILURE".to_string(),
            workflow: None,
            link: Some("https://github.com/owner/repo/actions/runs/123".to_string()),
        }];

        assert_eq!(failed_run_ids(&checks), vec![123]);
    }
}
