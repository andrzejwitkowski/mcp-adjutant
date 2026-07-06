use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdjutantConfigError {
    #[error("failed to resolve home directory")]
    HomeDir,

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}
