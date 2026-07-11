use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};

mod common;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use mcp_adjutant::agent::{
    run_web_fetch_with_cache, WebCacheOutcome, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT,
};
use mcp_adjutant::cache::WebSourceSnapshot;
use mcp_adjutant::domain::WebFetcherProfile;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct NeverCalledClient;

impl LlmClient for NeverCalledClient {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        panic!("LLM should not run on semantic web cache hit");
    }
}

struct FinalizeOnlyClient(Mutex<VecDeque<LlmModelTurn>>);

impl LlmClient for FinalizeOnlyClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, WEB_FETCHER_SYSTEM_PROMPT);
        self.0
            .lock()
            .map_err(|_| "script client lock poisoned".to_string())?
            .pop_front()
            .ok_or_else(|| "script client out of responses".to_string())
    }
}

fn sample_sources() -> Vec<WebSourceSnapshot> {
    vec![WebSourceSnapshot {
        url: "https://example.com/tokio/docs".to_string(),
        content_sha256: "abc123".to_string(),
        fetched_at: 1_700_000_000,
    }]
}

#[tokio::test]
async fn web_cache_hit_skips_llm() {
    let project_root = unique_temp_project("web-cache-hit");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let stored_query = "latest tokio async runtime docs";
    let paraphrase_query = "tokio async runtime documentation";
    let report = "## Tokio\nUses async tasks and timers.";

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_web_report(stored_query, report, sample_sources())
        .expect("store web report");

    let outcome = run_web_fetch_with_cache(
        &Arc::new(Mutex::new(cache)),
        &WebFetcherAgent::new(NeverCalledClient, WebFetcherProfile::default()),
        paraphrase_query,
        3,
        604_800,
        0.78,
        true,
    )
    .await
    .expect("cache hit");

    let WebCacheOutcome::Hit(cached) = outcome else {
        panic!("expected cache hit");
    };
    assert!(cached.contains("Tokio"));

    fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn web_cache_does_not_store_finalize_without_sources() {
    let project_root = unique_temp_project("web-cache-no-sources");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));
    let query = "latest tokio docs";

    let outcome = run_web_fetch_with_cache(
        &cache,
        &WebFetcherAgent::new(
            FinalizeOnlyClient(Mutex::new(VecDeque::from([LlmModelTurn {
                content: Some("Done.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "finalize".to_string(),
                    arguments: serde_json::json!({
                        "report": "## Tokio\nNo search performed."
                    }),
                }],
            }]))),
            WebFetcherProfile::default(),
        ),
        query,
        3,
        604_800,
        0.78,
        true,
    )
    .await
    .expect("fresh report");

    let WebCacheOutcome::Fresh(report) = outcome else {
        panic!("expected fresh report");
    };
    assert!(report.contains("Tokio"));

    let cache = cache.lock().expect("cache lock");
    assert!(
        cache
            .try_get_valid_web_report(query, 604_800, 0.78)
            .expect("lookup")
            .is_none(),
        "finalize-only reports must not be cached"
    );

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn web_cache_rejects_reports_without_sources() {
    let project_root = unique_temp_project("web-cache-empty-sources");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_web_report("tokio docs", "## Tokio", vec![])
        .expect("store empty-source report");

    assert!(
        cache
            .try_get_valid_web_report("tokio docs", 604_800, 0.78)
            .expect("lookup")
            .is_none(),
        "reports without sources must not cache-hit"
    );

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn web_cache_rejects_local_mock_sources() {
    let project_root = unique_temp_project("web-cache-mock-url");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_web_report(
            "tokio docs",
            "## Tokio mock",
            vec![WebSourceSnapshot {
                url: "http://127.0.0.1:8765/tokio/docs".to_string(),
                content_sha256: "abc".to_string(),
                fetched_at: 1_700_000_000,
            }],
        )
        .expect("store mock report");

    assert!(
        cache
            .try_get_valid_web_report("tokio docs", 604_800, 0.78)
            .expect("lookup")
            .is_none(),
        "mock/local source URLs must not cache-hit"
    );

    fs::remove_dir_all(&project_root).ok();
}

#[test]
fn web_cache_uses_report_ttl_fast_path() {
    let project_root = unique_temp_project("web-cache-ttl");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_web_report("tokio docs", "## Tokio fast path", sample_sources())
        .expect("store web report");

    let hit = cache
        .try_get_valid_web_report("tokio docs", 604_800, 0.78)
        .expect("lookup")
        .expect("recent report should hit without re-fetch");
    assert!(hit.contains("fast path"));

    fs::remove_dir_all(&project_root).ok();
}
