//! Shared helpers for integration tests; not every test binary uses every helper.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mcp_adjutant::ProjectCacheManager;

pub fn embedding_fixture_paths() -> (PathBuf, PathBuf) {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/embedding");
    (fixtures.join("model.onnx"), fixtures.join("tokenizer.json"))
}

pub fn unique_temp_project(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();

    std::env::temp_dir().join(format!("mcp-adjutant-{name}-{nanos}"))
}

pub fn write_demo_cargo_manifest(project_root: &Path) {
    fs::write(
        project_root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .expect("write cargo manifest");
}

pub fn open_cache_manager(project_root: &Path) -> ProjectCacheManager {
    let (model_path, tokenizer_path) = embedding_fixture_paths();
    ProjectCacheManager::new(project_root, &model_path, &tokenizer_path)
        .expect("initialize cache manager with embedding engine")
}
