mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use mcp_adjutant::domain::AgentPhase;
use mcp_adjutant::metrics::MetricsStore;
use rusqlite::params;

fn init_git_repo(project_root: &Path) {
    Command::new("git")
        .current_dir(project_root)
        .args(["init"])
        .output()
        .expect("git init");

    Command::new("git")
        .current_dir(project_root)
        .args(["config", "user.email", "test@example.com"])
        .output()
        .expect("git config email");

    Command::new("git")
        .current_dir(project_root)
        .args(["config", "user.name", "Cache Test"])
        .output()
        .expect("git config name");
}

#[test]
fn metrics_store_persists_cache_hit_in_project_database() {
    let project_root = unique_temp_project("metrics-cache-hit");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);
    init_git_repo(&project_root);

    let _cache = open_cache_manager(&project_root);
    let database_path = project_root.join(".adjutant/cache.db");
    let store = MetricsStore::open(&database_path).expect("open metrics store");

    store
        .record_cache_hit(AgentPhase::Scout)
        .expect("record cache hit");

    let count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM cache_hits WHERE agent_phase = ?1",
            params!["scout"],
            |row| row.get(0),
        )
        .expect("query cache hit");

    assert_eq!(count, 1);

    fs::remove_dir_all(&project_root).ok();
}
