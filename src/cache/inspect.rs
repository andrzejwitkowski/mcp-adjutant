use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

use super::file_state::{is_code_node_dirty, CodeNodeSnapshot};
use super::project::current_unix_timestamp;

#[derive(Debug, Clone, Serialize)]
pub struct AgentEvaluationRow {
    pub id: String,
    pub agent_name: String,
    pub original_task: String,
    pub agent_output: String,
    pub score: i32,
    pub feedback_notes: String,
    pub created_at: i64,
}

pub const EVALUATIONS_PAGE_SIZE: u32 = 20;

#[derive(Debug, Clone, Serialize)]
pub struct EvaluationsPage {
    pub items: Vec<AgentEvaluationRow>,
    pub page: u32,
    pub page_size: u32,
    pub total_count: usize,
    pub total_pages: u32,
    pub avg_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheOverview {
    pub project_root: String,
    pub query_count: usize,
    pub insight_count: usize,
    pub code_node_count: usize,
    pub embedding_count: usize,
    pub dependency_count: usize,
    pub evaluation_count: usize,
    pub web_query_count: usize,
    pub web_report_count: usize,
    pub web_source_count: usize,
    pub web_dependency_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CachedQueryRow {
    pub id: String,
    pub raw_text: String,
    pub has_embedding: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CachedInsightRow {
    pub id: String,
    pub query_text: Option<String>,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeNodeRow {
    pub id: String,
    pub file_path: String,
    pub last_known_git_sha: Option<String>,
    pub last_known_mtime: i64,
    pub is_dirty: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InsightDependencyRow {
    pub insight_id: String,
    pub code_node_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebQueryRow {
    pub id: String,
    pub raw_text: String,
    pub has_embedding: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebReportRow {
    pub id: String,
    pub query_text: Option<String>,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebSourceRow {
    pub id: String,
    pub url: String,
    pub content_sha256: String,
    pub fetched_at: i64,
    pub is_stale: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebFetchDependencyRow {
    pub report_id: String,
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheSnapshot {
    pub overview: CacheOverview,
    pub queries: Vec<CachedQueryRow>,
    pub insights: Vec<CachedInsightRow>,
    pub code_nodes: Vec<CodeNodeRow>,
    pub dependencies: Vec<InsightDependencyRow>,
    pub web_queries: Vec<WebQueryRow>,
    pub web_reports: Vec<WebReportRow>,
    pub web_sources: Vec<WebSourceRow>,
    pub web_dependencies: Vec<WebFetchDependencyRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoutCachePage {
    pub overview: CacheOverview,
    pub queries: Vec<CachedQueryRow>,
    pub insights: Vec<CachedInsightRow>,
    pub code_nodes: Vec<CodeNodeRow>,
    pub dependencies: Vec<InsightDependencyRow>,
    pub page: u32,
    pub page_size: u32,
    pub total_count: usize,
    pub total_pages: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebCachePage {
    pub overview: CacheOverview,
    pub web_queries: Vec<WebQueryRow>,
    pub web_reports: Vec<WebReportRow>,
    pub web_sources: Vec<WebSourceRow>,
    pub web_dependencies: Vec<WebFetchDependencyRow>,
    pub page: u32,
    pub page_size: u32,
    pub total_count: usize,
    pub total_pages: u32,
}

pub fn list_evaluations(conn: &Connection) -> Result<Vec<AgentEvaluationRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT id, agent_name, original_task, agent_output, score, feedback_notes, created_at
             FROM agent_evaluations
             ORDER BY created_at DESC",
        )
        .map_err(|err| format!("failed to prepare evaluations query: {err}"))?;

    let rows = statement
        .query_map([], map_evaluation_row)
        .map_err(|err| format!("failed to query evaluations: {err}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read evaluation row: {err}"))
}

pub fn list_evaluations_page(
    conn: &Connection,
    page: u32,
    page_size: u32,
) -> Result<EvaluationsPage, String> {
    let page = page.max(1);
    let page_size = page_size.max(1);
    let offset = (page - 1).saturating_mul(page_size);

    let (total_count, avg_score) = evaluation_stats(conn)?;
    let total_pages = if total_count == 0 {
        0
    } else {
        total_count.div_ceil(page_size as usize) as u32
    };

    let mut statement = conn
        .prepare(
            "SELECT id, agent_name, original_task, agent_output, score, feedback_notes, created_at
             FROM agent_evaluations
             ORDER BY created_at DESC
             LIMIT ?1 OFFSET ?2",
        )
        .map_err(|err| format!("failed to prepare evaluations page query: {err}"))?;

    let rows = statement
        .query_map(params![page_size, offset], map_evaluation_row)
        .map_err(|err| format!("failed to query evaluations page: {err}"))?;

    let items = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read evaluation row: {err}"))?;

    Ok(EvaluationsPage {
        items,
        page,
        page_size,
        total_count,
        total_pages,
        avg_score,
    })
}

fn evaluation_stats(conn: &Connection) -> Result<(usize, Option<f64>), String> {
    let (count, avg): (i64, Option<f64>) = conn
        .query_row(
            "SELECT COUNT(*), AVG(score) FROM agent_evaluations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|err| format!("failed to read evaluation stats: {err}"))?;

    let total_count =
        usize::try_from(count).map_err(|err| format!("invalid evaluation count: {err}"))?;
    Ok((total_count, avg))
}

fn map_evaluation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentEvaluationRow> {
    Ok(AgentEvaluationRow {
        id: row.get(0)?,
        agent_name: row.get(1)?,
        original_task: row.get(2)?,
        agent_output: row.get(3)?,
        score: row.get(4)?,
        feedback_notes: row.get(5)?,
        created_at: row.get(6)?,
    })
}

pub fn load_cache_snapshot(
    conn: &Connection,
    project_root: &Path,
    ttl_seconds: i64,
) -> Result<CacheSnapshot, String> {
    let overview = load_cache_overview(conn, project_root)?;
    Ok(CacheSnapshot {
        overview,
        queries: list_queries(conn, None, None)?,
        insights: list_insights(conn, None, None)?,
        code_nodes: list_code_nodes(conn, project_root, None, None)?,
        dependencies: list_dependencies(conn, None, None)?,
        web_queries: list_web_queries(conn, None, None)?,
        web_reports: list_web_reports(conn, None, None)?,
        web_sources: list_web_sources(conn, ttl_seconds, None, None)?,
        web_dependencies: list_web_dependencies(conn, None, None)?,
    })
}

pub fn load_scout_cache_page(
    conn: &Connection,
    project_root: &Path,
    page: u32,
    page_size: u32,
) -> Result<ScoutCachePage, String> {
    let page = page.max(1);
    let page_size = page_size.max(1);
    let offset = (page - 1).saturating_mul(page_size);
    let overview = load_cache_overview(conn, project_root)?;
    let total_count = [
        overview.query_count,
        overview.insight_count,
        overview.code_node_count,
        overview.dependency_count,
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let total_pages = page_count(total_count, page_size);

    Ok(ScoutCachePage {
        overview,
        queries: list_queries(conn, Some(page_size), Some(offset))?,
        insights: list_insights(conn, Some(page_size), Some(offset))?,
        code_nodes: list_code_nodes(conn, project_root, Some(page_size), Some(offset))?,
        dependencies: list_dependencies(conn, Some(page_size), Some(offset))?,
        page,
        page_size,
        total_count,
        total_pages,
    })
}

pub fn load_web_cache_page(
    conn: &Connection,
    project_root: &Path,
    ttl_seconds: i64,
    page: u32,
    page_size: u32,
) -> Result<WebCachePage, String> {
    let page = page.max(1);
    let page_size = page_size.max(1);
    let offset = (page - 1).saturating_mul(page_size);
    let overview = load_cache_overview(conn, project_root)?;
    let total_count = [
        overview.web_query_count,
        overview.web_report_count,
        overview.web_source_count,
        overview.web_dependency_count,
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let total_pages = page_count(total_count, page_size);

    Ok(WebCachePage {
        overview,
        web_queries: list_web_queries(conn, Some(page_size), Some(offset))?,
        web_reports: list_web_reports(conn, Some(page_size), Some(offset))?,
        web_sources: list_web_sources(conn, ttl_seconds, Some(page_size), Some(offset))?,
        web_dependencies: list_web_dependencies(conn, Some(page_size), Some(offset))?,
        page,
        page_size,
        total_count,
        total_pages,
    })
}

fn load_cache_overview(conn: &Connection, project_root: &Path) -> Result<CacheOverview, String> {
    Ok(CacheOverview {
        project_root: project_root.display().to_string(),
        query_count: count_rows(conn, "queries")?,
        insight_count: count_rows(conn, "insights")?,
        code_node_count: count_rows(conn, "code_nodes")?,
        embedding_count: count_embeddings(conn, "queries")?,
        dependency_count: count_rows(conn, "insight_dependencies")?,
        evaluation_count: count_rows(conn, "agent_evaluations")?,
        web_query_count: count_rows(conn, "web_queries")?,
        web_report_count: count_rows(conn, "web_reports")?,
        web_source_count: count_rows(conn, "web_sources")?,
        web_dependency_count: count_rows(conn, "web_fetch_dependencies")?,
    })
}

fn page_count(total_count: usize, page_size: u32) -> u32 {
    if total_count == 0 {
        0
    } else {
        total_count.div_ceil(page_size as usize) as u32
    }
}

fn count_embeddings(conn: &Connection, table: &str) -> Result<usize, String> {
    let sql = format!(
        "SELECT COUNT(*) FROM {table} WHERE embedding IS NOT NULL AND length(embedding) > 0"
    );
    let count: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|err| format!("failed to count embeddings in {table}: {err}"))?;
    usize::try_from(count).map_err(|err| format!("invalid embedding count for {table}: {err}"))
}

fn list_queries(
    conn: &Connection,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<CachedQueryRow>, String> {
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => "SELECT id, raw_text, embedding FROM queries ORDER BY id LIMIT ?1 OFFSET ?2",
        _ => "SELECT id, raw_text, embedding FROM queries ORDER BY id",
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare queries query: {err}"))?;

    let rows = match (limit, offset) {
        (Some(limit), Some(offset)) => statement
            .query_map(params![limit, offset], map_query_row)
            .map_err(|err| format!("failed to query cached queries: {err}"))?,
        _ => statement
            .query_map([], map_query_row)
            .map_err(|err| format!("failed to query cached queries: {err}"))?,
    };

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read query row: {err}"))
}

fn map_query_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedQueryRow> {
    let embedding: Option<Vec<u8>> = row.get(2)?;
    Ok(CachedQueryRow {
        id: row.get(0)?,
        raw_text: row.get(1)?,
        has_embedding: embedding.is_some_and(|blob| !blob.is_empty()),
    })
}

fn list_insights(
    conn: &Connection,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<CachedInsightRow>, String> {
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => {
            "SELECT i.id, q.raw_text, i.content, i.created_at
             FROM insights i
             LEFT JOIN queries q ON q.id = i.id
             ORDER BY i.created_at DESC
             LIMIT ?1 OFFSET ?2"
        }
        _ => {
            "SELECT i.id, q.raw_text, i.content, i.created_at
             FROM insights i
             LEFT JOIN queries q ON q.id = i.id
             ORDER BY i.created_at DESC"
        }
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare insights query: {err}"))?;

    let rows = match (limit, offset) {
        (Some(limit), Some(offset)) => statement
            .query_map(params![limit, offset], map_insight_row)
            .map_err(|err| format!("failed to query insights: {err}"))?,
        _ => statement
            .query_map([], map_insight_row)
            .map_err(|err| format!("failed to query insights: {err}"))?,
    };

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read insight row: {err}"))
}

fn map_insight_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedInsightRow> {
    Ok(CachedInsightRow {
        id: row.get(0)?,
        query_text: row.get(1)?,
        content: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn list_code_nodes(
    conn: &Connection,
    project_root: &Path,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<CodeNodeRow>, String> {
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => {
            "SELECT id, file_path, last_known_git_sha, last_known_mtime
             FROM code_nodes
             ORDER BY file_path
             LIMIT ?1 OFFSET ?2"
        }
        _ => {
            "SELECT id, file_path, last_known_git_sha, last_known_mtime
             FROM code_nodes
             ORDER BY file_path"
        }
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare code_nodes query: {err}"))?;

    let rows = match (limit, offset) {
        (Some(limit), Some(offset)) => statement
            .query_map(params![limit, offset], map_code_node_snapshot)
            .map_err(|err| format!("failed to query code_nodes: {err}"))?,
        _ => statement
            .query_map([], map_code_node_snapshot)
            .map_err(|err| format!("failed to query code_nodes: {err}"))?,
    };

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read code_node row: {err}"))
        .and_then(|nodes| map_code_node_rows(project_root, nodes))
}

fn map_code_node_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodeNodeSnapshot> {
    Ok(CodeNodeSnapshot {
        id: row.get(0)?,
        file_path: row.get(1)?,
        last_known_git_sha: row.get(2)?,
        last_known_mtime: row.get(3)?,
    })
}

fn map_code_node_rows(
    project_root: &Path,
    nodes: Vec<CodeNodeSnapshot>,
) -> Result<Vec<CodeNodeRow>, String> {
    nodes
        .into_iter()
        .map(|node| {
            let is_dirty = is_code_node_dirty(project_root, &node)?;
            Ok(CodeNodeRow {
                id: node.id,
                file_path: node.file_path,
                last_known_git_sha: node.last_known_git_sha,
                last_known_mtime: node.last_known_mtime,
                is_dirty,
            })
        })
        .collect()
}

fn list_dependencies(
    conn: &Connection,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<InsightDependencyRow>, String> {
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => {
            "SELECT insight_id, code_node_id
             FROM insight_dependencies
             ORDER BY insight_id, code_node_id
             LIMIT ?1 OFFSET ?2"
        }
        _ => {
            "SELECT insight_id, code_node_id
             FROM insight_dependencies
             ORDER BY insight_id, code_node_id"
        }
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare dependencies query: {err}"))?;

    let rows = match (limit, offset) {
        (Some(limit), Some(offset)) => statement
            .query_map(params![limit, offset], map_dependency_row)
            .map_err(|err| format!("failed to query dependencies: {err}"))?,
        _ => statement
            .query_map([], map_dependency_row)
            .map_err(|err| format!("failed to query dependencies: {err}"))?,
    };

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read dependency row: {err}"))
}

fn map_dependency_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InsightDependencyRow> {
    Ok(InsightDependencyRow {
        insight_id: row.get(0)?,
        code_node_id: row.get(1)?,
    })
}

fn list_web_queries(
    conn: &Connection,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<WebQueryRow>, String> {
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => {
            "SELECT id, raw_text, embedding FROM web_queries ORDER BY id LIMIT ?1 OFFSET ?2"
        }
        _ => "SELECT id, raw_text, embedding FROM web_queries ORDER BY id",
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare web_queries query: {err}"))?;
    let rows = match (limit, offset) {
        (Some(limit), Some(offset)) => statement
            .query_map(params![limit, offset], map_web_query_row)
            .map_err(|err| format!("failed to query web_queries: {err}"))?,
        _ => statement
            .query_map([], map_web_query_row)
            .map_err(|err| format!("failed to query web_queries: {err}"))?,
    };
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_query row: {err}"))
}

fn map_web_query_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebQueryRow> {
    let embedding: Option<Vec<u8>> = row.get(2)?;
    Ok(WebQueryRow {
        id: row.get(0)?,
        raw_text: row.get(1)?,
        has_embedding: embedding.is_some_and(|blob| !blob.is_empty()),
    })
}

fn list_web_reports(
    conn: &Connection,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<WebReportRow>, String> {
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => {
            "SELECT r.id, q.raw_text, r.content, r.created_at
             FROM web_reports r
             LEFT JOIN web_queries q ON q.id = r.id
             ORDER BY r.created_at DESC
             LIMIT ?1 OFFSET ?2"
        }
        _ => {
            "SELECT r.id, q.raw_text, r.content, r.created_at
             FROM web_reports r
             LEFT JOIN web_queries q ON q.id = r.id
             ORDER BY r.created_at DESC"
        }
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare web_reports query: {err}"))?;
    let rows = match (limit, offset) {
        (Some(limit), Some(offset)) => statement
            .query_map(params![limit, offset], map_web_report_row)
            .map_err(|err| format!("failed to query web_reports: {err}"))?,
        _ => statement
            .query_map([], map_web_report_row)
            .map_err(|err| format!("failed to query web_reports: {err}"))?,
    };
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_report row: {err}"))
}

fn map_web_report_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebReportRow> {
    Ok(WebReportRow {
        id: row.get(0)?,
        query_text: row.get(1)?,
        content: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn list_web_sources(
    conn: &Connection,
    ttl_seconds: i64,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<WebSourceRow>, String> {
    let now = current_unix_timestamp()?;
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => {
            "SELECT id, url, content_sha256, fetched_at FROM web_sources ORDER BY url LIMIT ?1 OFFSET ?2"
        }
        _ => "SELECT id, url, content_sha256, fetched_at FROM web_sources ORDER BY url",
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare web_sources query: {err}"))?;

    if let (Some(limit), Some(offset)) = (limit, offset) {
        let rows = statement
            .query_map(params![limit, offset], |row| map_web_source_row(row, now, ttl_seconds))
            .map_err(|err| format!("failed to query web_sources: {err}"))?;
        return rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to read web_source row: {err}"));
    }

    let rows = statement
        .query_map([], |row| map_web_source_row(row, now, ttl_seconds))
        .map_err(|err| format!("failed to query web_sources: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_source row: {err}"))
}

fn map_web_source_row(
    row: &rusqlite::Row<'_>,
    now: i64,
    ttl_seconds: i64,
) -> rusqlite::Result<WebSourceRow> {
    let fetched_at: i64 = row.get(3)?;
    let is_stale = now - fetched_at > ttl_seconds;
    Ok(WebSourceRow {
        id: row.get(0)?,
        url: row.get(1)?,
        content_sha256: row.get(2)?,
        fetched_at,
        is_stale,
    })
}

fn list_web_dependencies(
    conn: &Connection,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<WebFetchDependencyRow>, String> {
    let sql = match (limit, offset) {
        (Some(_), Some(_)) => {
            "SELECT report_id, source_id FROM web_fetch_dependencies ORDER BY report_id, source_id LIMIT ?1 OFFSET ?2"
        }
        _ => {
            "SELECT report_id, source_id FROM web_fetch_dependencies ORDER BY report_id, source_id"
        }
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|err| format!("failed to prepare web_dependencies query: {err}"))?;
    let rows = match (limit, offset) {
        (Some(limit), Some(offset)) => statement
            .query_map(params![limit, offset], map_web_dependency_row)
            .map_err(|err| format!("failed to query web_dependencies: {err}"))?,
        _ => statement
            .query_map([], map_web_dependency_row)
            .map_err(|err| format!("failed to query web_dependencies: {err}"))?,
    };
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_dependency row: {err}"))
}

fn map_web_dependency_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebFetchDependencyRow> {
    Ok(WebFetchDependencyRow {
        report_id: row.get(0)?,
        source_id: row.get(1)?,
    })
}

fn count_rows(conn: &Connection, table: &str) -> Result<usize, String> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|err| format!("failed to count {table}: {err}"))?;
    usize::try_from(count).map_err(|err| format!("invalid row count for {table}: {err}"))
}
