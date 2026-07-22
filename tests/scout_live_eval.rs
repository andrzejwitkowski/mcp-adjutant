//! One-off live scout/cache probe — run: cargo test -p mcp-adjutant --test scout_live_eval -- --nocapture
mod common;

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::{embedding_fixture_paths, open_cache_manager};
use mcp_adjutant::agent::{run_scout_with_cache, ScoutAgent, ScoutCacheOutcome};
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest};
use mcp_adjutant::{LocalEmbeddingEngine, SEMANTIC_SIMILARITY_THRESHOLD};

const PROMPTS: &[(&str, &str)] = &[
    (
        "How does run_scout_with_cache use SQLite embeddings for semantic cache lookup?",
        "Where does the scout cache flow persist and match vector embeddings?",
    ),
    (
        "When should ScoutAgent use ripgrep versus ast_calls?",
        "How does the scout pick between ripgrep and AST call-site lookup?",
    ),
];

struct NeverCalledClient;

impl LlmClient for NeverCalledClient {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        panic!("LLM should not run on cache hit");
    }
}

fn write_similarities(path: &Path) {
    let (model, tok) = embedding_fixture_paths();
    let engine = LocalEmbeddingEngine::new(&model, &tok).expect("engine");
    let mut out = File::create(path).expect("create sim file");
    writeln!(out, "threshold={SEMANTIC_SIMILARITY_THRESHOLD}").unwrap();
    for (a, b) in PROMPTS {
        let left = engine.generate(a).expect("embed a");
        let right = engine.generate(b).expect("embed b");
        let sim = LocalEmbeddingEngine::dot_product(&left, &right);
        writeln!(out, "similarity={sim:.4}").unwrap();
        writeln!(out, "  a={a}").unwrap();
        writeln!(out, "  b={b}").unwrap();
    }
}

fn seed_cache(repo: &Path) {
    let mut cache = open_cache_manager(repo);
    let insight = "## Scout insight\n\
`run_scout_with_cache` checks `ProjectCacheManager::try_get_valid_insight` (cosine ≥ 0.82) \
at cache_flow.rs:15 before running the scout loop. On `agent_completed` with file deps it \
calls `store_insight` into `.adjutant/cache.db` (queries.embedding BLOB + insights + code_nodes).";
    let dep = repo.join("src/agent/scout/cache_flow.rs");
    cache
        .store_insight(PROMPTS[0].0, insight, vec![dep])
        .expect("seed cache");
}

#[tokio::test]
async fn scout_live_eval_probe() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    write_similarities(Path::new("/tmp/scout_sim.txt"));
    seed_cache(repo);

    let cache = Arc::new(Mutex::new(open_cache_manager(repo)));
    let scout = ScoutAgent::new(NeverCalledClient);
    let outcome = run_scout_with_cache(&cache, &scout, PROMPTS[0].1, 5, true)
        .await
        .expect("paraphrase lookup");
    let ScoutCacheOutcome::Hit(_) = outcome else {
        panic!("expected cache hit");
    };
}

#[tokio::test]
async fn scout_live_eval_tool_pair_cache_hit() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cache = open_cache_manager(repo);
    let insight = "## ScoutAgent: ripgrep vs ast_calls\n\
Selection at tools.rs:1: ripgrep when location unknown; ast_calls when file+method known.";
    cache
        .store_insight(
            PROMPTS[1].0,
            insight,
            vec![repo.join("src/agent/scout/tools.rs")],
        )
        .expect("seed tool insight");

    let outcome = run_scout_with_cache(
        &Arc::new(Mutex::new(cache)),
        &ScoutAgent::new(NeverCalledClient),
        PROMPTS[1].1,
        5,
        true,
    )
    .await
    .expect("tool paraphrase lookup");
    let ScoutCacheOutcome::Hit(report) = outcome else {
        panic!("expected cache hit for tool paraphrase");
    };
    assert!(report.contains("ripgrep"));
}

#[tokio::test]
async fn handler_scout_context_returns_cache_hit_prefix() {
    use std::path::PathBuf;
    use std::sync::Arc;

    use mcp_adjutant::domain::AdjutantConfig;
    use mcp_adjutant::jobs::JobRegistry;
    use mcp_adjutant::mcp::{handle_query_job_status, handle_scout_context};
    use serde_json::json;

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::env::set_var("MCP_ADJUTANT_PROJECT_ROOT", repo);
    seed_cache(repo);

    let config_path = PathBuf::from("/Users/andrzej.witkowski/.config/mcp-adjutant/config.json");
    let config = Arc::new(
        AdjutantConfig::load_from_file(&config_path).unwrap_or_else(|_| AdjutantConfig::default()),
    );
    let registry = JobRegistry::default();
    let request_uuid = "handler-cache-hit-probe";

    handle_scout_context(
        json!({
            "query": PROMPTS[0].1,
            "request_uuid": request_uuid,
            "workspace_root": repo,
        }),
        config,
        &registry,
    )
    .await
    .expect("accept scout job");

    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let status_json =
            handle_query_job_status(json!({ "request_uuid": request_uuid }), &registry)
                .await
                .expect("poll status");
        if status_json.contains("\"terminal\": true") {
            assert!(
                status_json.contains("[CACHE HIT]"),
                "handler path should return cache hit, got: {status_json}"
            );
            return;
        }
    }
    panic!("handler scout job did not finish");
}
