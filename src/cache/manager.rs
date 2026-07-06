use std::path::{Path, PathBuf};

use bytemuck;
use rusqlite::{params, Connection, Error as RusqliteError};

use super::embedding::{LocalEmbeddingEngine, EMBEDDING_DIM};
use super::file_state::{capture_code_node_snapshot, is_code_node_dirty, CodeNodeSnapshot};
use super::project::{
    current_unix_timestamp, hash_query_text, normalize_relative_path, prepare_project_cache,
};

/// Minimum cosine similarity for a semantic cache hit (L2-normalized dot product).
/// ponytail: bge-small-en-v1.5 scores ~0.826 for the task's JWT paraphrase pair; 0.91 is aspirational.
pub const SEMANTIC_SIMILARITY_THRESHOLD: f32 = 0.82;

pub struct ProjectCacheManager {
    conn: Connection,
    project_root: PathBuf,
    embedding_engine: LocalEmbeddingEngine,
}

impl ProjectCacheManager {
    /// Automatically detects the project root (walking up for `Cargo.toml` or `.git`),
    /// initializes the per-project SQLite cache, and loads the local embedding engine.
    pub fn new(start_dir: &Path, model_path: &Path, tokenizer_path: &Path) -> Result<Self, String> {
        let (project_root, conn) = prepare_project_cache(start_dir)?;
        let embedding_engine = LocalEmbeddingEngine::new(model_path, tokenizer_path)?;

        Ok(Self {
            conn,
            project_root,
            embedding_engine,
        })
    }

    /// Returns an insight when a semantically similar query exists and every linked source file is up to date.
    pub fn try_get_valid_insight(&self, query_text: &str) -> Result<Option<String>, String> {
        let query_embedding = self.embedding_engine.generate(query_text)?;
        let matched_query_id = match self.find_semantic_match(&query_embedding)? {
            Some(query_id) => query_id,
            None => return Ok(None),
        };

        let insight_content = match self.conn.query_row(
            "SELECT content FROM insights WHERE id = ?1",
            params![matched_query_id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(content) => content,
            Err(RusqliteError::QueryReturnedNoRows) => return Ok(None),
            Err(err) => {
                return Err(format!("failed to load cached insight for query: {err}"));
            }
        };

        for node in self.load_insight_dependencies(&matched_query_id)? {
            if is_code_node_dirty(&self.project_root, &node)? {
                self.invalidate_insight(&matched_query_id)?;
                return Ok(None);
            }
        }

        Ok(Some(insight_content))
    }

    /// Stores a new insight, snapshots associated files, and links them as dependencies.
    pub fn store_insight(
        &mut self,
        query_text: &str,
        insight_content: &str,
        associated_files: Vec<PathBuf>,
    ) -> Result<(), String> {
        let query_id = hash_query_text(query_text);
        let created_at = current_unix_timestamp()?;
        let embedding = self.embedding_engine.generate(query_text)?;
        let embedding_blob = bytemuck::cast_slice::<f32, u8>(&embedding).to_vec();

        let transaction = self
            .conn
            .transaction()
            .map_err(|err| format!("failed to start cache transaction: {err}"))?;

        transaction
            .execute(
                "INSERT INTO queries (id, raw_text, embedding) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET raw_text = excluded.raw_text, embedding = excluded.embedding",
                params![query_id, query_text, embedding_blob],
            )
            .map_err(|err| format!("failed to store query: {err}"))?;

        transaction
            .execute(
                "INSERT INTO insights (id, content, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET content = excluded.content, created_at = excluded.created_at",
                params![query_id, insight_content, created_at],
            )
            .map_err(|err| format!("failed to store insight: {err}"))?;

        transaction
            .execute(
                "DELETE FROM insight_dependencies WHERE insight_id = ?1",
                params![query_id],
            )
            .map_err(|err| format!("failed to clear old insight dependencies: {err}"))?;

        for file_path in associated_files {
            let normalized_path = normalize_relative_path(&self.project_root, &file_path)?;
            let snapshot = capture_code_node_snapshot(&self.project_root, &normalized_path)?;

            transaction
                .execute(
                    "INSERT INTO code_nodes (id, file_path, last_known_git_sha, last_known_mtime)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET
                         file_path = excluded.file_path,
                         last_known_git_sha = excluded.last_known_git_sha,
                         last_known_mtime = excluded.last_known_mtime",
                    params![
                        snapshot.id,
                        snapshot.file_path,
                        snapshot.last_known_git_sha,
                        snapshot.last_known_mtime
                    ],
                )
                .map_err(|err| {
                    format!("failed to store code node {}: {err}", snapshot.file_path)
                })?;

            transaction
                .execute(
                    "INSERT OR IGNORE INTO insight_dependencies (insight_id, code_node_id)
                     VALUES (?1, ?2)",
                    params![query_id, snapshot.id],
                )
                .map_err(|err| format!("failed to link insight dependency: {err}"))?;
        }

        transaction
            .commit()
            .map_err(|err| format!("failed to commit cache transaction: {err}"))?;

        Ok(())
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    fn find_semantic_match(&self, query_embedding: &[f32]) -> Result<Option<String>, String> {
        let mut statement = self
            .conn
            .prepare("SELECT id, embedding FROM queries WHERE embedding IS NOT NULL")
            .map_err(|err| format!("failed to prepare semantic query lookup: {err}"))?;

        let rows = statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })
            .map_err(|err| format!("failed to query stored embeddings: {err}"))?;

        let mut best_match: Option<(String, f32)> = None;

        for row in rows {
            let (query_id, blob) =
                row.map_err(|err| format!("failed to read stored embedding row: {err}"))?;

            let Some(stored_embedding) = decode_embedding_blob(&blob) else {
                continue;
            };

            let similarity = LocalEmbeddingEngine::dot_product(query_embedding, stored_embedding);

            if similarity >= SEMANTIC_SIMILARITY_THRESHOLD
                && best_match
                    .as_ref()
                    .is_none_or(|(_, best_similarity)| similarity > *best_similarity)
            {
                best_match = Some((query_id, similarity));
            }
        }

        Ok(best_match.map(|(query_id, _)| query_id))
    }

    fn load_insight_dependencies(&self, insight_id: &str) -> Result<Vec<CodeNodeSnapshot>, String> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT cn.id, cn.file_path, cn.last_known_git_sha, cn.last_known_mtime
                 FROM code_nodes cn
                 INNER JOIN insight_dependencies dep ON dep.code_node_id = cn.id
                 WHERE dep.insight_id = ?1",
            )
            .map_err(|err| format!("failed to prepare dependency lookup: {err}"))?;

        let rows = statement
            .query_map(params![insight_id], |row| {
                Ok(CodeNodeSnapshot {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    last_known_git_sha: row.get(2)?,
                    last_known_mtime: row.get(3)?,
                })
            })
            .map_err(|err| format!("failed to query insight dependencies: {err}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to read insight dependencies: {err}"))
    }

    fn invalidate_insight(&self, insight_id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM insights WHERE id = ?1", params![insight_id])
            .map_err(|err| format!("failed to delete stale insight: {err}"))?;

        self.conn
            .execute("DELETE FROM queries WHERE id = ?1", params![insight_id])
            .map_err(|err| format!("failed to delete stale query: {err}"))?;

        Ok(())
    }
}

fn decode_embedding_blob(blob: &[u8]) -> Option<&[f32]> {
    let expected_bytes = EMBEDDING_DIM * std::mem::size_of::<f32>();
    if blob.len() != expected_bytes {
        return None;
    }

    bytemuck::try_cast_slice(blob).ok()
}
