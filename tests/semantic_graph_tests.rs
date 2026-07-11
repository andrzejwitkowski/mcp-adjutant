use std::fs;
use std::io::Write;
use std::time::{Duration, Instant};

mod common;

use common::{
    embedding_fixture_paths, open_cache_manager, unique_temp_project, write_demo_cargo_manifest,
};
use mcp_adjutant::{LocalEmbeddingEngine, SEMANTIC_SIMILARITY_THRESHOLD};

#[test]
fn semantic_graph_matches_paraphrase_and_invalidates_on_file_change() {
    let project_root = unique_temp_project("semantic-graph");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let auth_file = project_root.join("src/auth.rs");
    fs::write(&auth_file, "pub fn jwt_routes() {}\n").expect("write auth source");

    let stored_query = "How to set up JWT authentication routing";
    let paraphrase_query = "JWT auth middleware configuration";
    let insight = "## Insight\nUse `jwt_routes` for JWT middleware.";

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(stored_query, insight, vec![auth_file.clone()])
        .expect("store insight");

    let cached = cache
        .try_get_valid_insight(paraphrase_query)
        .expect("semantic lookup")
        .expect("paraphrase should hit cached insight");

    assert_eq!(cached, insight);

    std::thread::sleep(Duration::from_millis(1100));
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&auth_file)
        .expect("open auth source");
    writeln!(file, "// changed").expect("modify auth source");

    let invalidated = cache
        .try_get_valid_insight(paraphrase_query)
        .expect("lookup after invalidation");
    assert!(
        invalidated.is_none(),
        "modified dependency should invalidate semantic cache"
    );

    let retry = cache
        .try_get_valid_insight(stored_query)
        .expect("lookup after deletion");
    assert!(
        retry.is_none(),
        "invalidated semantic subgraph should be deleted from SQLite"
    );

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn embedding_engine_reports_high_similarity_for_jwt_paraphrases() {
    let (model_path, tokenizer_path) = embedding_fixture_paths();
    let engine =
        LocalEmbeddingEngine::new(&model_path, &tokenizer_path).expect("load embedding engine");

    let left = engine
        .generate("How to set up JWT authentication routing")
        .expect("embed stored query");
    let right = engine
        .generate("JWT auth middleware configuration")
        .expect("embed paraphrase");

    let similarity = LocalEmbeddingEngine::dot_product(&left, &right);
    assert!(
        similarity > SEMANTIC_SIMILARITY_THRESHOLD - 0.02,
        "expected semantic similarity near threshold, got {similarity}"
    );
}

#[test]
fn semantic_lookup_stays_under_budget_after_warmup() {
    let project_root = unique_temp_project("semantic-bench");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let auth_file = project_root.join("src/auth.rs");
    fs::write(&auth_file, "pub fn jwt_routes() {}\n").expect("write auth source");

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(
            "How to set up JWT authentication routing",
            "insight",
            vec![auth_file],
        )
        .expect("store insight");

    let query = "JWT auth middleware configuration";
    let _ = cache.try_get_valid_insight(query).expect("warmup lookup");

    let mut samples_ms = Vec::with_capacity(5);
    for _ in 0..5 {
        let start = Instant::now();
        let _ = cache.try_get_valid_insight(query).expect("timed lookup");
        samples_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    samples_ms.sort_by(|left, right| left.partial_cmp(right).expect("finite durations"));
    let median_ms = samples_ms[2];
    // ponytail: 20ms on dev hardware; GHA median ~35ms — 50ms catches real regressions
    let budget_ms = if std::env::var("CI").is_ok() { 50.0 } else { 20.0 };

    assert!(
        median_ms < budget_ms,
        "semantic lookup median should stay under {budget_ms} ms, got {median_ms:.2} ms (samples: {samples_ms:?})"
    );

    fs::remove_dir_all(&project_root).ok();
}
