use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

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
];

/// MCP workspace root: env override, then process cwd, then compile-time repo root.
pub fn mcp_workspace_root() -> PathBuf {
    std::env::var("MCP_ADJUTANT_PROJECT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        })
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
    use super::*;

    #[test]
    fn resolve_workspace_path_joins_dot() {
        std::env::set_var("MCP_ADJUTANT_PROJECT_ROOT", "/tmp/mcp-adjutant");
        assert_eq!(
            resolve_workspace_path("."),
            PathBuf::from("/tmp/mcp-adjutant/.")
        );
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
    }

    #[test]
    fn resolve_workspace_path_joins_relative_paths() {
        std::env::set_var("MCP_ADJUTANT_PROJECT_ROOT", "/tmp/mcp-adjutant");
        assert_eq!(
            resolve_workspace_path("./src/cache/project.rs"),
            PathBuf::from("/tmp/mcp-adjutant/src/cache/project.rs")
        );
        std::env::remove_var("MCP_ADJUTANT_PROJECT_ROOT");
    }
}
