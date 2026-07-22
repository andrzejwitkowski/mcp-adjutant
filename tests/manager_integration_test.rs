mod common;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use std::fs;

#[test]
fn prose_only_insight_invalidated_on_read() {
    let project_root = unique_temp_project("prose-quality-gate");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "how does hello work?",
            "## Insight\nCalls hello with no file line evidence.",
            vec![source_file],
        )
        .expect("store insight");

    let hit = cache
        .try_get_valid_insight("how does hello work?")
        .expect("lookup");
    assert!(hit.is_none(), "prose-only insight must miss quality gate");

    let retry = cache
        .try_get_valid_insight("how does hello work?")
        .expect("lookup after invalidation");
    assert!(
        retry.is_none(),
        "quality gate must delete the stale insight"
    );

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn insight_with_file_line_citation_hits() {
    let project_root = unique_temp_project("citation-quality-gate");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

    let insight = "## Insight\nCalls `hello` at lib.rs:1.";
    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight("how does hello work?", insight, vec![source_file])
        .expect("store insight");

    let hit = cache
        .try_get_valid_insight("how does hello work?")
        .expect("lookup")
        .expect("cited insight must hit");
    assert_eq!(hit, insight);

    fs::remove_dir_all(&project_root).ok();
}
