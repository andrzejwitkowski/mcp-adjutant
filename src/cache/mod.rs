pub mod embedding;
mod file_state;
pub mod inspect;
pub mod manager;
pub mod project;

pub use embedding::{LocalEmbeddingEngine, EMBEDDING_DIM};
pub use inspect::{
    list_evaluations, list_evaluations_page, load_cache_snapshot, AgentEvaluationRow,
    CacheSnapshot, EvaluationsPage, EVALUATIONS_PAGE_SIZE,
};
pub use manager::{ProjectCacheManager, SEMANTIC_SIMILARITY_THRESHOLD};
pub use project::{mcp_workspace_root, open_cache_connection, resolve_workspace_path};
