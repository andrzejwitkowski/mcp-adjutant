use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use super::ScoutAgent;
use crate::agent::AgentLoopOrchestrator;
use crate::cache::ProjectCacheManager;
use crate::llm::LlmClient;

pub enum ScoutCacheOutcome {
    Hit(String),
    Fresh(String),
}

pub async fn run_scout_with_cache<C: LlmClient>(
    cache: &Mutex<ProjectCacheManager>,
    scout: &ScoutAgent<C>,
    query: &str,
    max_iterations: u32,
    read_cache: bool,
) -> Result<ScoutCacheOutcome, String> {
    if read_cache {
        if let Some(cached) = cache
            .lock()
            .map_err(|_| "cache manager lock poisoned".to_string())?
            .try_get_valid_insight(query)?
        {
            return Ok(ScoutCacheOutcome::Hit(cached));
        }
    }

    let result = AgentLoopOrchestrator::run(scout, query.to_string(), max_iterations).await?;

    if result.agent_completed && !result.touched_files.is_empty() {
        let files = dedupe_paths(result.touched_files);
        // ponytail: cache store is best-effort; scout report is returned even if SQLite write fails
        let _ = cache
            .lock()
            .map_err(|_| "cache manager lock poisoned".to_string())?
            .store_insight(query, &result.accumulated_data, files);
    }

    if result.is_finished {
        return Ok(ScoutCacheOutcome::Fresh(result.accumulated_data));
    }

    Ok(ScoutCacheOutcome::Fresh(format!(
        "Scout report (finished={}, iterations={}):\n{}",
        result.is_finished, result.iterations, result.accumulated_data
    )))
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}
