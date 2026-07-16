use mcp_adjutant::cache::project::prepare_project_cache;
use std::fs;
use std::process::Command;

mod common;
use common::unique_temp_project;

#[test]
fn test_prepare_project_cache_creates_db_and_tables() {
    let project_root = unique_temp_project("cache-init");
    fs::create_dir_all(&project_root).expect("create project root");

    // Initialize git so find_project_root works (assuming it looks for .git)
    Command::new("git")
        .current_dir(&project_root)
        .args(["init"])
        .output()
        .expect("git init");

    let (root, conn) = prepare_project_cache(&project_root).expect("should prepare cache");

    // Canonicalize both paths to ensure comparison works regardless of /private/ prefix
    let canonical_root = fs::canonicalize(root).expect("canonicalize root");
    let canonical_project = fs::canonicalize(&project_root).expect("canonicalize project");

    assert_eq!(canonical_root, canonical_project);

    let db_path = project_root.join(".adjutant/cache.db");
    assert!(db_path.exists());

    // Verify tables exist
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='queries'")
        .unwrap();
    let exists: bool = stmt.exists([]).unwrap();
    assert!(exists, "queries table should exist");

    fs::remove_dir_all(&project_root).ok();
}
