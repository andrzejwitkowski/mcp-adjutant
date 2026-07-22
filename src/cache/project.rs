use std::cell::RefCell;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

thread_local! {
    // ponytail: bridge JobContext into spawn_blocking (task_local does not cross threads)
    static THREAD_WORKSPACE_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Run `f` with a thread-local workspace root (for blocking threads outside tokio task_local).
pub fn with_thread_workspace_root<R>(root: PathBuf, f: impl FnOnce() -> R) -> R {
    THREAD_WORKSPACE_ROOT.with(|cell| {
        let prev = cell.replace(Some(root));
        let out = f();
        *cell.borrow_mut() = prev;
        out
    })
}

pub const ADJUTANT_DIR: &str = ".adjutant";
const CACHE_DB_FILE: &str = "cache.db";

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS queries (
        id TEXT PRIMARY KEY,
        raw_text TEXT NOT NULL,
        embedding BLOB
    );",
    "CREATE TABLE IF NOT EXISTS insights (
        id TEXT PRIMARY KEY,
        content TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS code_nodes (
        id TEXT PRIMARY KEY,
        file_path TEXT NOT NULL,
        last_known_git_sha TEXT,
        last_known_mtime INTEGER NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS insight_dependencies (
        insight_id TEXT,
        code_node_id TEXT,
        PRIMARY KEY (insight_id, code_node_id),
        FOREIGN KEY(insight_id) REFERENCES insights(id) ON DELETE CASCADE,
        FOREIGN KEY(code_node_id) REFERENCES code_nodes(id) ON DELETE CASCADE
    );",
    "CREATE TABLE IF NOT EXISTS agent_evaluations (
        id TEXT PRIMARY KEY,
        agent_name TEXT NOT NULL,
        original_task TEXT NOT NULL,
        agent_output TEXT NOT NULL,
        score INTEGER NOT NULL,
        feedback_notes TEXT NOT NULL,
        desired_output TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS web_queries (
        id TEXT PRIMARY KEY,
        raw_text TEXT NOT NULL,
        embedding BLOB
    );",
    "CREATE TABLE IF NOT EXISTS web_reports (
        id TEXT PRIMARY KEY,
        content TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS web_sources (
        id TEXT PRIMARY KEY,
        url TEXT NOT NULL,
        content_sha256 TEXT NOT NULL,
        fetched_at INTEGER NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS web_fetch_dependencies (
        report_id TEXT,
        source_id TEXT,
        PRIMARY KEY (report_id, source_id),
        FOREIGN KEY(report_id) REFERENCES web_reports(id) ON DELETE CASCADE,
        FOREIGN KEY(source_id) REFERENCES web_sources(id) ON DELETE CASCADE
    );",
    "ALTER TABLE agent_evaluations ADD COLUMN desired_output TEXT NOT NULL DEFAULT '';",
];

/// MCP workspace root: job override, then thread override, then env, then walk up from cwd.
pub fn mcp_workspace_root() -> PathBuf {
    if let Some(root) = crate::metrics::current_job_context().and_then(|ctx| ctx.workspace_root) {
        return root;
    }
    if let Some(root) = THREAD_WORKSPACE_ROOT.with(|cell| cell.borrow().clone()) {
        return root;
    }
    resolve_default_workspace_root()
}

/// Stable project root for the config UI / cache API (pinned at MCP process start).
/// Ignores per-job overrides and invalid/unexpanded `MCP_ADJUTANT_PROJECT_ROOT` values.
pub fn resolve_config_cache_root() -> PathBuf {
    resolve_default_workspace_root()
}

fn resolve_default_workspace_root() -> PathBuf {
    if let Ok(raw) = std::env::var("MCP_ADJUTANT_PROJECT_ROOT") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && !trimmed.contains("${") {
            let path = PathBuf::from(trimmed);
            if path.is_dir() {
                return fs::canonicalize(&path).unwrap_or(path);
            }
        }
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let start = std::env::current_dir().unwrap_or_else(|_| manifest.clone());
    find_project_root(&start).unwrap_or_else(|_| find_project_root(&manifest).unwrap_or(start))
}

/// Parse optional `workspace_root` from MCP tool args (evaluate also accepts `project_path`).
/// Missing/empty → `Ok(None)`. Non-directory or missing path → `Err`.
pub fn parse_workspace_root_arg(args: &serde_json::Value) -> Result<Option<PathBuf>, String> {
    let raw = ["workspace_root", "project_path"]
        .into_iter()
        .find_map(|key| {
            args.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });

    let Some(raw) = raw else {
        return Ok(None);
    };

    let path = PathBuf::from(raw);
    let meta = fs::metadata(&path)
        .map_err(|err| format!("workspace_root must be an existing directory ({raw}): {err}"))?;
    if !meta.is_dir() {
        return Err(format!(
            "workspace_root must be a directory, got file: {raw}"
        ));
    }
    Ok(Some(fs::canonicalize(&path).unwrap_or(path)))
}

/// Require absolute `workspace_root` (or legacy `project_path`) for cache-writing MCP tools.
pub fn require_workspace_root_arg(args: &serde_json::Value) -> Result<PathBuf, String> {
    parse_workspace_root_arg(args)?
        .ok_or_else(|| "workspace_root is required (absolute path of the open project)".to_string())
}

/// Shared MCP schema property object for per-request project root (place under `properties.workspace_root`).
pub fn workspace_root_schema_property() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "description": "Absolute path of the project this job must operate on. Required on every tool except query_job_status when one MCP process serves multiple repos."
    })
}

/// Resolve a path under [`mcp_workspace_root`], rejecting `..` escapes.
pub fn resolve_workspace_path_bounded(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    use std::path::Component;

    let root = mcp_workspace_root();
    let path = path.as_ref();
    let stripped = path
        .to_string_lossy()
        .strip_prefix("./")
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let p = Path::new(&stripped);

    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }

    let mut out = root.clone();
    for comp in p.components() {
        match comp {
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                if out == root || !out.pop() {
                    return Err("file_path must stay within the workspace".to_string());
                }
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err("file_path must stay within the workspace".to_string());
            }
        }
    }
    if !out.starts_with(&root) {
        return Err("file_path must stay within the workspace".to_string());
    }
    Ok(out)
}

/// Resolve a relative path against [`mcp_workspace_root`].
pub fn resolve_workspace_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let stripped = path
        .to_string_lossy()
        .strip_prefix("./")
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let p = Path::new(&stripped);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        mcp_workspace_root().join(p)
    }
}

/// Opens (or creates) the per-project SQLite cache without loading the embedding engine.
pub fn open_cache_connection(start_dir: &Path) -> Result<(PathBuf, Connection), String> {
    prepare_project_cache(start_dir)
}

pub fn prepare_project_cache(start_dir: &Path) -> Result<(PathBuf, Connection), String> {
    let project_root = find_project_root(start_dir)?;
    // ponytail: cache lives under XDG, not repo/.adjutant (avoids dirty diffs / gitignore churn)
    let adjutant_dir = external_cache_dir(&project_root)?;

    fs::create_dir_all(&adjutant_dir)
        .map_err(|err| format!("failed to create cache directory: {err}"))?;

    let db_path = adjutant_dir.join(CACHE_DB_FILE);
    // One-shot migrate from legacy in-repo .adjutant/cache.db if present and external is empty.
    let legacy = project_root.join(ADJUTANT_DIR).join(CACHE_DB_FILE);
    if !db_path.exists() && legacy.exists() {
        if let Err(err) = fs::copy(&legacy, &db_path) {
            tracing::warn!(
                "failed to migrate legacy cache {} → {}: {err}",
                legacy.display(),
                db_path.display()
            );
        }
    }

    let conn = Connection::open(&db_path).map_err(|err| {
        format!(
            "failed to open SQLite database at {}: {err}",
            db_path.display()
        )
    })?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|err| format!("failed to enable SQLite foreign keys: {err}"))?;

    for migration in MIGRATIONS {
        if let Err(err) = conn.execute_batch(migration) {
            // ponytail: ALTER ADD COLUMN re-runs every open; ignore duplicate-column on existing DBs
            let msg = err.to_string();
            if msg.contains("duplicate column name") {
                continue;
            }
            return Err(format!("failed to run cache migration: {err}"));
        }
    }

    super::agent_names::backfill_evaluation_agent_names(&conn)?;

    Ok((project_root, conn))
}

fn external_cache_dir(project_root: &Path) -> Result<PathBuf, String> {
    let canon = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let key = hash_query_text(&canon.to_string_lossy());
    let base = dirs_cache_home()
        .join("mcp-adjutant")
        .join("projects")
        .join(&key[..16.min(key.len())]);
    Ok(base)
}

/// Absolute path to the SQLite file for a project (outside the repo).
pub fn project_cache_db_path(project_root: &Path) -> Result<PathBuf, String> {
    Ok(external_cache_dir(project_root)?.join(CACHE_DB_FILE))
}

fn dirs_cache_home() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    home::home_dir()
        .map(|h| h.join(".cache"))
        .unwrap_or_else(|| PathBuf::from(".cache"))
}

pub fn hash_query_text(query_text: &str) -> String {
    let digest = Sha256::digest(query_text.as_bytes());
    digest
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

pub fn normalize_relative_path(project_root: &Path, file_path: &Path) -> Result<String, String> {
    let absolute_path = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        project_root.join(file_path)
    };

    let absolute_path = fs::canonicalize(&absolute_path).map_err(|err| {
        format!(
            "failed to resolve source file {}: {err}",
            absolute_path.display()
        )
    })?;

    let project_root = fs::canonicalize(project_root)
        .map_err(|err| format!("failed to canonicalize project root: {err}"))?;

    let relative = absolute_path.strip_prefix(&project_root).map_err(|_| {
        format!(
            "file {} is outside project root {}",
            absolute_path.display(),
            project_root.display()
        )
    })?;

    Ok(path_to_posix_string(relative))
}

pub fn current_unix_timestamp() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .map_err(|err| format!("system clock is before UNIX epoch: {err}"))
}

pub fn find_project_root(start_dir: &Path) -> Result<PathBuf, String> {
    let start_dir = fs::canonicalize(start_dir)
        .map_err(|err| format!("failed to canonicalize {}: {err}", start_dir.display()))?;

    let start_display = start_dir.display().to_string();
    let mut current = start_dir;

    loop {
        if current.join("Cargo.toml").is_file() || current.join(".git").exists() {
            return Ok(current);
        }

        if !current.pop() {
            return Err(format!("could not find project root from {start_display}"));
        }
    }
}

fn path_to_posix_string(path: &Path) -> String {
    let mut normalized = String::new();

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => normalized.push_str("../"),
            Component::Normal(part) => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(&part.to_string_lossy());
            }
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_workspace_path_joins_dot() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let dir = std::env::temp_dir().join(format!(
            "mcp-ws-dot-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let root = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        std::env::set_var(
            "MCP_ADJUTANT_PROJECT_ROOT",
            root.to_string_lossy().to_string(),
        );
        assert_eq!(resolve_workspace_path("."), root.join("."));
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_workspace_path_joins_relative_paths() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let dir = std::env::temp_dir().join(format!(
            "mcp-ws-rel-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let root = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        std::env::set_var(
            "MCP_ADJUTANT_PROJECT_ROOT",
            root.to_string_lossy().to_string(),
        );
        assert_eq!(
            resolve_workspace_path("./src/cache/project.rs"),
            root.join("src/cache/project.rs")
        );
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_config_cache_root_ignores_unexpanded_template_env() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        std::env::set_var("MCP_ADJUTANT_PROJECT_ROOT", "${workspaceFolder}");
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| manifest.clone());
        let original = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&home).expect("chdir home");
        let root = resolve_config_cache_root();
        std::env::set_current_dir(original).expect("restore cwd");
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
        assert_eq!(root, manifest);
    }

    #[test]
    fn resolve_config_cache_root_uses_valid_env_directory() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let dir = std::env::temp_dir().join(format!(
            "mcp-config-root-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let expected = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        std::env::set_var(
            "MCP_ADJUTANT_PROJECT_ROOT",
            dir.to_string_lossy().to_string(),
        );
        let root = resolve_config_cache_root();
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(root, expected);
    }

    #[test]
    fn mcp_workspace_root_walks_up_from_target_dir() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target_dir = manifest.join("target");
        fs::create_dir_all(&target_dir).expect("create target dir");
        let original = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&target_dir).expect("chdir target");
        let root = mcp_workspace_root();
        std::env::set_current_dir(original).expect("restore cwd");
        assert_eq!(root, manifest);
    }

    #[test]
    fn parse_workspace_root_arg_missing_is_none() {
        let args = serde_json::json!({});
        assert_eq!(parse_workspace_root_arg(&args).expect("ok"), None);
    }

    #[test]
    fn require_workspace_root_arg_rejects_missing() {
        let args = serde_json::json!({});
        let err = require_workspace_root_arg(&args).expect_err("required");
        assert!(err.contains("workspace_root is required"), "{err}");
    }

    #[test]
    fn parse_workspace_root_arg_rejects_missing_path() {
        let args = serde_json::json!({ "workspace_root": "/tmp/mcp-adjutant-no-such-dir-xyz" });
        let err = parse_workspace_root_arg(&args).expect_err("missing");
        assert!(err.contains("workspace_root"), "{err}");
    }

    #[test]
    fn parse_workspace_root_arg_accepts_directory() {
        let dir = std::env::temp_dir().join(format!(
            "mcp-ws-root-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let args = serde_json::json!({ "workspace_root": dir.to_string_lossy() });
        let got = parse_workspace_root_arg(&args).expect("ok").expect("some");
        let _ = fs::remove_dir_all(&dir);
        assert!(got.is_absolute());
    }

    #[test]
    fn parse_workspace_root_arg_accepts_project_path_alias() {
        let dir = std::env::temp_dir().join(format!(
            "mcp-ws-alias-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let args = serde_json::json!({ "project_path": dir.to_string_lossy() });
        let got = parse_workspace_root_arg(&args).expect("ok").expect("some");
        let _ = fs::remove_dir_all(&dir);
        assert!(got.is_absolute());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // ENV_TEST_LOCK must span env mutation + await
    async fn mcp_workspace_root_prefers_job_context_override() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let env_fallback = std::env::temp_dir().join(format!(
            "mcp-env-fallback-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&env_fallback).expect("mkdir env fallback");
        let expected_fallback =
            fs::canonicalize(&env_fallback).unwrap_or_else(|_| env_fallback.clone());
        std::env::set_var(
            "MCP_ADJUTANT_PROJECT_ROOT",
            env_fallback.to_string_lossy().to_string(),
        );
        let override_root = PathBuf::from("/tmp/mcp-adjutant-job-override");
        crate::metrics::with_job_context_async(
            crate::metrics::JobContext {
                request_uuid: Some("ws-1".into()),
                mcp_tool: Some("scout_context".into()),
                workspace_root: Some(override_root.clone()),
            },
            || async {
                assert_eq!(mcp_workspace_root(), override_root);
            },
        )
        .await;
        assert_eq!(mcp_workspace_root(), expected_fallback);
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
        let _ = fs::remove_dir_all(&env_fallback);
    }
}
