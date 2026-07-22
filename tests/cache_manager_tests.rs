use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

mod common;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use rusqlite::{params, Connection};

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
fn cache_miss_for_unknown_query() {
    let project_root = unique_temp_project("cache-miss");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let cache = open_cache_manager(&project_root);

    let result = cache
        .try_get_valid_insight("unknown query")
        .expect("lookup should succeed");

    assert!(result.is_none());

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn finds_project_root_from_nested_directory() {
    let project_root = unique_temp_project("root-detect");
    fs::create_dir_all(project_root.join("src/nested")).expect("create nested dirs");
    write_demo_cargo_manifest(&project_root);

    let cache = open_cache_manager(&project_root.join("src/nested"));

    assert_eq!(
        cache.project_root(),
        fs::canonicalize(&project_root).unwrap()
    );
    assert!(project_root.join(".adjutant").is_dir());

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn appends_adjutant_directory_to_existing_gitignore() {
    let project_root = unique_temp_project("gitignore");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);
    fs::write(project_root.join(".gitignore"), "target/\n").expect("gitignore");

    open_cache_manager(&project_root);

    let gitignore = fs::read_to_string(project_root.join(".gitignore")).expect("read gitignore");
    assert!(gitignore.contains(".adjutant/"));

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn store_and_retrieve_valid_insight() {
    let project_root = unique_temp_project("cache-hit");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "how does hello work?",
            "## Insight\nCalls `hello` at lib.rs:1.",
            vec![source_file.clone()],
        )
        .expect("store insight");

    let cached = cache
        .try_get_valid_insight("how does hello work?")
        .expect("lookup")
        .expect("cache hit");

    assert_eq!(cached, "## Insight\nCalls `hello` at lib.rs:1.");

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn modified_file_invalidates_cached_insight() {
    let project_root = unique_temp_project("cache-invalidate");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "explain hello",
            "## Insight\nOriginal.",
            vec![source_file.clone()],
        )
        .expect("store insight");

    std::thread::sleep(Duration::from_millis(1100));
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&source_file)
        .expect("open source");
    writeln!(file, "// changed").expect("modify source");

    let cached = cache
        .try_get_valid_insight("explain hello")
        .expect("lookup");
    assert!(cached.is_none(), "modified file should invalidate cache");

    let retry = cache
        .try_get_valid_insight("explain hello")
        .expect("lookup after invalidation");
    assert!(retry.is_none(), "invalidated insight should be deleted");

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn re_store_insight_refreshes_dependencies() {
    let project_root = unique_temp_project("re-store");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let first_file = project_root.join("src/a.rs");
    let second_file = project_root.join("src/b.rs");
    fs::write(&first_file, "fn a() {}\n").expect("write first file");
    fs::write(&second_file, "fn b() {}\n").expect("write second file");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "dual dependency query",
            "## Insight\nFirst version at a.rs:1.",
            vec![first_file.clone()],
        )
        .expect("store first insight");

    cache
        .store_insight(
            "dual dependency query",
            "## Insight\nSecond version at b.rs:1.",
            vec![second_file.clone()],
        )
        .expect("store updated insight");

    let cached = cache
        .try_get_valid_insight("dual dependency query")
        .expect("lookup")
        .expect("cache hit");

    assert_eq!(cached, "## Insight\nSecond version at b.rs:1.");

    fs::write(&first_file, "fn a() {}\n// stale\n").expect("modify stale dependency");

    let still_valid = cache
        .try_get_valid_insight("dual dependency query")
        .expect("lookup after stale file change")
        .expect("cache should remain valid");

    assert_eq!(still_valid, "## Insight\nSecond version at b.rs:1.");

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn git_content_change_invalidates_cached_insight() {
    let project_root = unique_temp_project("git-invalidate");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);
    init_git_repo(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

    Command::new("git")
        .current_dir(&project_root)
        .args(["add", "."])
        .output()
        .expect("git add");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "git tracked insight",
            "## Insight\nTracked.",
            vec![source_file.clone()],
        )
        .expect("store insight");

    fs::write(&source_file, "pub fn hello() {}\npub fn world() {}\n").expect("rewrite source");

    let cached = cache
        .try_get_valid_insight("git tracked insight")
        .expect("lookup");
    assert!(cached.is_none(), "git blob change should invalidate cache");

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn git_tracking_after_store_invalidates_cached_insight() {
    let project_root = unique_temp_project("git-tracked-after-store");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn tracked() {}\n").expect("write source");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "pre-git query",
            "## Insight\nBefore git.",
            vec![source_file.clone()],
        )
        .expect("store insight");

    init_git_repo(&project_root);
    Command::new("git")
        .current_dir(&project_root)
        .args(["add", "."])
        .output()
        .expect("git add");

    let cached = cache
        .try_get_valid_insight("pre-git query")
        .expect("lookup");
    assert!(
        cached.is_none(),
        "new git blob identity should invalidate a snapshot stored without git sha"
    );

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn git_tracked_dependency_change_invalidates_cache() {
    let project_root = unique_temp_project("git-tracked");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);
    init_git_repo(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn tracked() {}\n").expect("write source");
    Command::new("git")
        .current_dir(&project_root)
        .args(["add", "."])
        .output()
        .expect("git add");

    let mut cache = open_cache_manager(&project_root);
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

#[test]
fn store_evaluation_allows_duplicate_scores_in_same_second() {
    let project_root = unique_temp_project("eval-dup");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let mut cache = open_cache_manager(&project_root);

    cache
        .store_evaluation(
            "Phase_1_Scout",
            "same task",
            "identical output",
            6,
            "identical critique",
            "exemplar",
        )
        .expect("first evaluation");
    cache
        .store_evaluation(
            "Phase_1_Scout",
            "same task",
            "identical output",
            6,
            "identical critique",
            "exemplar",
        )
        .expect("second evaluation with same score");

    let db_path = project_root.join(".adjutant/cache.db");
    let conn = Connection::open(&db_path).expect("open cache.db");
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_evaluations WHERE agent_name = ?1",
            params!["Phase_1_Scout"],
            |row| row.get(0),
        )
        .expect("count evaluations");
    assert_eq!(count, 2, "both evaluations should be persisted");

    fs::remove_dir_all(&project_root).ok();
}
