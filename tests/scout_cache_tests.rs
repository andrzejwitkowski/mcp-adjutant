use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use mcp_adjutant::agent::{
    run_scout_with_cache, ScoutAgent, ScoutCacheOutcome, SCOUT_SYSTEM_PROMPT,
};
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct ScriptClient(Mutex<VecDeque<LlmModelTurn>>);

impl LlmClient for ScriptClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, SCOUT_SYSTEM_PROMPT);
        self.0
            .lock()
            .map_err(|_| "script client lock poisoned".to_string())?
            .pop_front()
            .ok_or_else(|| "script client out of responses".to_string())
    }
}

struct NeverCalledClient;

impl LlmClient for NeverCalledClient {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        panic!("LLM should not run on semantic cache hit");
    }
}

fn jwt_fixture(project_root: &std::path::Path) -> (std::path::PathBuf, &'static str, &'static str) {
    let auth_file = project_root.join("src/auth.rs");
    fs::write(&auth_file, "pub fn jwt_routes() {}\n").expect("write auth source");
    (
        auth_file,
        "How to set up JWT authentication routing",
        "JWT auth middleware configuration",
    )
}

#[tokio::test]
async fn scout_cache_hit_skips_llm() {
    let project_root = unique_temp_project("scout-cache-hit");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let (auth_file, stored_query, paraphrase_query) = jwt_fixture(&project_root);
    let insight = "## Insight\nUse `jwt_routes` for JWT middleware.";

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(stored_query, insight, vec![auth_file])
        .expect("store insight");

    let outcome = run_scout_with_cache(
        &Arc::new(Mutex::new(cache)),
        &ScoutAgent::new(NeverCalledClient),
        paraphrase_query,
        5,
        true,
    )
    .await
    .expect("cache hit");

    let ScoutCacheOutcome::Hit(report) = outcome else {
        panic!("expected cache hit");
    };
    assert!(report.contains(insight));

    fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn scout_cache_stores_finished_report() {
    let project_root = unique_temp_project("scout-cache-store");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);
    let marker_file = project_root.join("marker.txt");
    fs::write(&marker_file, "alpha marker\n").expect("write marker");

    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));
    let query = "Find alpha marker";
    let outcome = run_scout_with_cache(
        &cache,
        &ScoutAgent::new(ScriptClient(Mutex::new(VecDeque::from([
            LlmModelTurn {
                content: Some("Read marker.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({
                        "file": marker_file,
                        "start": 1,
                        "end": 1
                    }),
                }],
            },
            LlmModelTurn {
                content: Some("Done.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "finalize".to_string(),
                    arguments: serde_json::json!({ "report": "## Scout\n- alpha marker" }),
                }],
            },
        ])))),
        query,
        5,
        true,
    )
    .await
    .expect("scout should finish");

    let ScoutCacheOutcome::Fresh(report) = outcome else {
        panic!("expected fresh scout run");
    };
    assert!(report.contains("alpha marker"));
    assert!(cache
        .lock()
        .expect("cache lock")
        .try_get_valid_insight(query)
        .expect("lookup")
        .expect("stored insight")
        .contains("alpha marker"));

    fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn scout_cache_invalidates_when_dependency_changes() {
    let project_root = unique_temp_project("scout-cache-invalidate");
    fs::create_dir_all(project_root.join("src")).expect("create src");
    write_demo_cargo_manifest(&project_root);

    let (auth_file, stored_query, paraphrase_query) = jwt_fixture(&project_root);
    let insight = "## Insight\nUse `jwt_routes` for JWT middleware.";

    let mut cache = open_cache_manager(&project_root);
    cache
        .store_insight(stored_query, insight, vec![auth_file.clone()])
        .expect("store insight");

    std::thread::sleep(Duration::from_millis(1100));
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&auth_file)
            .expect("open auth source"),
        "// changed"
    )
    .expect("modify auth source");

    assert!(cache
        .try_get_valid_insight(paraphrase_query)
        .expect("lookup after invalidation")
        .is_none());

    let outcome = run_scout_with_cache(
        &Arc::new(Mutex::new(cache)),
        &ScoutAgent::new(ScriptClient(Mutex::new(VecDeque::from([
            LlmModelTurn {
                content: Some("Read auth.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({
                        "file": auth_file,
                        "start": 1,
                        "end": 1
                    }),
                }],
            },
            LlmModelTurn {
                content: Some("Refreshed.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "finalize".to_string(),
                    arguments: serde_json::json!({ "report": "## Scout\n- refreshed insight" }),
                }],
            },
        ])))),
        paraphrase_query,
        5,
        true,
    )
    .await
    .expect("scout should rerun after invalidation");

    let ScoutCacheOutcome::Fresh(report) = outcome else {
        panic!("expected fresh scout run after invalidation");
    };
    assert!(report.contains("refreshed insight"));

    fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn scout_cache_skips_store_without_file_dependencies() {
    let project_root = unique_temp_project("scout-cache-no-deps");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));
    let query = "finalize only";
    let outcome = run_scout_with_cache(
        &cache,
        &ScoutAgent::new(ScriptClient(Mutex::new(VecDeque::from([LlmModelTurn {
            content: Some("Done.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({ "report": "## Scout\n- no deps" }),
            }],
        }])))),
        query,
        5,
        true,
    )
    .await
    .expect("scout should finish");

    let ScoutCacheOutcome::Fresh(report) = outcome else {
        panic!("expected fresh scout run");
    };
    assert!(report.contains("no deps"));
    assert!(cache
        .lock()
        .expect("cache lock")
        .try_get_valid_insight(query)
        .expect("lookup")
        .is_none());

    fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn scout_cache_returns_report_when_store_fails() {
    let project_root = unique_temp_project("scout-cache-store-fail");
    fs::create_dir_all(&project_root).expect("create project root");
    write_demo_cargo_manifest(&project_root);

    let outside_root = unique_temp_project("scout-cache-outside");
    let outside_file = outside_root.join("missing.rs");
    fs::create_dir_all(&outside_root).expect("create outside dir");
    fs::write(&outside_file, "fn outside() {}\n").expect("write outside file");

    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));
    let outcome = run_scout_with_cache(
        &cache,
        &ScoutAgent::new(ScriptClient(Mutex::new(VecDeque::from([
            LlmModelTurn {
                content: Some("Read outside.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({
                        "file": outside_file,
                        "start": 1,
                        "end": 1
                    }),
                }],
            },
            LlmModelTurn {
                content: Some("Done.".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "finalize".to_string(),
                    arguments: serde_json::json!({ "report": "## Scout\n- outside ok" }),
                }],
            },
        ])))),
        "outside dependency query",
        5,
        true,
    )
    .await
    .expect("scout report should succeed even when cache store fails");

    let ScoutCacheOutcome::Fresh(report) = outcome else {
        panic!("expected fresh scout run");
    };
    assert!(report.contains("outside ok"));

    fs::remove_dir_all(&project_root).ok();
    fs::remove_dir_all(&outside_root).ok();
}
