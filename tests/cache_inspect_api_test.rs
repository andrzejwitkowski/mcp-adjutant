mod common;

use std::fs;
use std::thread;
use std::time::Duration;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use mcp_adjutant::cache::{
    list_evaluations, list_evaluations_page, load_cache_snapshot, open_cache_connection,
    EVALUATIONS_PAGE_SIZE,
};

#[test]
fn list_evaluations_returns_newest_first() {
    let project_root = unique_temp_project("inspect-eval");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_evaluation("Scout", "task one", "output one", 7, "ok")
        .expect("first evaluation");
    thread::sleep(Duration::from_millis(1100));
    cache
        .store_evaluation("Builder", "task two", "output two", 9, "great")
        .expect("second evaluation");

    let (_, conn) = open_cache_connection(&project_root).expect("open cache");
    let rows = list_evaluations(&conn).expect("list evaluations");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].agent_name, "Builder");
    assert_eq!(rows[0].score, 9);
    assert_eq!(rows[1].agent_name, "Scout");

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn cache_snapshot_reports_embeddings_and_dirty_nodes() {
    let project_root = unique_temp_project("inspect-cache");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let source_file = project_root.join("src/lib.rs");
    fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "how does hello work?",
            "## Insight\nCalls `hello`.",
            vec![source_file.clone()],
        )
        .expect("store insight");

    fs::write(&source_file, "pub fn hello() { println!(\"hi\"); }\n").expect("mutate source");

    let (_, conn) = open_cache_connection(&project_root).expect("open cache");
    let snapshot = load_cache_snapshot(&conn, &project_root).expect("load snapshot");

    assert_eq!(snapshot.overview.query_count, 1);
    assert_eq!(snapshot.overview.insight_count, 1);
    assert_eq!(snapshot.overview.embedding_count, 1);
    assert_eq!(snapshot.queries[0].has_embedding, true);
    assert_eq!(snapshot.code_nodes.len(), 1);
    assert!(snapshot.code_nodes[0].is_dirty);
    assert_eq!(snapshot.dependencies.len(), 1);

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn list_evaluations_page_returns_twenty_per_page_newest_first() {
    let project_root = unique_temp_project("inspect-eval-page");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let mut cache = open_cache_manager(&project_root);
    for index in 0..25 {
        cache
            .store_evaluation(
                "Scout",
                &format!("task {index}"),
                &format!("output {index}"),
                (index % 10) + 1,
                "ok",
            )
            .expect("store evaluation");
        thread::sleep(Duration::from_millis(10));
    }

    let (_, conn) = open_cache_connection(&project_root).expect("open cache");
    let page1 = list_evaluations_page(&conn, 1, EVALUATIONS_PAGE_SIZE).expect("page 1");
    let page2 = list_evaluations_page(&conn, 2, EVALUATIONS_PAGE_SIZE).expect("page 2");

    assert_eq!(page1.total_count, 25);
    assert_eq!(page1.total_pages, 2);
    assert_eq!(page1.items.len(), 20);
    assert_eq!(page2.items.len(), 5);
    assert!(page1.items[0].created_at >= page1.items[1].created_at);
    assert_eq!(page1.avg_score, Some(5.0));

    fs::remove_dir_all(&project_root).ok();
}
