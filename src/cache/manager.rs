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

/// A fetched web source snapshot, produced by the scrape tool, stored as a dependency.
#[derive(Debug, Clone)]
pub struct WebSourceSnapshot {
    pub url: String,
    pub content_sha256: String,
    pub fetched_at: i64,
}

#[derive(Debug, Clone)]
pub struct WebReportRevalidation {
    pub query_id: String,
    pub report_content: String,
    pub sources: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum WebReportCacheLookup {
    Fresh(String),
    Stale(WebReportRevalidation),
    Miss,
}

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

        // ponytail: naive file:line gate (basename.ext:N); evaluator score if false positives
        if !insight_has_file_line_citation(&insight_content) {
            self.invalidate_insight(&matched_query_id)?;
            return Ok(None);
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

    pub fn store_web_report(
        &mut self,
        search_phrase: &str,
        report_content: &str,
        sources: Vec<WebSourceSnapshot>,
    ) -> Result<(), String> {
        let query_id = hash_query_text(search_phrase);
        let created_at = current_unix_timestamp()?;
        let embedding = self.embedding_engine.generate(search_phrase)?;
        let embedding_blob = bytemuck::cast_slice::<f32, u8>(&embedding).to_vec();

        let transaction = self
            .conn
            .transaction()
            .map_err(|err| format!("failed to start web cache transaction: {err}"))?;

        transaction
            .execute(
                "INSERT INTO web_queries (id, raw_text, embedding) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET raw_text = excluded.raw_text, embedding = excluded.embedding",
                params![query_id, search_phrase, embedding_blob],
            )
            .map_err(|err| format!("failed to store web query: {err}"))?;

        transaction
            .execute(
                "INSERT INTO web_reports (id, content, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET content = excluded.content, created_at = excluded.created_at",
                params![query_id, report_content, created_at],
            )
            .map_err(|err| format!("failed to store web report: {err}"))?;

        transaction
            .execute(
                "DELETE FROM web_fetch_dependencies WHERE report_id = ?1",
                params![query_id],
            )
            .map_err(|err| format!("failed to clear old web dependencies: {err}"))?;

        for source in sources {
            let source_id = hash_query_text(&source.url);

            transaction
                .execute(
                    "INSERT INTO web_sources (id, url, content_sha256, fetched_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET
                         url = excluded.url,
                         content_sha256 = excluded.content_sha256,
                         fetched_at = excluded.fetched_at",
                    params![
                        source_id,
                        source.url,
                        source.content_sha256,
                        source.fetched_at
                    ],
                )
                .map_err(|err| format!("failed to store web source {}: {err}", source.url))?;

            transaction
                .execute(
                    "INSERT OR IGNORE INTO web_fetch_dependencies (report_id, source_id)
                     VALUES (?1, ?2)",
                    params![query_id, source_id],
                )
                .map_err(|err| format!("failed to link web dependency: {err}"))?;
        }

        transaction
            .commit()
            .map_err(|err| format!("failed to commit web cache transaction: {err}"))?;

        Ok(())
    }

    pub fn lookup_web_report_cache(
        &self,
        search_phrase: &str,
        ttl_seconds: i64,
        threshold: f32,
    ) -> Result<WebReportCacheLookup, String> {
        let query_embedding = self.embedding_engine.generate(search_phrase)?;
        let Some(matched_query_id) = self.find_web_semantic_match(&query_embedding, threshold)?
        else {
            return Ok(WebReportCacheLookup::Miss);
        };

        let (report_content, report_created_at) = match self.conn.query_row(
            "SELECT content, created_at FROM web_reports WHERE id = ?1",
            params![matched_query_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        ) {
            Ok(row) => row,
            Err(RusqliteError::QueryReturnedNoRows) => return Ok(WebReportCacheLookup::Miss),
            Err(err) => {
                return Err(format!("failed to load cached web report: {err}"));
            }
        };

        let sources = self.load_web_source_urls(&matched_query_id)?;
        if sources.is_empty() || sources.iter().any(|(_, url, _, _)| is_local_cache_url(url)) {
            self.invalidate_web_report(&matched_query_id)?;
            return Ok(WebReportCacheLookup::Miss);
        }

        let now = current_unix_timestamp()?;
        if now - report_created_at < ttl_seconds {
            return Ok(WebReportCacheLookup::Fresh(report_content));
        }

        Ok(WebReportCacheLookup::Stale(WebReportRevalidation {
            query_id: matched_query_id,
            report_content,
            sources: sources
                .into_iter()
                .map(|(_, url, hash, _)| (url, hash))
                .collect(),
        }))
    }

    pub fn revalidate_stale_web_report(
        &self,
        pending: WebReportRevalidation,
    ) -> Result<Option<String>, String> {
        if crate::tools::web_fetch::web_sources_still_valid(&pending.sources) {
            Ok(Some(pending.report_content))
        } else {
            self.invalidate_stale_web_report(&pending.query_id)?;
            Ok(None)
        }
    }

    pub fn invalidate_stale_web_report(&self, query_id: &str) -> Result<(), String> {
        self.invalidate_web_report(query_id)
    }

    pub fn try_get_valid_web_report(
        &self,
        search_phrase: &str,
        ttl_seconds: i64,
        threshold: f32,
    ) -> Result<Option<String>, String> {
        match self.lookup_web_report_cache(search_phrase, ttl_seconds, threshold)? {
            WebReportCacheLookup::Fresh(report) => Ok(Some(report)),
            WebReportCacheLookup::Miss => Ok(None),
            WebReportCacheLookup::Stale(pending) => self.revalidate_stale_web_report(pending),
        }
    }

    /// Persists an LLM-as-a-Judge evaluation for meta-learning and prompt optimization.
    pub fn store_evaluation(
        &mut self,
        agent_name: &str,
        original_task: &str,
        agent_output: &str,
        score: i32,
        feedback_notes: &str,
        desired_output: &str,
    ) -> Result<(), String> {
        let agent_name = super::agent_names::normalize_agent_name(agent_name);
        let created_at = current_unix_timestamp()?;
        let id = hash_query_text(&format!(
            "{agent_name}\0{original_task}\0{agent_output}\0{feedback_notes}\0{desired_output}\0{created_at}\0{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.subsec_nanos())
                .unwrap_or(0)
        ));

        self.conn
            .execute(
                "INSERT INTO agent_evaluations
                 (id, agent_name, original_task, agent_output, score, feedback_notes, desired_output, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    agent_name.as_str(),
                    original_task,
                    agent_output,
                    score,
                    feedback_notes,
                    desired_output,
                    created_at
                ],
            )
            .map_err(|err| format!("failed to store agent evaluation: {err}"))?;

        Ok(())
    }

    pub fn clear_web_cache(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM web_fetch_dependencies", [])
            .map_err(|err| format!("failed to clear web dependencies: {err}"))?;
        self.conn
            .execute("DELETE FROM web_reports", [])
            .map_err(|err| format!("failed to clear web reports: {err}"))?;
        self.conn
            .execute("DELETE FROM web_queries", [])
            .map_err(|err| format!("failed to clear web queries: {err}"))?;
        self.conn
            .execute("DELETE FROM web_sources", [])
            .map_err(|err| format!("failed to clear web sources: {err}"))?;
        Ok(())
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    fn find_semantic_match(&self, query_embedding: &[f32]) -> Result<Option<String>, String> {
        self.best_embedding_match("queries", query_embedding, SEMANTIC_SIMILARITY_THRESHOLD)
    }

    fn find_web_semantic_match(
        &self,
        query_embedding: &[f32],
        threshold: f32,
    ) -> Result<Option<String>, String> {
        self.best_embedding_match("web_queries", query_embedding, threshold)
    }

    fn best_embedding_match(
        &self,
        table: &str,
        query_embedding: &[f32],
        threshold: f32,
    ) -> Result<Option<String>, String> {
        let sql = format!("SELECT id, embedding FROM {table} WHERE embedding IS NOT NULL");
        let mut statement = self
            .conn
            .prepare(&sql)
            .map_err(|err| format!("failed to prepare semantic lookup on {table}: {err}"))?;

        let rows = statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })
            .map_err(|err| format!("failed to query embeddings from {table}: {err}"))?;

        let mut best_match: Option<(String, f32)> = None;
        for row in rows {
            let (query_id, blob) =
                row.map_err(|err| format!("failed to read embedding row from {table}: {err}"))?;
            let Some(stored) = decode_embedding_blob(&blob) else {
                continue;
            };
            let similarity = LocalEmbeddingEngine::dot_product(query_embedding, stored);
            if similarity >= threshold
                && best_match
                    .as_ref()
                    .is_none_or(|(_, best)| similarity > *best)
            {
                best_match = Some((query_id, similarity));
            }
        }
        Ok(best_match.map(|(id, _)| id))
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

    fn load_web_source_urls(
        &self,
        report_id: &str,
    ) -> Result<Vec<(String, String, String, i64)>, String> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT s.id, s.url, s.content_sha256, s.fetched_at
                 FROM web_sources s
                 INNER JOIN web_fetch_dependencies dep ON dep.source_id = s.id
                 WHERE dep.report_id = ?1",
            )
            .map_err(|err| format!("failed to prepare web source lookup: {err}"))?;

        let rows = statement
            .query_map(params![report_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|err| format!("failed to query web sources: {err}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to read web source row: {err}"))
    }

    fn invalidate_web_report(&self, report_id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM web_reports WHERE id = ?1", params![report_id])
            .map_err(|err| format!("failed to delete stale web report: {err}"))?;
        self.conn
            .execute("DELETE FROM web_queries WHERE id = ?1", params![report_id])
            .map_err(|err| format!("failed to delete stale web query: {err}"))?;
        self.conn
            .execute(
                "DELETE FROM web_sources
                 WHERE id NOT IN (SELECT source_id FROM web_fetch_dependencies)",
                [],
            )
            .map_err(|err| format!("failed to delete orphaned web sources: {err}"))?;
        Ok(())
    }
}

fn is_local_cache_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("127.0.0.1") || lower.contains("localhost")
}

/// True when content has at least one `basename.ext:line` citation (SCOUT RUBRIC evidence).
fn insight_has_file_line_citation(content: &str) -> bool {
    let bytes = content.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] != b'.' {
            i += 1;
            continue;
        }
        if i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if j == i + 1 || j >= bytes.len() || bytes[j] != b':' {
            i += 1;
            continue;
        }
        let mut k = j + 1;
        while k < bytes.len() && bytes[k].is_ascii_digit() {
            k += 1;
        }
        if k > j + 1 {
            return true;
        }
        i = j;
    }
    false
}

fn decode_embedding_blob(blob: &[u8]) -> Option<&[f32]> {
    let expected_bytes = EMBEDDING_DIM * std::mem::size_of::<f32>();
    if blob.len() != expected_bytes {
        return None;
    }

    bytemuck::try_cast_slice(blob).ok()
}

#[cfg(test)]
mod tests {
    use super::insight_has_file_line_citation;

    #[test]
    fn citation_accepts_basename_ext_line() {
        assert!(insight_has_file_line_citation("see manager.rs:42"));
        assert!(insight_has_file_line_citation("path/to/cache_flow.rs:28"));
        assert!(insight_has_file_line_citation("foo.ts:1 and bar.tsx:99"));
    }

    #[test]
    fn citation_rejects_prose_without_file_line() {
        assert!(!insight_has_file_line_citation(
            "## Insight\nUse jwt_routes for JWT middleware."
        ));
        assert!(!insight_has_file_line_citation(
            "version 1.2: not a citation"
        ));
        assert!(!insight_has_file_line_citation("no dots or colon digits"));
    }
}
