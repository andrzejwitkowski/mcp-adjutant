use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

use super::file_state::{is_code_node_dirty, CodeNodeSnapshot};

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
pub struct CacheSnapshot {
    pub overview: CacheOverview,
    pub queries: Vec<CachedQueryRow>,
    pub insights: Vec<CachedInsightRow>,
    pub code_nodes: Vec<CodeNodeRow>,
    pub dependencies: Vec<InsightDependencyRow>,
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

    let total_count = usize::try_from(count)
        .map_err(|err| format!("invalid evaluation count: {err}"))?;
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
) -> Result<CacheSnapshot, String> {
    let queries = list_queries(conn)?;
    let insights = list_insights(conn)?;
    let code_nodes = list_code_nodes(conn, project_root)?;
    let dependencies = list_dependencies(conn)?;

    let overview = CacheOverview {
        project_root: project_root.display().to_string(),
        query_count: queries.len(),
        insight_count: insights.len(),
        code_node_count: code_nodes.len(),
        embedding_count: queries.iter().filter(|q| q.has_embedding).count(),
        dependency_count: dependencies.len(),
        evaluation_count: count_rows(conn, "agent_evaluations")?,
    };

    Ok(CacheSnapshot {
        overview,
        queries,
        insights,
        code_nodes,
        dependencies,
    })
}

fn list_queries(conn: &Connection) -> Result<Vec<CachedQueryRow>, String> {
    let mut statement = conn
        .prepare("SELECT id, raw_text, embedding FROM queries ORDER BY id")
        .map_err(|err| format!("failed to prepare queries query: {err}"))?;

    let rows = statement
        .query_map([], |row| {
            let embedding: Option<Vec<u8>> = row.get(2)?;
            Ok(CachedQueryRow {
                id: row.get(0)?,
                raw_text: row.get(1)?,
                has_embedding: embedding.is_some_and(|blob| !blob.is_empty()),
            })
        })
        .map_err(|err| format!("failed to query cached queries: {err}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read query row: {err}"))
}

fn list_insights(conn: &Connection) -> Result<Vec<CachedInsightRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT i.id, q.raw_text, i.content, i.created_at
             FROM insights i
             LEFT JOIN queries q ON q.id = i.id
             ORDER BY i.created_at DESC",
        )
        .map_err(|err| format!("failed to prepare insights query: {err}"))?;

    let rows = statement
        .query_map([], |row| {
            Ok(CachedInsightRow {
                id: row.get(0)?,
                query_text: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|err| format!("failed to query insights: {err}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read insight row: {err}"))
}

fn list_code_nodes(conn: &Connection, project_root: &Path) -> Result<Vec<CodeNodeRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT id, file_path, last_known_git_sha, last_known_mtime
             FROM code_nodes
             ORDER BY file_path",
        )
        .map_err(|err| format!("failed to prepare code_nodes query: {err}"))?;

    let rows = statement
        .query_map([], |row| {
            Ok(CodeNodeSnapshot {
                id: row.get(0)?,
                file_path: row.get(1)?,
                last_known_git_sha: row.get(2)?,
                last_known_mtime: row.get(3)?,
            })
        })
        .map_err(|err| format!("failed to query code_nodes: {err}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read code_node row: {err}"))
        .and_then(|nodes| {
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
        })
}

fn list_dependencies(conn: &Connection) -> Result<Vec<InsightDependencyRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT insight_id, code_node_id
             FROM insight_dependencies
             ORDER BY insight_id, code_node_id",
        )
        .map_err(|err| format!("failed to prepare dependencies query: {err}"))?;

    let rows = statement
        .query_map([], |row| {
            Ok(InsightDependencyRow {
                insight_id: row.get(0)?,
                code_node_id: row.get(1)?,
            })
        })
        .map_err(|err| format!("failed to query dependencies: {err}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read dependency row: {err}"))
}

fn count_rows(conn: &Connection, table: &str) -> Result<usize, String> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|err| format!("failed to count {table}: {err}"))?;
    usize::try_from(count).map_err(|err| format!("invalid row count for {table}: {err}"))
}
