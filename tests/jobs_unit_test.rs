use mcp_adjutant::jobs::JobRegistry;

#[test]
fn test_job_lifecycle() {
    let registry = JobRegistry::new();
    let uuid = "test-uuid";

    // Register
    registry
        .register(uuid, "test-tool")
        .expect("should register");

    // Query initial
    let status = registry.query(uuid).expect("should query");
    assert_eq!(status["status"], "queued");

    // Set running
    registry.set_running(uuid);
    let status = registry.query(uuid).expect("should query");
    assert_eq!(status["status"], "running");

    // Complete
    registry.complete(uuid, "done".to_string());
    let status = registry.query(uuid).expect("should query");
    assert_eq!(status["status"], "completed");
    assert_eq!(status["result"], "done");
    assert!(status["terminal"].as_bool().unwrap());
}
