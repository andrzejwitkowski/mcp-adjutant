use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

pub const QUERY_JOB_STATUS_TOOL_NAME: &str = "query_job_status";

const STALL_AFTER: Duration = Duration::from_secs(90);

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
        self.mutate(request_uuid, |job| {
            job.status = JobStatus::Running;
        });
    }

    pub fn heartbeat(&self, request_uuid: &str) {
        self.mutate(request_uuid, |job| {
            job.last_heartbeat = Instant::now();
        });
    }

    pub fn complete(&self, request_uuid: &str, result: String) {
        self.mutate(request_uuid, |job| {
            job.status = JobStatus::Completed;
            job.result = Some(result);
            job.error = None;
        });
    }

    pub fn fail(&self, request_uuid: &str, error: String) {
        self.mutate(request_uuid, |job| {
            job.status = JobStatus::Failed;
            job.error = Some(error);
            job.result = None;
        });
    }

    pub fn query(&self, request_uuid: &str) -> Result<Value, String> {
        let mut jobs = self.inner.lock().expect("job registry lock");
        let Some(job) = jobs.get_mut(request_uuid) else {
            return Err(format!("unknown request_uuid: {request_uuid}"));
        };

        if job.status == JobStatus::Running && job.last_heartbeat.elapsed() > STALL_AFTER {
            job.status = JobStatus::Stalled;
            job.updated_at = Instant::now();
        }

        Ok(json!({
            "request_uuid": request_uuid,
            "tool": job.tool_name,
            "status": job.status,
            "created_at_secs": elapsed_since_unix(job.created_at),
            "updated_at_secs": elapsed_since_unix(job.updated_at),
            "elapsed_secs": job.created_at.elapsed().as_secs(),
            "seconds_since_heartbeat": job.last_heartbeat.elapsed().as_secs(),
            "result": job.result,
            "error": job.error,
            "terminal": matches!(
                job.status,
                JobStatus::Completed | JobStatus::Failed | JobStatus::Stalled
            ),
        }))
    }

    fn mutate(&self, request_uuid: &str, update: impl FnOnce(&mut JobRecord)) {
        let mut jobs = self.inner.lock().expect("job registry lock");
        if let Some(job) = jobs.get_mut(request_uuid) {
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
        .ok_or_else(|| "request_uuid is required".to_string())
}

pub fn request_uuid_schema_property() -> Value {
    json!({
        "request_uuid": {
            "type": "string",
            "description": "Caller-generated UUID for this request. The tool returns immediately; poll query_job_status with the same UUID until status is terminal (completed, failed, or stalled)."
        }
    })
}

pub fn accepted_job_response(request_uuid: &str, tool_name: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "request_uuid": request_uuid,
        "tool": tool_name,
        "status": "accepted",
        "message": format!(
            "Job accepted. Poll `{QUERY_JOB_STATUS_TOOL_NAME}` with request_uuid until terminal=true."
        ),
    }))
    .expect("serialize accepted job response")
}

pub fn query_job_status_schema() -> Value {
    json!({
        "name": QUERY_JOB_STATUS_TOOL_NAME,
        "description": "Poll async adjutant job status by request_uuid. Do not guess timeouts — call this until terminal=true.",
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

pub async fn run_tracked_job<F, Fut>(registry: JobRegistry, request_uuid: String, work: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
{
    registry.set_running(&request_uuid);
    let heartbeat_registry = registry.clone();
    let heartbeat_uuid = request_uuid.clone();
    let heartbeat = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            heartbeat_registry.heartbeat(&heartbeat_uuid);
        }
    });

    match work().await {
        Ok(result) => registry.complete(&request_uuid, result),
        Err(error) => registry.fail(&request_uuid, error),
    }

    heartbeat.abort();
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
    fn query_marks_stalled_without_heartbeat() {
        let registry = JobRegistry::new();
        registry
            .register("job-1", "scout_context")
            .expect("register");
        {
            let mut jobs = registry.inner.lock().expect("lock");
            let job = jobs.get_mut("job-1").expect("job");
            job.status = JobStatus::Running;
            job.last_heartbeat = Instant::now() - STALL_AFTER - Duration::from_secs(1);
        }

        let status = registry.query("job-1").expect("query");
        assert_eq!(status["status"], "stalled");
        assert_eq!(status["terminal"], true);
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
}
