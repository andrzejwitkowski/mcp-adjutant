mod common;

use std::path::Path;

use common::embedding_fixture_paths;
use mcp_adjutant::cache::embedding::LocalEmbeddingEngine;

#[test]
fn embedding_engine_loads_fixtures() {
    let (model_path, tokenizer_path) = embedding_fixture_paths();

    assert!(model_path.exists());
    assert!(tokenizer_path.exists());

    let engine = LocalEmbeddingEngine::new(&model_path, &tokenizer_path)
        .expect("failed to load embedding engine");

    let embedding = engine
        .generate("hello world")
        .expect("failed to generate embedding");

    assert_eq!(
        embedding.len(),
        mcp_adjutant::cache::embedding::EMBEDDING_DIM
    );
}
