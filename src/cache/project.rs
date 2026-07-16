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
const GITIGNORE_ENTRY: &str = ".adjutant/";

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
];

/// MCP workspace root: job override, then thread override, then env, then walk up from cwd.
pub fn mcp_workspace_root() -> PathBuf {
    if let Some(root) = crate::metrics::current_job_context().and_then(|ctx| ctx.workspace_root) {
        return root;
    }
    if let Some(root) = THREAD_WORKSPACE_ROOT.with(|cell| cell.borrow().clone()) {
        return root;
    }
    std::env::var("MCP_ADJUTANT_PROJECT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let start = std::env::current_dir().unwrap_or_else(|_| manifest.clone());
            find_project_root(&start)
                .unwrap_or_else(|_| find_project_root(&manifest).unwrap_or(start))
        })
}

/// Parse optional `workspace_root` from MCP tool args (evaluate also accepts `project_path`).
/// Missing/empty → `Ok(None)`. Non-directory or missing path → `Err`.
pub fn parse_workspace_root_arg(args: &serde_json::Value) -> Result<Option<PathBuf>, String> {
    let raw = args
        .get("workspace_root")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            args.get("project_path")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });

    let Some(raw) = raw else {
        return Ok(None);
    };

    let path = PathBuf::from(raw);
    let meta = fs::metadata(&path).map_err(|err| {
        format!("workspace_root must be an existing directory ({raw}): {err}")
    })?;
    if !meta.is_dir() {
        return Err(format!("workspace_root must be a directory, got file: {raw}"));
    }
    Ok(Some(
        fs::canonicalize(&path).unwrap_or(path),
    ))
}

/// Shared MCP schema property for per-request project root.
pub fn workspace_root_schema_property() -> serde_json::Value {
    serde_json::json!({
        "workspace_root": {
            "type": "string",
            "description": "Absolute path of the project this job should operate on. Required when one MCP process serves multiple repos; defaults to MCP_ADJUTANT_PROJECT_ROOT / process cwd."
        }
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
    let adjutant_dir = project_root.join(ADJUTANT_DIR);

    fs::create_dir_all(&adjutant_dir)
        .map_err(|err| format!("failed to create {ADJUTANT_DIR} directory: {err}"))?;

    ensure_gitignore_entry(&project_root)?;

    let db_path = adjutant_dir.join(CACHE_DB_FILE);
    let conn = Connection::open(&db_path).map_err(|err| {
        format!(
            "failed to open SQLite database at {}: {err}",
            db_path.display()
        )
    })?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|err| format!("failed to enable SQLite foreign keys: {err}"))?;

    for migration in MIGRATIONS {
        conn.execute_batch(migration)
            .map_err(|err| format!("failed to run cache migration: {err}"))?;
    }

    Ok((project_root, conn))
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

fn ensure_gitignore_entry(project_root: &Path) -> Result<(), String> {
    let gitignore_path = project_root.join(".gitignore");
    if !gitignore_path.is_file() {
        return Ok(());
    }

    let contents = fs::read_to_string(&gitignore_path)
        .map_err(|err| format!("failed to read {}: {err}", gitignore_path.display()))?;

    let already_ignored = contents.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == ".adjutant" || trimmed == GITIGNORE_ENTRY
    });

    if already_ignored {
        return Ok(());
    }

    let updated = if contents.is_empty() {
        format!("{GITIGNORE_ENTRY}\n")
    } else if contents.ends_with('\n') {
        format!("{contents}{GITIGNORE_ENTRY}\n")
    } else {
        format!("{contents}\n{GITIGNORE_ENTRY}\n")
    };

    fs::write(&gitignore_path, updated)
        .map_err(|err| format!("failed to update {}: {err}", gitignore_path.display()))?;

    Ok(())
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
        std::env::set_var("MCP_ADJUTANT_PROJECT_ROOT", "/tmp/mcp-adjutant");
        assert_eq!(
            resolve_workspace_path("."),
            PathBuf::from("/tmp/mcp-adjutant/.")
        );
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
    }

    #[test]
    fn resolve_workspace_path_joins_relative_paths() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        std::env::set_var("MCP_ADJUTANT_PROJECT_ROOT", "/tmp/mcp-adjutant");
        assert_eq!(
            resolve_workspace_path("./src/cache/project.rs"),
            PathBuf::from("/tmp/mcp-adjutant/src/cache/project.rs")
        );
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
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
    async fn mcp_workspace_root_prefers_job_context_override() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        std::env::set_var("MCP_ADJUTANT_PROJECT_ROOT", "/tmp/mcp-adjutant-env-fallback");
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
        assert_eq!(
            mcp_workspace_root(),
            PathBuf::from("/tmp/mcp-adjutant-env-fallback")
        );
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
    }
}
