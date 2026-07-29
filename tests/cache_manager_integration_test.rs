mod common;

use mcp_adjutant::metrics::MetricsStore;
use rusqlite::params;
use std::fs;

#[test]
fn store_evaluation_persists_data() {
    let dir = std::env::temp_dir().join(format!(
        "eval-persist-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("tmpdir");
    let store = MetricsStore::open(&dir.join("metrics.db")).expect("open");

    let agent_name = "TestAgent";
    let original_task = "Test task description";
    let agent_output = "Test agent output";
    let score = 10;
    let feedback_notes = "Test feedback notes";

    store
        .store_evaluation(
            agent_name,
            original_task,
            agent_output,
            score,
            feedback_notes,
            "",
        )
        .expect("store evaluation");

    let count: i32 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM agent_evaluations WHERE agent_name = ?1",
            params![agent_name],
            |row| row.get(0),
        )
        .expect("count evaluations");
    assert_eq!(count, 1, "evaluation should be persisted");

    let stored: (String, String, String, i32, String, String) = store
        .connection()
        .query_row(
            "SELECT agent_name, original_task, agent_output, score, feedback_notes, desired_output
             FROM agent_evaluations WHERE agent_name = ?1",
            params![agent_name],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("retrieve");

    assert_eq!(stored.0, agent_name);
    assert_eq!(stored.1, original_task);
    assert_eq!(stored.2, agent_output);
    assert_eq!(stored.3, score);
    assert_eq!(stored.4, feedback_notes);
    assert_eq!(stored.5, "");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn store_evaluation_normalizes_builder_alias() {
    let dir = std::env::temp_dir().join(format!(
        "eval-norm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("tmpdir");
    let store = MetricsStore::open(&dir.join("metrics.db")).expect("open");
    store
        .store_evaluation("builder", "task", "output", 7, "ok", "exemplar")
        .expect("store");

    let agent_name: String = store
        .connection()
        .query_row(
            "SELECT agent_name FROM agent_evaluations LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("read");

    assert_eq!(agent_name, "Phase_4_Builder");
    let _ = fs::remove_dir_all(&dir);
}
