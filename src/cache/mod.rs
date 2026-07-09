pub mod embedding;
mod file_state;
pub mod inspect;
pub mod manager;
pub mod project;

pub use embedding::{LocalEmbeddingEngine, EMBEDDING_DIM};
pub use inspect::{
    load_cache_snapshot, list_evaluations, list_evaluations_page, AgentEvaluationRow,
    CacheSnapshot, EVALUATIONS_PAGE_SIZE, EvaluationsPage,
};
pub use manager::{ProjectCacheManager, SEMANTIC_SIMILARITY_THRESHOLD};
pub use project::{mcp_workspace_root, open_cache_connection, resolve_workspace_path};
