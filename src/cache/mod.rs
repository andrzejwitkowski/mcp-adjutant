pub mod embedding;
mod file_state;
pub mod inspect;
pub mod manager;
pub mod project;

pub use embedding::{LocalEmbeddingEngine, EMBEDDING_DIM};
pub use inspect::{
    list_evaluations, list_evaluations_page, load_cache_snapshot, load_scout_cache_page,
    load_web_cache_page, AgentEvaluationRow, CacheSnapshot, EvaluationsPage, ScoutCachePage,
    WebCachePage, WebFetchDependencyRow, WebQueryRow, WebReportRow, WebSourceRow,
    EVALUATIONS_PAGE_SIZE,
};
pub use manager::{
    ProjectCacheManager, WebReportCacheLookup, WebReportRevalidation, WebSourceSnapshot,
    SEMANTIC_SIMILARITY_THRESHOLD,
};
pub use project::{
    current_unix_timestamp, hash_query_text, mcp_workspace_root, open_cache_connection,
    resolve_workspace_path, resolve_workspace_path_bounded,
};
