mod common;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use std::fs;
use std::path::Path;

#[test]
fn prepare_project_cache_creates_adjutant_dir_and_db() {
    let project_root = unique_temp_project("prepare-project-cache");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let _cache = open_cache_manager(&project_root);

    let adjutant_dir = project_root.join(".adjutant");
    assert!(adjutant_dir.is_dir());

    let db_path = adjutant_dir.join("cache.db");
    assert!(db_path.is_file());

    fs::remove_dir_all(&project_root).ok();
}