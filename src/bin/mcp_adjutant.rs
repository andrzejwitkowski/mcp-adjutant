use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

use mcp_adjutant::config_server::{
    load_or_default, resolve_config_path, run as run_config_server, static_root, ConfigServerState,
};
use mcp_adjutant::mcp_server;
use mcp_adjutant::AdjutantConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mcp_adjutant=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    let config_path = std::env::var("MCP_ADJUTANT_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| resolve_config_path(&AdjutantConfig::default()));

    let config = load_or_default(&config_path);
    let port = config.server_port;
    let shared = Arc::new(RwLock::new(config));

    let config_state = ConfigServerState {
        config: Arc::clone(&shared),
        config_path,
        static_root: static_root(),
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
