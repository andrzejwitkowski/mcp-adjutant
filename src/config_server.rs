use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use socket2::{Domain, Socket, Type};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower_http::services::{ServeDir, ServeFile};

use crate::cache::{
    list_evaluations_page, load_cache_snapshot, mcp_workspace_root, open_cache_connection,
    CacheSnapshot, EvaluationsPage, EVALUATIONS_PAGE_SIZE,
};
use crate::domain::AdjutantConfig;
use crate::error::AdjutantConfigError;

#[derive(Clone)]
pub struct ConfigServerState {
    pub config: Arc<RwLock<AdjutantConfig>>,
    pub config_path: PathBuf,
    pub static_root: PathBuf,
}

pub async fn run(state: ConfigServerState, port: u16) -> Result<(), String> {
    let index_file = state.static_root.join("index.html");
    let serve_dir = ServeDir::new(&state.static_root).not_found_service(ServeFile::new(index_file));

    let app = Router::new()
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/evaluations", get(get_evaluations))
        .route("/api/cache", get(get_cache))
        .fallback_service(serve_dir)
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = bind_reuse_addr(addr)
        .await
        .map_err(|err| format!("config server bind failed on {addr}: {err}"))?;

    tracing::debug!("config UI listening on http://{addr}");
    axum::serve(listener, app)
        .await
        .map_err(|err| format!("config server failed: {err}"))
}

async fn get_config(State(state): State<ConfigServerState>) -> Json<AdjutantConfig> {
    Json(state.config.read().await.clone())
}

async fn put_config(
    State(state): State<ConfigServerState>,
    Json(mut incoming): Json<AdjutantConfig>,
) -> Result<Json<AdjutantConfig>, AppError> {
    let mut config = state.config.write().await;
    for (phase, profile) in incoming.phases.drain() {
        config.phases.insert(phase, profile);
    }
    config.server_port = incoming.server_port;
    config.storage_path = incoming.storage_path;
    config.triage_overrides = incoming.triage_overrides;

    config
        .save_to_file(&state.config_path)
        .map_err(AppError::from)?;

    Ok(Json(config.clone()))
}

fn open_workspace_cache() -> Result<(std::path::PathBuf, rusqlite::Connection), String> {
    open_cache_connection(&mcp_workspace_root())
}

async fn get_evaluations(
    State(_state): State<ConfigServerState>,
    Query(query): Query<EvaluationsQuery>,
) -> Result<Json<EvaluationsPage>, CacheApiError> {
    let (_, conn) = open_workspace_cache().map_err(CacheApiError::from)?;
    let page = list_evaluations_page(&conn, query.page, EVALUATIONS_PAGE_SIZE)
        .map_err(CacheApiError::from)?;
    Ok(Json(page))
}

#[derive(Debug, serde::Deserialize)]
struct EvaluationsQuery {
    #[serde(default = "default_evaluations_page")]
    page: u32,
}

fn default_evaluations_page() -> u32 {
    1
}

async fn get_cache(
    State(_state): State<ConfigServerState>,
) -> Result<Json<CacheSnapshot>, CacheApiError> {
    let (project_root, conn) = open_workspace_cache().map_err(CacheApiError::from)?;
    let snapshot = load_cache_snapshot(&conn, &project_root).map_err(CacheApiError::from)?;
    Ok(Json(snapshot))
}

#[derive(Debug)]
struct AppError(AdjutantConfigError);

impl From<AdjutantConfigError> for AppError {
    fn from(error: AdjutantConfigError) -> Self {
        Self(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save config: {}", self.0),
        )
            .into_response()
    }
}

#[derive(Debug)]
struct CacheApiError(String);

impl From<String> for CacheApiError {
    fn from(error: String) -> Self {
        Self(error)
    }
}

impl IntoResponse for CacheApiError {
    fn into_response(self) -> Response {
        let status = if self.0.contains("could not find project root") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        (status, self.0).into_response()
    }
}

pub fn static_root() -> PathBuf {
    std::env::var("MCP_ADJUTANT_STATIC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend/dist"))
}

pub fn resolve_config_path(config: &AdjutantConfig) -> PathBuf {
    let path = PathBuf::from(&config.storage_path);
    if path.as_os_str().is_empty() {
        return AdjutantConfig::default().storage_path.into();
    }
    path
}

pub fn load_or_default(path: &Path) -> AdjutantConfig {
    match AdjutantConfig::load_from_file(path) {
        Ok(mut config) => {
            config.merge_missing_from_defaults();
            config
        }
        Err(AdjutantConfigError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            let config = AdjutantConfig::default();
            let _ = config.save_to_file(path);
            config
        }
        Err(_) => AdjutantConfig::default(),
    }
}

async fn bind_reuse_addr(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    TcpListener::from_std(socket.into())
}
