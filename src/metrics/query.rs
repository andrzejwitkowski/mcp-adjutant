use rusqlite::{params, Connection};

use serde::Serialize;

use super::time::utc_date_from_secs;
use crate::cache::current_unix_timestamp;
use crate::domain::AgentPhase;

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSummary {
    pub session_id: String,
    pub utc_date: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cache_hits: CacheHitSummary,
    pub by_phase: Vec<PhaseTokenSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheHitSummary {
    pub scout: u64,
    pub web_fetcher: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PhaseTokenSummary {
    pub agent_phase: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub job_runs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyMetricsRow {
    pub date: String,
    pub agent_phase: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cache_hits: u64,
    pub job_runs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineBucket {
    pub hour: u32,
    pub agent_phase: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cumulative_prompt_tokens: u64,
    pub cumulative_completion_tokens: u64,
}

pub fn query_summary(conn: &Connection, session_id: &str) -> Result<MetricsSummary, String> {
    let utc_date = utc_date_from_secs(current_unix_timestamp()?);

    let prompt_tokens: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(prompt_tokens), 0) FROM llm_calls WHERE utc_date = ?1",
            params![utc_date],
            |row| row.get(0),
        )
        .map_err(|err| format!("summary prompt tokens: {err}"))?;

    let completion_tokens: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(completion_tokens), 0) FROM llm_calls WHERE utc_date = ?1",
            params![utc_date],
            |row| row.get(0),
        )
        .map_err(|err| format!("summary completion tokens: {err}"))?;

    let scout_hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cache_hits WHERE utc_date = ?1 AND agent_phase = ?2",
            params![utc_date, phase_label(AgentPhase::Scout)],
            |row| row.get(0),
        )
        .map_err(|err| format!("summary scout cache hits: {err}"))?;

    let web_hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cache_hits WHERE utc_date = ?1 AND agent_phase = ?2",
            params![utc_date, phase_label(AgentPhase::WebFetcher)],
            |row| row.get(0),
        )
        .map_err(|err| format!("summary web cache hits: {err}"))?;

    let mut stmt = conn
        .prepare(
            "WITH phases(agent_phase) AS (
                SELECT DISTINCT agent_phase FROM llm_calls WHERE utc_date = ?1
                UNION
                SELECT DISTINCT agent_phase FROM agent_runs WHERE utc_date = ?1
             )
             SELECT p.agent_phase,
                    COALESCE(SUM(l.prompt_tokens), 0),
                    COALESCE(SUM(l.completion_tokens), 0),
                    COALESCE(
                        NULLIF((
                            SELECT COUNT(*) FROM agent_runs r
                            WHERE r.utc_date = ?1 AND r.agent_phase = p.agent_phase
                        ), 0),
                        (
                            SELECT COUNT(DISTINCT l2.request_uuid)
                            FROM llm_calls l2
                            WHERE l2.utc_date = ?1 AND l2.agent_phase = p.agent_phase
                        ),
                        0
                    )
             FROM phases p
             LEFT JOIN llm_calls l
               ON l.utc_date = ?1 AND l.agent_phase = p.agent_phase
             GROUP BY p.agent_phase
             ORDER BY p.agent_phase",
        )
        .map_err(|err| format!("summary by phase prepare: {err}"))?;

    let by_phase = stmt
        .query_map(params![utc_date], |row| {
            Ok(PhaseTokenSummary {
                agent_phase: row.get(0)?,
                prompt_tokens: row.get::<_, i64>(1)? as u64,
                completion_tokens: row.get::<_, i64>(2)? as u64,
                job_runs: row.get::<_, i64>(3)? as u64,
            })
        })
        .map_err(|err| format!("summary by phase query: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("summary by phase row: {err}"))?;

    Ok(MetricsSummary {
        session_id: session_id.to_string(),
        utc_date,
        prompt_tokens: prompt_tokens as u64,
        completion_tokens: completion_tokens as u64,
        cache_hits: CacheHitSummary {
            scout: scout_hits as u64,
            web_fetcher: web_hits as u64,
        },
        by_phase,
    })
}

pub fn query_daily(
    conn: &Connection,
    from_date: &str,
    to_date: &str,
) -> Result<Vec<DailyMetricsRow>, String> {
    let mut stmt = conn
        .prepare(
            "WITH days(agent_phase, date) AS (
                SELECT DISTINCT agent_phase, utc_date FROM llm_calls
                WHERE utc_date BETWEEN ?1 AND ?2
                UNION
                SELECT DISTINCT agent_phase, utc_date FROM cache_hits
                WHERE utc_date BETWEEN ?1 AND ?2
                UNION
                SELECT DISTINCT agent_phase, utc_date FROM agent_runs
                WHERE utc_date BETWEEN ?1 AND ?2
             )
             SELECT d.date,
                    d.agent_phase,
                    COALESCE(SUM(l.prompt_tokens), 0) AS prompt_tokens,
                    COALESCE(SUM(l.completion_tokens), 0) AS completion_tokens,
                    COALESCE((
                        SELECT COUNT(*) FROM cache_hits c
                        WHERE c.utc_date = d.date AND c.agent_phase = d.agent_phase
                    ), 0) AS cache_hits,
                    COALESCE(
                        NULLIF((
                            SELECT COUNT(*) FROM agent_runs r
                            WHERE r.utc_date = d.date AND r.agent_phase = d.agent_phase
                        ), 0),
                        (
                            SELECT COUNT(DISTINCT l2.request_uuid)
                            FROM llm_calls l2
                            WHERE l2.utc_date = d.date AND l2.agent_phase = d.agent_phase
                        ),
                        0
                    ) AS job_runs
             FROM days d
             LEFT JOIN llm_calls l
               ON l.utc_date = d.date AND l.agent_phase = d.agent_phase
             GROUP BY d.date, d.agent_phase
             ORDER BY d.date, d.agent_phase",
        )
        .map_err(|err| format!("daily prepare: {err}"))?;

    let rows = stmt
        .query_map(params![from_date, to_date], |row| {
            Ok(DailyMetricsRow {
                date: row.get(0)?,
                agent_phase: row.get(1)?,
                prompt_tokens: row.get::<_, i64>(2)? as u64,
                completion_tokens: row.get::<_, i64>(3)? as u64,
                cache_hits: row.get::<_, i64>(4)? as u64,
                job_runs: row.get::<_, i64>(5)? as u64,
            })
        })
        .map_err(|err| format!("daily query: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("daily row: {err}"))?;

    Ok(rows)
}

pub fn query_timeline(conn: &Connection, date: &str) -> Result<Vec<TimelineBucket>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT CAST(strftime('%H', created_at, 'unixepoch') AS INTEGER) AS hour,
                    agent_phase,
                    COALESCE(SUM(prompt_tokens), 0),
                    COALESCE(SUM(completion_tokens), 0)
             FROM llm_calls
             WHERE utc_date = ?1
             GROUP BY hour, agent_phase
             ORDER BY agent_phase, hour",
        )
        .map_err(|err| format!("timeline prepare: {err}"))?;

    let hourly = stmt
        .query_map(params![date], |row| {
            Ok((
                row.get::<_, i32>(0)? as u32,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
            ))
        })
        .map_err(|err| format!("timeline query: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("timeline row: {err}"))?;

    let mut by_phase: std::collections::BTreeMap<String, Vec<(u32, u64, u64)>> =
        std::collections::BTreeMap::new();
    for (hour, phase, prompt, completion) in hourly {
        by_phase
            .entry(phase)
            .or_default()
            .push((hour, prompt, completion));
    }

    let mut buckets = Vec::new();
    for (phase, mut rows) in by_phase {
        rows.sort_by_key(|(hour, _, _)| *hour);
        let mut cumulative_prompt = 0_u64;
        let mut cumulative_completion = 0_u64;
        for (hour, prompt, completion) in rows {
            cumulative_prompt += prompt;
            cumulative_completion += completion;
            buckets.push(TimelineBucket {
                hour,
                agent_phase: phase.clone(),
                prompt_tokens: prompt,
                completion_tokens: completion,
                cumulative_prompt_tokens: cumulative_prompt,
                cumulative_completion_tokens: cumulative_completion,
            });
        }
    }

    buckets.sort_by(|left, right| {
        left.agent_phase
            .cmp(&right.agent_phase)
            .then(left.hour.cmp(&right.hour))
    });

    Ok(buckets)
}

fn phase_label(phase: AgentPhase) -> String {
    serde_json::to_value(phase)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{phase:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{init, MetricsStore};
    use std::sync::{Arc, Mutex};

    #[test]
    fn daily_and_timeline_queries_aggregate_rows() {
        let dir = std::env::temp_dir().join(format!("metrics-query-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let db_path = dir.join("metrics.db");
        let store = Arc::new(Mutex::new(MetricsStore::open(&db_path).expect("open")));
        init("session-query".to_string(), Arc::clone(&store));

        {
            let store = store.lock().expect("lock");
            let conn = store.connection();
            conn.execute(
                "INSERT INTO llm_calls (
                    id, session_id, request_uuid, mcp_tool, agent_phase, model_name,
                    prompt_tokens, completion_tokens, created_at, utc_date
                ) VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    "a",
                    "session-query",
                    "scout",
                    "deepseek-chat",
                    100,
                    20,
                    1_704_067_200,
                    "2024-01-01"
                ],
            )
            .expect("insert scout morning");
            conn.execute(
                "INSERT INTO llm_calls (
                    id, session_id, request_uuid, mcp_tool, agent_phase, model_name,
                    prompt_tokens, completion_tokens, created_at, utc_date
                ) VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    "b",
                    "session-query",
                    "triage",
                    "deepseek-coder",
                    50,
                    10,
                    1_704_070_800,
                    "2024-01-01"
                ],
            )
            .expect("insert triage afternoon");
            conn.execute(
                "INSERT INTO cache_hits (
                    id, session_id, request_uuid, mcp_tool, agent_phase, created_at, utc_date
                ) VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5)",
                params!["c", "session-query", "scout", 1_704_067_200, "2024-01-01"],
            )
            .expect("insert cache hit");
            conn.execute(
                "INSERT INTO agent_runs (
                    id, session_id, request_uuid, mcp_tool, agent_phase, created_at, utc_date
                ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)",
                params![
                    "d",
                    "session-query",
                    "verify_and_triage",
                    "triage",
                    1_704_070_800,
                    "2024-01-01"
                ],
            )
            .expect("insert triage run");
        }

        let store = store.lock().expect("lock");
        let conn = store.connection();
        let daily = query_daily(conn, "2024-01-01", "2024-01-01").expect("daily");
        assert_eq!(daily.len(), 2);
        assert!(daily
            .iter()
            .any(|row| row.agent_phase == "scout" && row.cache_hits == 1));
        assert!(daily
            .iter()
            .any(|row| row.agent_phase == "triage" && row.job_runs == 1));

        let timeline = query_timeline(conn, "2024-01-01").expect("timeline");
        assert!(timeline.iter().any(|row| row.agent_phase == "scout"));
        assert!(timeline
            .iter()
            .any(|row| row.cumulative_prompt_tokens >= 100));

        drop(store);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
