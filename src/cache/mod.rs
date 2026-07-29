pub mod agent_names;
pub mod embedding;
mod file_state;
pub mod inspect;
pub mod manager;
pub mod project;

pub use agent_names::{backfill_evaluation_agent_names, normalize_agent_name};
pub use embedding::{LocalEmbeddingEngine, EMBEDDING_DIM};
pub use inspect::{
    is_dense_builder_report_exemplar, load_cache_snapshot, load_scout_cache_page,
    load_web_cache_page, CacheSnapshot, ScoutCachePage, WebCachePage, WebFetchDependencyRow,
    WebQueryRow, WebReportRow, WebSourceRow,
};
pub use manager::{
    ProjectCacheManager, WebReportCacheLookup, WebReportRevalidation, WebSourceSnapshot,
    SEMANTIC_SIMILARITY_THRESHOLD,
};
pub use project::{
    current_unix_timestamp, display_rel, hash_query_text, mcp_workspace_root,
    open_cache_connection, parse_workspace_root_arg, project_cache_db_path,
    require_workspace_root_arg, resolve_config_cache_root, resolve_workspace_path,
    resolve_workspace_path_bounded, with_thread_workspace_root, workspace_root_schema_property,
};
