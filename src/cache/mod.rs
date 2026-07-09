pub mod embedding;
mod file_state;
pub mod manager;
pub mod project;

pub use embedding::{LocalEmbeddingEngine, EMBEDDING_DIM};
pub use manager::{ProjectCacheManager, SEMANTIC_SIMILARITY_THRESHOLD};
pub use project::{mcp_workspace_root, resolve_workspace_path};
