use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{params, Connection};

use crate::cache::current_unix_timestamp;
use crate::domain::AgentPhase;
use crate::llm::LlmUsage;

use super::context::current_job_context;
use super::time::utc_date_from_secs;

static METRICS: OnceLock<Arc<Mutex<MetricsStore>>> = OnceLock::new();
static SESSION_ID: OnceLock<String> = OnceLock::new();

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS llm_calls (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        request_uuid TEXT,
        mcp_tool TEXT,
        agent_phase TEXT NOT NULL,
        model_name TEXT,
        prompt_tokens INTEGER NOT NULL,
        completion_tokens INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        utc_date TEXT NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS cache_hits (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        request_uuid TEXT,
        mcp_tool TEXT,
        agent_phase TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        utc_date TEXT NOT NULL
    );",
    "CREATE INDEX IF NOT EXISTS idx_llm_calls_utc_date ON llm_calls(utc_date);",
    "CREATE INDEX IF NOT EXISTS idx_cache_hits_utc_date ON cache_hits(utc_date);",
    "CREATE TABLE IF NOT EXISTS agent_runs (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        request_uuid TEXT,
        mcp_tool TEXT NOT NULL,
        agent_phase TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        utc_date TEXT NOT NULL
    );",
    "CREATE INDEX IF NOT EXISTS idx_agent_runs_utc_date ON agent_runs(utc_date);",
];

pub fn resolve_metrics_db_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|dir| dir.join("metrics.db"))
        .unwrap_or_else(|| PathBuf::from("metrics.db"))
}

pub fn new_session_id() -> String {
    let pid = std::process::id();
    let started_at = current_unix_timestamp().unwrap_or(0);
    format!("session-{started_at}-{pid}")
}

pub fn init(session_id: String, store: Arc<Mutex<MetricsStore>>) {
    let _ = SESSION_ID.set(session_id);
    let _ = METRICS.set(store);
}

pub fn session_id() -> &'static str {
    SESSION_ID.get().map(String::as_str).unwrap_or("unknown")
}

pub fn metrics_store() -> Option<Arc<Mutex<MetricsStore>>> {
    METRICS.get().cloned()
}

pub struct MetricsStore {
    conn: Connection,
}

impl MetricsStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("create metrics db dir {}: {err}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .map_err(|err| format!("open metrics db {}: {err}", path.display()))?;
        for migration in MIGRATIONS {
            conn.execute_batch(migration)
                .map_err(|err| format!("metrics migration failed: {err}"))?;
        }
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn record_llm_call(
        &self,
        phase: AgentPhase,
        model_name: &str,
        usage: LlmUsage,
    ) -> Result<(), String> {
        let created_at = current_unix_timestamp()?;
        let utc_date = utc_date_from_secs(created_at);
        let job = current_job_context();
        let (request_uuid, mcp_tool) = job
            .map(|ctx| (ctx.request_uuid, ctx.mcp_tool))
            .unwrap_or((None, None));

        self.conn
            .execute(
                "INSERT INTO llm_calls (
                    id, session_id, request_uuid, mcp_tool, agent_phase, model_name,
                    prompt_tokens, completion_tokens, created_at, utc_date
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    new_event_id(),
                    session_id(),
                    request_uuid,
                    mcp_tool,
                    phase_label(phase),
                    model_name,
                    usage.prompt_tokens as i64,
                    usage.completion_tokens as i64,
                    created_at,
                    utc_date,
                ],
            )
            .map_err(|err| format!("record llm call: {err}"))?;
        Ok(())
    }

    pub fn record_cache_hit(&self, phase: AgentPhase) -> Result<(), String> {
        let created_at = current_unix_timestamp()?;
        let utc_date = utc_date_from_secs(created_at);
        let job = current_job_context();
        let (request_uuid, mcp_tool) = job
            .map(|ctx| (ctx.request_uuid, ctx.mcp_tool))
            .unwrap_or((None, None));

        self.conn
            .execute(
                "INSERT INTO cache_hits (
                    id, session_id, request_uuid, mcp_tool, agent_phase, created_at, utc_date
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    new_event_id(),
                    session_id(),
                    request_uuid,
                    mcp_tool,
                    phase_label(phase),
                    created_at,
                    utc_date,
                ],
            )
            .map_err(|err| format!("record cache hit: {err}"))?;
        Ok(())
    }

    pub fn record_agent_run(
        &self,
        mcp_tool: &str,
        request_uuid: Option<String>,
    ) -> Result<(), String> {
        let Some(phase) = mcp_tool_to_phase(mcp_tool) else {
            return Ok(());
        };
        let created_at = current_unix_timestamp()?;
        let utc_date = utc_date_from_secs(created_at);
        let request_uuid = request_uuid.or_else(|| {
            current_job_context().and_then(|ctx| ctx.request_uuid)
        });

        self.conn
            .execute(
                "INSERT INTO agent_runs (
                    id, session_id, request_uuid, mcp_tool, agent_phase, created_at, utc_date
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    new_event_id(),
                    session_id(),
                    request_uuid,
                    mcp_tool,
                    phase_label(phase),
                    created_at,
                    utc_date,
                ],
            )
            .map_err(|err| format!("record agent run: {err}"))?;
        Ok(())
    }
}

pub fn record_llm_call(phase: AgentPhase, model_name: &str, usage: LlmUsage) {
    let Some(store) = metrics_store() else {
        return;
    };
    let Ok(guard) = store.lock() else {
        return;
    };
    if let Err(err) = guard.record_llm_call(phase, model_name, usage) {
        tracing::warn!("metrics llm call not recorded: {err}");
    }
}

pub fn record_cache_hit(phase: AgentPhase) {
    let Some(store) = metrics_store() else {
        return;
    };
    let Ok(guard) = store.lock() else {
        return;
    };
    if let Err(err) = guard.record_cache_hit(phase) {
        tracing::warn!("metrics cache hit not recorded: {err}");
    }
}

pub fn record_agent_run(mcp_tool: &str, request_uuid: Option<String>) {
    let Some(store) = metrics_store() else {
        return;
    };
    let Ok(guard) = store.lock() else {
        return;
    };
    if let Err(err) = guard.record_agent_run(mcp_tool, request_uuid) {
        tracing::warn!("metrics agent run not recorded: {err}");
    }
}

fn mcp_tool_to_phase(mcp_tool: &str) -> Option<AgentPhase> {
    match mcp_tool {
        "scout_context" => Some(AgentPhase::Scout),
        "verify_and_triage" => Some(AgentPhase::Triage),
        "generate_tests_and_scaffolding" => Some(AgentPhase::Builder),
        "evaluate_agent_performance" => Some(AgentPhase::Evaluator),
        "web_fetch" => Some(AgentPhase::WebFetcher),
        "execute_global_refactor" => Some(AgentPhase::Transformer),
        _ => None,
    }
}

fn phase_label(phase: AgentPhase) -> String {
    serde_json::to_value(phase)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{phase:?}"))
}

fn new_event_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = current_unix_timestamp().unwrap_or(0);
    format!("{now:x}-{seq:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::context::{with_job_context_async, JobContext};

    #[tokio::test]
    async fn record_llm_call_and_cache_hit_persist_rows() {
        let dir = std::env::temp_dir().join(format!("metrics-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let db_path = dir.join("metrics.db");

        let store = Arc::new(Mutex::new(MetricsStore::open(&db_path).expect("open")));
        init("session-test".to_string(), Arc::clone(&store));

        with_job_context_async(
            JobContext {
                request_uuid: Some("req-1".to_string()),
                mcp_tool: Some("scout_context".to_string()),
            },
            || async {
                store
                    .lock()
                    .expect("lock")
                    .record_llm_call(
                        AgentPhase::Scout,
                        "deepseek-chat",
                        LlmUsage {
                            prompt_tokens: 10,
                            completion_tokens: 5,
                            total_tokens: 15,
                            cached_tokens: 0,
                        },
                    )
                    .expect("llm");

                store
                    .lock()
                    .expect("lock")
                    .record_cache_hit(AgentPhase::Scout)
                    .expect("cache");
            },
        )
        .await;

        with_job_context_async(
            JobContext {
                request_uuid: Some("req-1".to_string()),
                mcp_tool: Some("verify_and_triage".to_string()),
            },
            || async {
                store
                    .lock()
                    .expect("lock")
                    .record_agent_run("verify_and_triage", Some("req-1".to_string()))
                    .expect("run");
            },
        )
        .await;

        let store = store.lock().expect("lock");
        let conn = store.connection();
        let llm_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_calls", [], |row| row.get(0))
            .expect("count llm");
        let cache_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM cache_hits", [], |row| row.get(0))
            .expect("count cache");
        let run_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_runs", [], |row| row.get(0))
            .expect("count runs");
        assert_eq!(llm_rows, 1);
        assert_eq!(cache_rows, 1);
        assert_eq!(run_rows, 1);

        drop(store);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
