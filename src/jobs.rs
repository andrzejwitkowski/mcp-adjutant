use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle as ThreadJoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

pub const QUERY_JOB_STATUS_TOOL_NAME: &str = "query_job_status";

const STALL_HINT_AFTER: Duration = Duration::from_secs(90);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const TERMINAL_RETENTION: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Stalled,
}

#[derive(Debug, Clone)]
struct JobRecord {
    tool_name: String,
    status: JobStatus,
    result: Option<String>,
    error: Option<String>,
    created_at: Instant,
    updated_at: Instant,
    last_heartbeat: Instant,
}

struct HeartbeatHandle {
    stop: Arc<AtomicBool>,
    thread: Option<ThreadJoinHandle<()>>,
}

impl HeartbeatHandle {
    fn start(registry: JobRegistry, request_uuid: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut elapsed = Duration::ZERO;
            while !stop_flag.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(1));
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                elapsed += Duration::from_secs(1);
                if elapsed >= HEARTBEAT_INTERVAL {
                    elapsed = Duration::ZERO;
                    registry.heartbeat(&request_uuid);
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for HeartbeatHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // ponytail: detach heartbeat thread; join() would block a Tokio worker
        let _ = self.thread.take();
    }
}

#[derive(Clone)]
pub struct JobRegistry {
    inner: Arc<Mutex<HashMap<String, JobRecord>>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register(&self, request_uuid: &str, tool_name: &str) -> Result<(), String> {
        let mut jobs = self.inner.lock().expect("job registry lock");
        evict_stale_terminal_jobs(&mut jobs);
        if jobs.contains_key(request_uuid) {
            return Err(format!(
                "request_uuid already in use: {request_uuid}. Use a fresh UUID or poll query_job_status."
            ));
        }

        let now = Instant::now();
        jobs.insert(
            request_uuid.to_string(),
            JobRecord {
                tool_name: tool_name.to_string(),
                status: JobStatus::Queued,
                result: None,
                error: None,
                created_at: now,
                updated_at: now,
                last_heartbeat: now,
            },
        );
        Ok(())
    }

    pub fn set_running(&self, request_uuid: &str) {
        self.mutate_if_active(request_uuid, |job| {
            job.status = JobStatus::Running;
        });
    }

    pub fn heartbeat(&self, request_uuid: &str) {
        self.mutate_if_active(request_uuid, |job| {
            job.last_heartbeat = Instant::now();
        });
    }

    pub fn complete(&self, request_uuid: &str, result: String) {
        self.mutate_if_active(request_uuid, |job| {
            job.status = JobStatus::Completed;
            job.result = Some(result);
            job.error = None;
        });
    }

    pub fn fail(&self, request_uuid: &str, error: String) {
        self.mutate_if_active(request_uuid, |job| {
            job.status = JobStatus::Failed;
            job.error = Some(error);
            job.result = None;
        });
    }

    /// Terminal outcome once the job has finished. `None` while still queued/running.
    pub fn terminal_result(&self, request_uuid: &str) -> Option<Result<String, String>> {
        let jobs = self.inner.lock().expect("job registry lock");
        let job = jobs.get(request_uuid)?;
        match job.status {
            JobStatus::Completed => Some(Ok(job.result.clone().unwrap_or_default())),
            JobStatus::Failed => Some(Err(job.error.clone().unwrap_or_default())),
            _ => None,
        }
    }

    pub fn query(&self, request_uuid: &str) -> Result<Value, String> {
        let jobs = self.inner.lock().expect("job registry lock");
        let Some(job) = jobs.get(request_uuid) else {
            return Err(format!("unknown request_uuid: {request_uuid}"));
        };

        let possibly_stalled =
            job.status == JobStatus::Running && job.last_heartbeat.elapsed() > STALL_HINT_AFTER;

        Ok(json!({
            "request_uuid": request_uuid,
            "tool": job.tool_name,
            "status": job.status,
            "created_at_secs": elapsed_since_unix(job.created_at),
            "updated_at_secs": elapsed_since_unix(job.updated_at),
            "elapsed_secs": job.created_at.elapsed().as_secs(),
            "seconds_since_heartbeat": job.last_heartbeat.elapsed().as_secs(),
            "possibly_stalled": possibly_stalled,
            "result": job.result,
            "error": job.error,
            "terminal": matches!(job.status, JobStatus::Completed | JobStatus::Failed),
        }))
    }

    fn mutate_if_active(&self, request_uuid: &str, update: impl FnOnce(&mut JobRecord)) {
        let mut jobs = self.inner.lock().expect("job registry lock");
        if let Some(job) = jobs.get_mut(request_uuid) {
            if !matches!(job.status, JobStatus::Queued | JobStatus::Running) {
                return;
            }
            update(job);
            job.updated_at = Instant::now();
        }
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn evict_stale_terminal_jobs(jobs: &mut HashMap<String, JobRecord>) {
    jobs.retain(|_, job| {
        !matches!(job.status, JobStatus::Completed | JobStatus::Failed)
            || job.updated_at.elapsed() <= TERMINAL_RETENTION
    });
}

fn elapsed_since_unix(instant: Instant) -> u64 {
    let now = SystemTime::now();
    let reference = now.checked_sub(instant.elapsed()).unwrap_or(UNIX_EPOCH);
    reference
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn parse_request_uuid(args: &Value) -> Result<String, String> {
    args.get("request_uuid")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            "request_uuid is required: generate a new UUID v4, retry this tool call with the same arguments plus request_uuid, then poll query_job_status with that UUID until terminal=true. Do not fall back to native tools — this is a recoverable input error.".to_string()
        })
}

pub fn request_uuid_schema_property() -> Value {
    json!({
        "request_uuid": {
            "type": "string",
            "description": "Caller-generated UUID for this request. The tool awaits the job inline and returns the result directly. If the job exceeds the await timeout, it falls back to async mode and you poll query_job_status with this UUID until terminal=true."
        }
    })
}

pub fn accepted_job_response(request_uuid: &str, tool_name: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "request_uuid": request_uuid,
        "tool": tool_name,
        "status": "accepted",
        "message": format!(
            "Job still running (exceeded inline await timeout). Poll `{QUERY_JOB_STATUS_TOOL_NAME}` with request_uuid until terminal=true."
        ),
    }))
    .expect("serialize accepted job response")
}

pub fn query_job_status_schema() -> Value {
    json!({
        "name": QUERY_JOB_STATUS_TOOL_NAME,
        "description": "Poll async adjutant job status by request_uuid. Most jobs return their result inline; use this only when a tool response says the job exceeded the await timeout, or to check liveness/staleness of a running job. possibly_stalled=true is advisory only; keep polling until terminal=true.",
        "input_schema": {
            "type": "object",
            "properties": {
                "request_uuid": {
                    "type": "string",
                    "description": "UUID from the original tool call."
                }
            },
            "required": ["request_uuid"]
        }
    })
}

pub async fn run_tracked_job<F, Fut>(
    registry: JobRegistry,
    request_uuid: String,
    tool_name: String,
    workspace_root: Option<std::path::PathBuf>,
    work: F,
) where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
{
    registry.set_running(&request_uuid);
    registry.heartbeat(&request_uuid);
    let _heartbeat = HeartbeatHandle::start(registry.clone(), request_uuid.clone());

    let ctx = crate::metrics::JobContext {
        request_uuid: Some(request_uuid.clone()),
        mcp_tool: Some(tool_name.clone()),
        workspace_root,
    };

    let work_handle = tokio::spawn(crate::metrics::with_job_context_async(ctx, work));
    let join_result = work_handle.await;

    match join_result {
        Ok(Ok(result)) => {
            crate::metrics::record_agent_run(&tool_name, Some(request_uuid.clone()));
            registry.complete(&request_uuid, result)
        }
        Ok(Err(error)) => {
            crate::metrics::record_agent_run(&tool_name, Some(request_uuid.clone()));
            registry.fail(&request_uuid, error)
        }
        Err(join_error) if join_error.is_panic() => {
            registry.fail(&request_uuid, "Job panicked during execution".to_string());
        }
        Err(join_error) => {
            registry.fail(&request_uuid, format!("Job task cancelled: {join_error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_rejects_duplicate_uuid() {
        let registry = JobRegistry::new();
        registry.register("job-1", "scout_context").expect("first");
        let err = registry
            .register("job-1", "scout_context")
            .expect_err("duplicate");
        assert!(err.contains("already in use"));
    }

    #[test]
    fn query_reports_possibly_stalled_without_mutating_status() {
        let registry = JobRegistry::new();
        registry
            .register("job-1", "scout_context")
            .expect("register");
        {
            let mut jobs = registry.inner.lock().expect("lock");
            let job = jobs.get_mut("job-1").expect("job");
            job.status = JobStatus::Running;
            job.last_heartbeat = Instant::now() - STALL_HINT_AFTER - Duration::from_secs(1);
        }

        let status = registry.query("job-1").expect("query");
        assert_eq!(status["status"], "running");
        assert_eq!(status["possibly_stalled"], true);
        assert_eq!(status["terminal"], false);
    }

    #[test]
    fn complete_makes_job_terminal() {
        let registry = JobRegistry::new();
        registry
            .register("job-1", "scout_context")
            .expect("register");
        registry.complete("job-1", "done".to_string());

        let status = registry.query("job-1").expect("query");
        assert_eq!(status["status"], "completed");
        assert_eq!(status["result"], "done");
        assert_eq!(status["terminal"], true);
    }

    #[test]
    fn complete_does_not_overwrite_terminal_failure() {
        let registry = JobRegistry::new();
        registry
            .register("job-1", "scout_context")
            .expect("register");
        registry.fail("job-1", "first failure".to_string());
        registry.complete("job-1", "late success".to_string());

        let status = registry.query("job-1").expect("query");
        assert_eq!(status["status"], "failed");
        assert_eq!(status["error"], "first failure");
        assert_eq!(status["result"], Value::Null);
    }

    #[test]
    fn register_evicts_stale_terminal_jobs() {
        let registry = JobRegistry::new();
        registry
            .register("old-job", "scout_context")
            .expect("register");
        registry.complete("old-job", "done".to_string());
        {
            let mut jobs = registry.inner.lock().expect("lock");
            let job = jobs.get_mut("old-job").expect("job");
            job.updated_at = Instant::now() - TERMINAL_RETENTION - Duration::from_secs(1);
        }

        registry
            .register("new-job", "scout_context")
            .expect("register");

        assert!(registry.query("old-job").is_err());
        assert_eq!(
            registry.query("new-job").expect("query")["status"],
            "queued"
        );
    }

    #[tokio::test]
    async fn run_tracked_job_marks_failed_on_panic() {
        let registry = JobRegistry::new();
        registry
            .register("job-1", "scout_context")
            .expect("register");

        run_tracked_job(
            registry.clone(),
            "job-1".to_string(),
            "scout_context".to_string(),
            None,
            || async {
                panic!("boom");
            },
        )
        .await;

        let status = registry.query("job-1").expect("query");
        assert_eq!(status["status"], "failed");
        assert_eq!(status["terminal"], true);
        assert!(status["error"]
            .as_str()
            .unwrap_or_default()
            .contains("panicked"));
    }
}
