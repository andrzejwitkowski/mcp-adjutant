use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct JobContext {
    pub request_uuid: Option<String>,
    pub mcp_tool: Option<String>,
    /// Per-job project root override for multi-repo MCP processes.
    pub workspace_root: Option<PathBuf>,
}

tokio::task_local! {
    static JOB_CTX: JobContext;
}

pub async fn with_job_context_async<F, Fut, R>(ctx: JobContext, work: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    JOB_CTX.scope(ctx, work()).await
}

pub fn current_job_context() -> Option<JobContext> {
    JOB_CTX.try_with(|ctx| ctx.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn job_context_is_task_local() {
        with_job_context_async(
            JobContext {
                request_uuid: Some("job-1".to_string()),
                mcp_tool: Some("scout_context".to_string()),
                workspace_root: None,
            },
            || async {
                let ctx = current_job_context().expect("context");
                assert_eq!(ctx.request_uuid.as_deref(), Some("job-1"));
            },
        )
        .await;
        assert!(current_job_context().is_none());
    }
}
