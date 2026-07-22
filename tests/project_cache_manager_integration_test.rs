mod common;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use std::fs;

#[test]
fn prepare_project_cache_creates_external_db_not_in_repo() {
    let project_root = unique_temp_project("prepare-project-cache");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let _cache = open_cache_manager(&project_root);

    let db_path = mcp_adjutant::cache::project_cache_db_path(&project_root).expect("db path");
    assert!(
        db_path.is_file(),
        "external cache db should exist at {}",
        db_path.display()
    );
    assert!(
        !project_root.join(".adjutant").is_dir(),
        "cache must not create in-repo .adjutant/"
    );

    fs::remove_dir_all(&project_root).ok();
}
