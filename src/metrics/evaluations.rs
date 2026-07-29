use rusqlite::{params, Connection};
use serde::Serialize;

use crate::cache::{is_dense_builder_report_exemplar, normalize_agent_name};

pub const EVALUATIONS_PAGE_SIZE: u32 = 20;

#[derive(Debug, Clone, Serialize)]
pub struct AgentEvaluationRow {
    pub id: String,
    pub agent_name: String,
    pub original_task: String,
    pub agent_output: String,
    pub score: i32,
    pub feedback_notes: String,
    pub desired_output: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_root: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvaluationsPage {
    pub items: Vec<AgentEvaluationRow>,
    pub page: u32,
    pub page_size: u32,
    pub total_count: usize,
    pub total_pages: u32,
    pub avg_score: Option<f64>,
}

const EVAL_SELECT: &str = "SELECT id, agent_name, original_task, agent_output, score, feedback_notes,
        desired_output, created_at, project_root
 FROM agent_evaluations";

pub fn list_evaluations(conn: &Connection) -> Result<Vec<AgentEvaluationRow>, String> {
    let mut statement = conn
        .prepare(&format!("{EVAL_SELECT} ORDER BY created_at DESC"))
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
        .prepare(&format!("{EVAL_SELECT} ORDER BY created_at DESC LIMIT ?1 OFFSET ?2"))
        .map_err(|err| format!("failed to prepare evaluations page query: {err}"))?;
    let items = statement
        .query_map(params![page_size, offset], map_evaluation_row)
        .map_err(|err| format!("failed to query evaluations page: {err}"))?
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
    Ok((
        usize::try_from(count).map_err(|err| format!("invalid evaluation count: {err}"))?,
        avg,
    ))
}

fn map_evaluation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentEvaluationRow> {
    Ok(AgentEvaluationRow {
        id: row.get(0)?,
        agent_name: row.get(1)?,
        original_task: row.get(2)?,
        agent_output: row.get(3)?,
        score: row.get(4)?,
        feedback_notes: row.get(5)?,
        desired_output: row.get(6)?,
        created_at: row.get(7)?,
        project_root: row.get(8)?,
    })
}

pub fn load_best_desired_output_exemplar(
    conn: &Connection,
    agent_name: &str,
) -> Result<Option<String>, String> {
    let canonical = normalize_agent_name(agent_name);
    conn.query_row(
        "SELECT desired_output FROM agent_evaluations
         WHERE agent_name = ?1 AND desired_output != '' AND score >= 7
         ORDER BY score DESC, created_at DESC
         LIMIT 1",
        params![canonical],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        err => Err(format!("failed to load desired_output exemplar: {err}")),
    })
}

pub fn load_best_builder_dense_exemplar(conn: &Connection) -> Result<Option<String>, String> {
    let canonical = normalize_agent_name("Phase_4_Builder");
    let mut statement = conn
        .prepare(
            "SELECT desired_output FROM agent_evaluations
             WHERE agent_name = ?1 AND desired_output != '' AND score >= 7
             ORDER BY score DESC, created_at DESC
             LIMIT 20",
        )
        .map_err(|err| format!("failed to prepare builder exemplar query: {err}"))?;
    for row in statement
        .query_map(params![canonical], |row| row.get::<_, String>(0))
        .map_err(|err| format!("failed to query builder exemplars: {err}"))?
    {
        let text = row.map_err(|err| format!("failed to read builder exemplar: {err}"))?;
        if is_dense_builder_report_exemplar(&text) {
            return Ok(Some(text));
        }
    }
    Ok(None)
}
