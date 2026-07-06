use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mcp_adjutant::{LocalEmbeddingEngine, ProjectCacheManager};

fn embedding_fixture_paths() -> (PathBuf, PathBuf) {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/embedding");
    (fixtures.join("model.onnx"), fixtures.join("tokenizer.json"))
}

fn unique_temp_project(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();

    std::env::temp_dir().join(format!("mcp-adjutant-semantic-{name}-{nanos}"))
}

fn write_demo_cargo_manifest(project_root: &Path) {
    fs::write(
        project_root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .expect("write cargo manifest");
}

fn open_cache_manager(project_root: &Path) -> ProjectCacheManager {
    let (model_path, tokenizer_path) = embedding_fixture_paths();
    ProjectCacheManager::new(project_root, &model_path, &tokenizer_path)
        .expect("initialize cache manager with embedding engine")
}

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
        similarity > 0.82,
        "expected semantic similarity above threshold, got {similarity}"
    );
}

#[test]
fn semantic_lookup_stays_under_twenty_milliseconds_after_warmup() {
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

    let _ = cache
        .try_get_valid_insight("JWT auth middleware configuration")
        .expect("warmup lookup");

    let start = Instant::now();
    let _ = cache
        .try_get_valid_insight("JWT auth middleware configuration")
        .expect("timed lookup");
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    assert!(
        elapsed_ms < 20.0,
        "semantic lookup should stay under 20 ms, took {elapsed_ms:.2} ms"
    );

    fs::remove_dir_all(&project_root).ok();
}
