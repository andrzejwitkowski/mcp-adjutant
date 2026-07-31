use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{params, Connection};

use crate::cache::{
    current_unix_timestamp, hash_query_text, mcp_workspace_root, normalize_agent_name,
};
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
    "CREATE TABLE IF NOT EXISTS premium_bridge (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        request_uuid TEXT,
        mcp_tool TEXT,
        agent_phase TEXT NOT NULL,
        premium_in_tokens INTEGER NOT NULL,
        premium_out_tokens INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        utc_date TEXT NOT NULL
    );",
    "CREATE INDEX IF NOT EXISTS idx_premium_bridge_utc_date ON premium_bridge(utc_date);",
    "CREATE TABLE IF NOT EXISTS agent_evaluations (
        id TEXT PRIMARY KEY,
        agent_name TEXT NOT NULL,
        original_task TEXT NOT NULL,
        agent_output TEXT NOT NULL,
        score INTEGER NOT NULL,
        feedback_notes TEXT NOT NULL,
        desired_output TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL,
        project_root TEXT NOT NULL DEFAULT '',
        request_uuid TEXT,
        utc_date TEXT NOT NULL
    );",
    "CREATE INDEX IF NOT EXISTS idx_agent_evaluations_utc_date ON agent_evaluations(utc_date);",
    "CREATE INDEX IF NOT EXISTS idx_agent_evaluations_created_at ON agent_evaluations(created_at DESC);",
    "CREATE INDEX IF NOT EXISTS idx_agent_evaluations_agent_score ON agent_evaluations(agent_name, score);",
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
        let phases = phases_for_mcp_tool(mcp_tool);
        if phases.is_empty() {
            return Ok(());
        }
        let created_at = current_unix_timestamp()?;
        let utc_date = utc_date_from_secs(created_at);
        let request_uuid =
            request_uuid.or_else(|| current_job_context().and_then(|ctx| ctx.request_uuid));

        for phase in phases {
            // Skip build-only / no-LLM completions so token tables don't show Runs with 0/0.
            if let Some(uuid) = request_uuid.as_deref() {
                if !self.phase_had_metered_activity(phase, uuid)? {
                    continue;
                }
            }
            self.conn
                .execute(
                    "INSERT INTO agent_runs (
                    id, session_id, request_uuid, mcp_tool, agent_phase, created_at, utc_date
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        new_event_id(),
                        session_id(),
                        request_uuid.clone(),
                        mcp_tool,
                        phase_label(phase),
                        created_at,
                        utc_date,
                    ],
                )
                .map_err(|err| format!("record agent run: {err}"))?;
        }
        Ok(())
    }

    fn phase_had_metered_activity(
        &self,
        phase: AgentPhase,
        request_uuid: &str,
    ) -> Result<bool, String> {
        let label = phase_label(phase);
        let llm: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM llm_calls WHERE request_uuid = ?1 AND agent_phase = ?2",
                params![request_uuid, label],
                |row| row.get(0),
            )
            .map_err(|err| format!("metered llm check: {err}"))?;
        if llm > 0 {
            return Ok(true);
        }
        let cache: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM cache_hits WHERE request_uuid = ?1 AND agent_phase = ?2",
                params![request_uuid, label],
                |row| row.get(0),
            )
            .map_err(|err| format!("metered cache check: {err}"))?;
        if cache > 0 {
            return Ok(true);
        }
        let bridge: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM premium_bridge WHERE request_uuid = ?1 AND agent_phase = ?2",
                params![request_uuid, label],
                |row| row.get(0),
            )
            .map_err(|err| format!("metered bridge check: {err}"))?;
        Ok(bridge > 0)
    }

    /// One row per job on the first metered phase (LLM/cache), else `phases[0]`.
    pub fn record_premium_bridge(
        &self,
        mcp_tool: &str,
        request_uuid: Option<String>,
        premium_in_tokens: u64,
        premium_out_tokens: u64,
    ) -> Result<(), String> {
        if premium_in_tokens == 0 && premium_out_tokens == 0 {
            return Ok(());
        }
        let phases = phases_for_mcp_tool(mcp_tool);
        if phases.is_empty() {
            return Ok(());
        }
        let request_uuid =
            request_uuid.or_else(|| current_job_context().and_then(|ctx| ctx.request_uuid));
        let phase = if let Some(uuid) = request_uuid.as_deref() {
            let mut selected = None;
            for candidate in phases.iter().copied() {
                if self.phase_had_metered_activity(candidate, uuid)? {
                    selected = Some(candidate);
                    break;
                }
            }
            selected.unwrap_or(phases[0])
        } else {
            phases[0]
        };
        let created_at = current_unix_timestamp()?;
        let utc_date = utc_date_from_secs(created_at);

        self.conn
            .execute(
                "INSERT INTO premium_bridge (
                    id, session_id, request_uuid, mcp_tool, agent_phase,
                    premium_in_tokens, premium_out_tokens, created_at, utc_date
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    new_event_id(),
                    session_id(),
                    request_uuid,
                    mcp_tool,
                    phase_label(phase),
                    premium_in_tokens as i64,
                    premium_out_tokens as i64,
                    created_at,
                    utc_date,
                ],
            )
            .map_err(|err| format!("record premium bridge: {err}"))?;
        Ok(())
    }

    pub fn store_evaluation(
        &self,
        agent_name: &str,
        original_task: &str,
        agent_output: &str,
        score: i32,
        feedback_notes: &str,
        desired_output: &str,
    ) -> Result<(), String> {
        let agent_name = normalize_agent_name(agent_name);
        let created_at = current_unix_timestamp()?;
        let utc_date = utc_date_from_secs(created_at);
        let job = current_job_context();
        let request_uuid = job.as_ref().and_then(|ctx| ctx.request_uuid.clone());
        let project_root = job
            .and_then(|ctx| ctx.workspace_root)
            .unwrap_or_else(mcp_workspace_root)
            .display()
            .to_string();
        let id = hash_query_text(&format!(
            "{agent_name}\0{original_task}\0{agent_output}\0{feedback_notes}\0{desired_output}\0{created_at}\0{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.subsec_nanos())
                .unwrap_or(0)
        ));

        self.conn
            .execute(
                "INSERT INTO agent_evaluations (
                    id, agent_name, original_task, agent_output, score, feedback_notes,
                    desired_output, created_at, project_root, request_uuid, utc_date
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id,
                    agent_name.as_str(),
                    original_task,
                    agent_output,
                    score,
                    feedback_notes,
                    desired_output,
                    created_at,
                    project_root,
                    request_uuid,
                    utc_date,
                ],
            )
            .map_err(|err| format!("failed to store agent evaluation: {err}"))?;
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

pub fn record_premium_bridge(
    mcp_tool: &str,
    request_uuid: Option<String>,
    premium_in_tokens: u64,
    premium_out_tokens: u64,
) {
    let Some(store) = metrics_store() else {
        return;
    };
    let Ok(guard) = store.lock() else {
        return;
    };
    if let Err(err) = guard.record_premium_bridge(
        mcp_tool,
        request_uuid,
        premium_in_tokens,
        premium_out_tokens,
    ) {
        tracing::warn!("metrics premium bridge not recorded: {err}");
    }
}

pub fn store_evaluation(
    agent_name: &str,
    original_task: &str,
    agent_output: &str,
    score: i32,
    feedback_notes: &str,
    desired_output: &str,
) -> Result<(), String> {
    let store = metrics_store().ok_or_else(|| "metrics store not initialized".to_string())?;
    let guard = store
        .lock()
        .map_err(|_| "metrics store lock poisoned".to_string())?;
    guard.store_evaluation(
        agent_name,
        original_task,
        agent_output,
        score,
        feedback_notes,
        desired_output,
    )
}

fn phases_for_mcp_tool(mcp_tool: &str) -> Vec<AgentPhase> {
    match mcp_tool {
        "scout_context" => vec![AgentPhase::Scout],
        "verify_and_triage" => vec![AgentPhase::Triage],
        "generate_tests_and_scaffolding" => vec![AgentPhase::Builder],
        "evaluate_agent_performance" => vec![AgentPhase::Evaluator],
        "analyze_log" => vec![AgentPhase::LogAnalyzer],
        "web_fetch" => vec![AgentPhase::WebFetcher],
        "execute_global_refactor" => vec![AgentPhase::Transformer],
        "babysit_pr" => vec![AgentPhase::Babysitter],
        "plan_blueprint" => vec![AgentPhase::Planner, AgentPhase::PlannerEmit],
        // triage + builder (and builder may scout); premium bridge uses first phase only
        "execute_blueprint" => vec![AgentPhase::Triage, AgentPhase::Builder],
        "prepare_git_copy" | "create_git_branch" => vec![AgentPhase::GitJanitor],
        "transpile_types" => vec![AgentPhase::Builder],
        "compact_context" => vec![AgentPhase::Pruner],
        _ => vec![],
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
                workspace_root: None,
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

                store
                    .lock()
                    .expect("lock")
                    .record_agent_run("scout_context", Some("req-1".to_string()))
                    .expect("scout run");
            },
        )
        .await;

        with_job_context_async(
            JobContext {
                request_uuid: Some("req-triage-green".to_string()),
                mcp_tool: Some("verify_and_triage".to_string()),
                workspace_root: None,
            },
            || async {
                // Green-path triage: no LLM → no agent_runs row.
                store
                    .lock()
                    .expect("lock")
                    .record_agent_run("verify_and_triage", Some("req-triage-green".to_string()))
                    .expect("run");
            },
        )
        .await;

        with_job_context_async(
            JobContext {
                request_uuid: Some("req-triage-llm".to_string()),
                mcp_tool: Some("verify_and_triage".to_string()),
                workspace_root: None,
            },
            || async {
                store
                    .lock()
                    .expect("lock")
                    .record_llm_call(
                        AgentPhase::Triage,
                        "deepseek-chat",
                        LlmUsage {
                            prompt_tokens: 3,
                            completion_tokens: 1,
                            total_tokens: 4,
                            cached_tokens: 0,
                        },
                    )
                    .expect("triage llm");
                store
                    .lock()
                    .expect("lock")
                    .record_agent_run("verify_and_triage", Some("req-triage-llm".to_string()))
                    .expect("triage run");
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
        let triage_runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_runs WHERE agent_phase = 'triage'",
                [],
                |row| row.get(0),
            )
            .expect("count triage runs");
        assert_eq!(llm_rows, 2);
        assert_eq!(cache_rows, 1);
        assert_eq!(run_rows, 2); // scout + metered triage
        assert_eq!(triage_runs, 1);

        drop(store);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn record_agent_run_maps_plan_blueprint_to_planner_and_emit() {
        let dir =
            std::env::temp_dir().join(format!("metrics-plan-blueprint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let db_path = dir.join("metrics.db");

        let store = Arc::new(Mutex::new(MetricsStore::open(&db_path).expect("open")));
        init("session-plan-blueprint".to_string(), Arc::clone(&store));

        with_job_context_async(
            JobContext {
                request_uuid: Some("req-planner-1".to_string()),
                mcp_tool: Some("plan_blueprint".to_string()),
                workspace_root: None,
            },
            || async {
                let store = store.lock().expect("lock");
                store
                    .record_llm_call(
                        AgentPhase::Planner,
                        "m",
                        LlmUsage {
                            prompt_tokens: 1,
                            completion_tokens: 1,
                            total_tokens: 2,
                            cached_tokens: 0,
                        },
                    )
                    .expect("planner llm");
                store
                    .record_llm_call(
                        AgentPhase::PlannerEmit,
                        "m",
                        LlmUsage {
                            prompt_tokens: 2,
                            completion_tokens: 2,
                            total_tokens: 4,
                            cached_tokens: 0,
                        },
                    )
                    .expect("emit llm");
                store
                    .record_agent_run("plan_blueprint", Some("req-planner-1".to_string()))
                    .expect("run");
            },
        )
        .await;

        {
            let store = store.lock().expect("lock");
            let conn = store.connection();
            let run_rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM agent_runs", [], |row| row.get(0))
                .expect("count runs");
            assert_eq!(run_rows, 2);

            let mut phases: Vec<String> = Vec::new();
            let mut stmt = conn
                .prepare("SELECT agent_phase FROM agent_runs ORDER BY agent_phase")
                .expect("prepare");
            let rows = stmt.query_map([], |row| row.get(0)).expect("query");
            for row in rows {
                phases.push(row.expect("row"));
            }
            assert_eq!(phases, vec!["planner", "planner_emit"]);
        }

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_premium_bridge_one_row_for_multi_phase_tool() {
        let dir = std::env::temp_dir().join(format!("metrics-premium-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let db_path = dir.join("metrics.db");
        let store = Arc::new(Mutex::new(MetricsStore::open(&db_path).expect("open")));
        init("session-premium".to_string(), Arc::clone(&store));

        {
            let store = store.lock().expect("lock");
            store
                .record_premium_bridge("plan_blueprint", Some("req-prem-1".to_string()), 40, 80)
                .expect("record");
            let conn = store.connection();
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM premium_bridge", [], |row| row.get(0))
                .expect("count");
            assert_eq!(rows, 1);
            let phase: String = conn
                .query_row("SELECT agent_phase FROM premium_bridge", [], |row| {
                    row.get(0)
                })
                .expect("phase");
            assert_eq!(phase, "planner");
            let (inn, out): (i64, i64) = conn
                .query_row(
                    "SELECT premium_in_tokens, premium_out_tokens FROM premium_bridge",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("tokens");
            assert_eq!((inn, out), (40, 80));

            store
                .record_premium_bridge("execute_blueprint", Some("req-prem-2".to_string()), 10, 20)
                .expect("record execute");
            let exec_phase: String = conn
                .query_row(
                    "SELECT agent_phase FROM premium_bridge WHERE request_uuid = 'req-prem-2'",
                    [],
                    |row| row.get(0),
                )
                .expect("exec phase");
            assert_eq!(exec_phase, "triage");
            assert_eq!(
                phases_for_mcp_tool("execute_blueprint"),
                vec![AgentPhase::Triage, AgentPhase::Builder]
            );
        }

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn green_triage_with_bridge_counts_as_run() {
        let dir =
            std::env::temp_dir().join(format!("metrics-triage-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let store = Arc::new(Mutex::new(
            MetricsStore::open(&dir.join("metrics.db")).expect("open"),
        ));
        init("session-triage-bridge".to_string(), Arc::clone(&store));

        store
            .lock()
            .expect("lock")
            .record_premium_bridge("verify_and_triage", Some("req-tb".into()), 10, 20)
            .expect("bridge");
        store
            .lock()
            .expect("lock")
            .record_agent_run("verify_and_triage", Some("req-tb".into()))
            .expect("run");

        let runs: i64 = store
            .lock()
            .expect("lock")
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM agent_runs WHERE agent_phase = 'triage' AND request_uuid = 'req-tb'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(runs, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_blueprint_bridge_follows_builder_llm() {
        let dir = std::env::temp_dir().join(format!("metrics-exec-builder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let store = Arc::new(Mutex::new(
            MetricsStore::open(&dir.join("metrics.db")).expect("open"),
        ));
        init("session-exec-builder".to_string(), Arc::clone(&store));

        with_job_context_async(
            JobContext {
                request_uuid: Some("req-eb".into()),
                mcp_tool: Some("execute_blueprint".into()),
                workspace_root: None,
            },
            || async {
                let store = store.lock().expect("lock");
                store
                    .record_llm_call(
                        AgentPhase::Builder,
                        "m",
                        LlmUsage {
                            prompt_tokens: 1,
                            completion_tokens: 1,
                            total_tokens: 2,
                            cached_tokens: 0,
                        },
                    )
                    .expect("builder llm");
                store
                    .record_premium_bridge("execute_blueprint", Some("req-eb".into()), 5, 6)
                    .expect("bridge");
                store
                    .record_agent_run("execute_blueprint", Some("req-eb".into()))
                    .expect("run");
            },
        )
        .await;

        let store = store.lock().expect("lock");
        let conn = store.connection();
        let phase: String = conn
            .query_row(
                "SELECT agent_phase FROM premium_bridge WHERE request_uuid = 'req-eb'",
                [],
                |row| row.get(0),
            )
            .expect("phase");
        assert_eq!(phase, "builder");
        let triage_runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_runs WHERE agent_phase = 'triage' AND request_uuid = 'req-eb'",
                [],
                |row| row.get(0),
            )
            .expect("triage runs");
        let builder_runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_runs WHERE agent_phase = 'builder' AND request_uuid = 'req-eb'",
                [],
                |row| row.get(0),
            )
            .expect("builder runs");
        assert_eq!(triage_runs, 0);
        assert_eq!(builder_runs, 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
