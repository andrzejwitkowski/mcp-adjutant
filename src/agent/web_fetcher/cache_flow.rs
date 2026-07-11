use std::sync::Mutex;

use crate::cache::{ProjectCacheManager, WebSourceSnapshot};
use crate::tools::web_fetch::FetchedPage;

use super::WebFetcherAgent;

pub enum WebCacheOutcome {
    Hit(String),
    Fresh(String),
}

/// Run the web fetcher with a semantic cache.
/// 1. Check cache for a semantically similar phrase (TTL + content-hash validated).
/// 2. If miss, run the agent, then store the report + sources.
pub async fn run_web_fetch_with_cache<RC: crate::llm::LlmClient>(
    cache: &Mutex<ProjectCacheManager>,
    agent: &WebFetcherAgent<RC>,
    search_phrase: &str,
    max_iterations: u32,
    ttl_seconds: i64,
    use_cache: bool,
) -> Result<WebCacheOutcome, String> {
    if use_cache {
        let report = {
            let cache = cache
                .lock()
                .map_err(|_| "cache manager lock poisoned".to_string())?;
            cache.try_get_valid_web_report(search_phrase, ttl_seconds)?
        };
        if let Some(cached) = report {
            return Ok(WebCacheOutcome::Hit(cached));
        }
    }

    let result = crate::agent::AgentLoopOrchestrator::run(
        agent,
        search_phrase.to_string(),
        max_iterations,
    )
    .await?;

    if use_cache && result.agent_completed {
        let sources = collect_source_snapshots(agent);
        let _ = {
            let mut cache = cache
                .lock()
                .map_err(|_| "cache manager lock poisoned".to_string())?;
            cache.store_web_report(search_phrase, &result.accumulated_data, sources)
        };
    }

    Ok(WebCacheOutcome::Fresh(result.accumulated_data))
}

fn collect_source_snapshots<RC: crate::llm::LlmClient>(
    agent: &WebFetcherAgent<RC>,
) -> Vec<WebSourceSnapshot> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    agent
        .take_sources()
        .into_iter()
        .map(|page: FetchedPage| WebSourceSnapshot {
            url: page.url,
            content_sha256: page.content_sha256,
            fetched_at: now,
        })
        .collect()
}
