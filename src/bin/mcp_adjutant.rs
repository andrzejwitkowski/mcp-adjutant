use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

use mcp_adjutant::cache::resolve_config_cache_root;
use mcp_adjutant::config_server::{
    load_or_default, resolve_config_path, run as run_config_server, static_root, ConfigServerState,
};
use mcp_adjutant::mcp_server;
use mcp_adjutant::metrics::{self, MetricsStore};
use mcp_adjutant::AdjutantConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mcp_adjutant=info".parse()?))
        .with_writer(std::io::stderr)
        // ponytail: ANSI stderr looks like protocol noise to Cursor's MCP client
        .with_ansi(false)
        .init();

    let config_path = std::env::var("MCP_ADJUTANT_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| resolve_config_path(&AdjutantConfig::default()));

    let config = load_or_default(&config_path);
    let port = config.server_port;
    let checksum = mcp_adjutant::llm::config_checksum(&config);
    tracing::info!("loaded config checksum={checksum}");
    if mcp_adjutant::llm::skip_preflight() {
        tracing::warn!("MCP_ADJUTANT_SKIP_PREFLIGHT set — skipping startup provider preflight");
    } else {
        match mcp_adjutant::llm::preflight_config(&config) {
            Ok(cs) => tracing::info!("startup preflight ok checksum={cs}"),
            Err(err) => tracing::warn!("startup preflight failed (jobs may fail closed): {err}"),
        }
    }
    let shared = Arc::new(RwLock::new(config));

    let metrics_db_path = metrics::resolve_metrics_db_path(&config_path);
    let metrics_store = Arc::new(std::sync::Mutex::new(
        MetricsStore::open(&metrics_db_path).map_err(|err| format!("metrics db: {err}"))?,
    ));
    let session_id = metrics::new_session_id();
    metrics::init(session_id, Arc::clone(&metrics_store));

    let cache_project_root = resolve_config_cache_root();
    tracing::info!(
        "config UI cache project root: {}",
        cache_project_root.display()
    );

    let config_state = ConfigServerState {
        config: Arc::clone(&shared),
        config_path,
        static_root: static_root(),
        cache_project_root,
        metrics: metrics_store,
    };

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("config server runtime");
        runtime.block_on(async {
            if let Err(err) = run_config_server(config_state, port).await {
                tracing::error!("config server stopped: {err}");
            }
        });
    });

    mcp_server::run_stdio(shared)?;
    Ok(())
}
