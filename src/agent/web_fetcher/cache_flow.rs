use std::sync::Mutex;

use crate::agent::AgentLoopOrchestrator;
use crate::cache::{
    current_unix_timestamp, ProjectCacheManager, WebReportCacheLookup, WebSourceSnapshot,
};
use crate::llm::LlmClient;
use crate::tools::web_fetch::FetchedPage;

use super::WebFetcherAgent;

pub enum WebCacheOutcome {
    Hit(String),
    Fresh(String),
}

pub async fn run_web_fetch_with_cache<RC: LlmClient>(
    cache: &Mutex<ProjectCacheManager>,
    agent: &WebFetcherAgent<RC>,
    search_phrase: &str,
    max_iterations: u32,
    ttl_seconds: i64,
    cache_threshold: f32,
    read_cache: bool,
) -> Result<WebCacheOutcome, String> {
    if read_cache {
        let lookup = {
            let guard = cache
                .lock()
                .map_err(|_| "cache manager lock poisoned".to_string())?;
            guard.lookup_web_report_cache(search_phrase, ttl_seconds, cache_threshold)?
        };
        match lookup {
            WebReportCacheLookup::Fresh(cached) => {
                crate::metrics::record_cache_hit(crate::domain::AgentPhase::WebFetcher);
                return Ok(WebCacheOutcome::Hit(cached));
            }
            WebReportCacheLookup::Stale(pending) => {
                if crate::tools::web_fetch::web_sources_still_valid(&pending.sources) {
                    crate::metrics::record_cache_hit(crate::domain::AgentPhase::WebFetcher);
                    return Ok(WebCacheOutcome::Hit(pending.report_content));
                }
                cache
                    .lock()
                    .map_err(|_| "cache manager lock poisoned".to_string())?
                    .invalidate_stale_web_report(&pending.query_id)?;
            }
            WebReportCacheLookup::Miss => {}
        }
    }

    let result =
        AgentLoopOrchestrator::run(agent, search_phrase.to_string(), max_iterations).await?;

    if result.agent_completed {
        let sources = collect_source_snapshots(agent);
        if !sources.is_empty() {
            // ponytail: cache store is best-effort; web report is returned even if SQLite write fails
            let _ = cache
                .lock()
                .map_err(|_| "cache manager lock poisoned".to_string())?
                .store_web_report(search_phrase, &result.accumulated_data, sources);
        }
    }

    Ok(WebCacheOutcome::Fresh(result.accumulated_data))
}

fn collect_source_snapshots<RC: LlmClient>(agent: &WebFetcherAgent<RC>) -> Vec<WebSourceSnapshot> {
    let fetched_at = current_unix_timestamp().unwrap_or(0);
    agent
        .take_sources()
        .into_iter()
        .map(|page: FetchedPage| WebSourceSnapshot {
            url: page.url,
            content_sha256: page.content_sha256,
            fetched_at,
        })
        .collect()
}
