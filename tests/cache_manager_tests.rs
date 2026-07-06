use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use mcp_adjutant::ProjectCacheManager;

fn unique_temp_project(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();

    std::env::temp_dir().join(format!("mcp-adjutant-integration-{name}-{nanos}"))
}

#[test]
fn cache_miss_for_unknown_query() {
    let project_root = unique_temp_project("cache-miss");
    fs::create_dir_all(&project_root).expect("create project root");
    fs::write(
        project_root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .expect("write cargo manifest");

    let cache = ProjectCacheManager::new(&project_root).expect("initialize cache");

    let result = cache
        .try_get_valid_insight("unknown query")
        .expect("lookup should succeed");

    assert!(result.is_none());

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn re_store_insight_refreshes_dependencies() {
    let project_root = unique_temp_project("re-store");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    fs::write(
        project_root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .expect("write cargo manifest");

    let first_file = project_root.join("src/a.rs");
    let second_file = project_root.join("src/b.rs");
    fs::write(&first_file, "fn a() {}\n").expect("write first file");
    fs::write(&second_file, "fn b() {}\n").expect("write second file");

    let mut cache = ProjectCacheManager::new(&project_root).expect("initialize cache");
    cache
        .store_insight(
            "dual dependency query",
            "## Insight\nFirst version.",
            vec![first_file.clone()],
        )
        .expect("store first insight");

    cache
        .store_insight(
            "dual dependency query",
            "## Insight\nSecond version.",
            vec![second_file.clone()],
        )
        .expect("store updated insight");

    let cached = cache
        .try_get_valid_insight("dual dependency query")
        .expect("lookup")
        .expect("cache hit");

    assert_eq!(cached, "## Insight\nSecond version.");

    fs::write(&first_file, "fn a() {}\n// stale\n").expect("modify stale dependency");

    let still_valid = cache
        .try_get_valid_insight("dual dependency query")
        .expect("lookup after stale file change")
        .expect("cache should remain valid");

    assert_eq!(still_valid, "## Insight\nSecond version.");

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn git_tracked_dependency_change_invalidates_cache() {
    let project_root = unique_temp_project("git-tracked");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    fs::write(
        project_root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .expect("write cargo manifest");

    Command::new("git")
        .current_dir(&project_root)
        .args(["init"])
        .output()
        .expect("git init");
    Command::new("git")
        .current_dir(&project_root)
        .args(["config", "user.email", "test@example.com"])
        .output()
        .expect("git config email");
    Command::new("git")
        .current_dir(&project_root)
        .args(["config", "user.name", "Cache Test"])
        .output()
        .expect("git config name");

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn tracked() {}\n").expect("write source");
    Command::new("git")
        .current_dir(&project_root)
        .args(["add", "."])
        .output()
        .expect("git add");

    let mut cache = ProjectCacheManager::new(&project_root).expect("initialize cache");
    cache
        .store_insight(
            "tracked query",
            "## Insight\nTracked dependency.",
            vec![source_file.clone()],
        )
        .expect("store insight");

    fs::write(&source_file, "pub fn tracked() {}\npub fn extra() {}\n").expect("modify source");

    let cached = cache
        .try_get_valid_insight("tracked query")
        .expect("lookup");
    assert!(
        cached.is_none(),
        "tracked file change should invalidate cache"
    );

    fs::remove_dir_all(&project_root).ok();
}
