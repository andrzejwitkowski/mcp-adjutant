mod common;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use rusqlite::{params, Connection};
use std::fs;

#[test]
fn store_evaluation_persists_data() {
    let project_root = unique_temp_project("eval-persist");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let mut cache = open_cache_manager(&project_root);

    let agent_name = "TestAgent";
    let original_task = "Test task description";
    let agent_output = "Test agent output";
    let score = 10;
    let feedback_notes = "Test feedback notes";

    cache
        .store_evaluation(
            agent_name,
            original_task,
            agent_output,
            score,
            feedback_notes,
        )
        .expect("store evaluation");

    let db_path = project_root.join(".adjutant/cache.db");
    let conn = Connection::open(&db_path).expect("open cache.db");
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_evaluations WHERE agent_name = ?1",
            params![agent_name],
            |row| row.get(0),
        )
        .expect("count evaluations");
    assert_eq!(count, 1, "evaluation should be persisted");

    let stored_evaluation: (String, String, String, i32, String) = conn
        .query_row(
            "SELECT agent_name, original_task, agent_output, score, feedback_notes FROM agent_evaluations WHERE agent_name = ?1",
            params![agent_name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .expect("retrieve stored evaluation");

    assert_eq!(stored_evaluation.0, agent_name);
    assert_eq!(stored_evaluation.1, original_task);
    assert_eq!(stored_evaluation.2, agent_output);
    assert_eq!(stored_evaluation.3, score);
    assert_eq!(stored_evaluation.4, feedback_notes);

    fs::remove_dir_all(&project_root).ok();
}
