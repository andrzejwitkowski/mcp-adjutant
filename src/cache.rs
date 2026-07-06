use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

const ADJUTANT_DIR: &str = ".adjutant";
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
];

pub struct ProjectCacheManager {
    conn: Connection,
    project_root: PathBuf,
}

struct CodeNodeSnapshot {
    id: String,
    file_path: String,
    last_known_git_sha: Option<String>,
    last_known_mtime: i64,
}

impl ProjectCacheManager {
    /// Automatically detects the project root (walking up for `Cargo.toml` or `.git`)
    /// and initializes the per-project SQLite cache.
    pub fn new(start_dir: &Path) -> Result<Self, String> {
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

        Ok(Self { conn, project_root })
    }

    /// Returns an insight only when the query exists and every linked source file is up to date.
    pub fn try_get_valid_insight(&self, query_text: &str) -> Result<Option<String>, String> {
        let query_id = hash_query_text(query_text);

        let insight_content: Option<String> = self
            .conn
            .query_row(
                "SELECT content FROM insights WHERE id = ?1",
                params![query_id],
                |row| row.get(0),
            )
            .ok();

        let Some(content) = insight_content else {
            return Ok(None);
        };

        let dependencies = self.load_insight_dependencies(&query_id)?;
        for node in dependencies {
            if self.is_code_node_dirty(&node)? {
                self.invalidate_insight(&query_id)?;
                return Ok(None);
            }
        }

        Ok(Some(content))
    }

    /// Stores a new insight, snapshots associated files, and links them as dependencies.
    pub fn store_insight(
        &mut self,
        query_text: &str,
        insight_content: &str,
        associated_files: Vec<PathBuf>,
    ) -> Result<(), String> {
        let query_id = hash_query_text(query_text);
        let created_at = current_unix_timestamp()?;

        let transaction = self
            .conn
            .transaction()
            .map_err(|err| format!("failed to start cache transaction: {err}"))?;

        transaction
            .execute(
                "INSERT INTO queries (id, raw_text, embedding) VALUES (?1, ?2, NULL)
                 ON CONFLICT(id) DO UPDATE SET raw_text = excluded.raw_text",
                params![query_id, query_text],
            )
            .map_err(|err| format!("failed to store query: {err}"))?;

        transaction
            .execute(
                "INSERT INTO insights (id, content, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET content = excluded.content, created_at = excluded.created_at",
                params![query_id, insight_content, created_at],
            )
            .map_err(|err| format!("failed to store insight: {err}"))?;

        transaction
            .execute(
                "DELETE FROM insight_dependencies WHERE insight_id = ?1",
                params![query_id],
            )
            .map_err(|err| format!("failed to clear old insight dependencies: {err}"))?;

        for file_path in associated_files {
            let normalized_path = normalize_relative_path(&self.project_root, &file_path)?;
            let snapshot = capture_code_node_snapshot(&self.project_root, &normalized_path)?;

            transaction
                .execute(
                    "INSERT INTO code_nodes (id, file_path, last_known_git_sha, last_known_mtime)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET
                         file_path = excluded.file_path,
                         last_known_git_sha = excluded.last_known_git_sha,
                         last_known_mtime = excluded.last_known_mtime",
                    params![
                        snapshot.id,
                        snapshot.file_path,
                        snapshot.last_known_git_sha,
                        snapshot.last_known_mtime
                    ],
                )
                .map_err(|err| {
                    format!("failed to store code node {}: {err}", snapshot.file_path)
                })?;

            transaction
                .execute(
                    "INSERT OR IGNORE INTO insight_dependencies (insight_id, code_node_id)
                     VALUES (?1, ?2)",
                    params![query_id, snapshot.id],
                )
                .map_err(|err| format!("failed to link insight dependency: {err}"))?;
        }

        transaction
            .commit()
            .map_err(|err| format!("failed to commit cache transaction: {err}"))?;

        Ok(())
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    fn load_insight_dependencies(&self, insight_id: &str) -> Result<Vec<CodeNodeSnapshot>, String> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT cn.id, cn.file_path, cn.last_known_git_sha, cn.last_known_mtime
                 FROM code_nodes cn
                 INNER JOIN insight_dependencies dep ON dep.code_node_id = cn.id
                 WHERE dep.insight_id = ?1",
            )
            .map_err(|err| format!("failed to prepare dependency lookup: {err}"))?;

        let rows = statement
            .query_map(params![insight_id], |row| {
                Ok(CodeNodeSnapshot {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    last_known_git_sha: row.get(2)?,
                    last_known_mtime: row.get(3)?,
                })
            })
            .map_err(|err| format!("failed to query insight dependencies: {err}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to read insight dependencies: {err}"))
    }

    fn is_code_node_dirty(&self, node: &CodeNodeSnapshot) -> Result<bool, String> {
        let absolute_path = self.project_root.join(&node.file_path);

        let metadata = match fs::metadata(&absolute_path) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(true),
        };

        let current_mtime = file_mtime(&metadata)?;
        if current_mtime != node.last_known_mtime {
            return Ok(true);
        }

        match (
            &node.last_known_git_sha,
            git_blob_sha(&self.project_root, &absolute_path),
        ) {
            (Some(stored_sha), Some(current_sha)) if stored_sha != &current_sha => Ok(true),
            (Some(_), None) => Ok(true),
            _ => Ok(false),
        }
    }

    fn invalidate_insight(&self, insight_id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM insights WHERE id = ?1", params![insight_id])
            .map_err(|err| format!("failed to delete stale insight: {err}"))?;

        self.conn
            .execute("DELETE FROM queries WHERE id = ?1", params![insight_id])
            .map_err(|err| format!("failed to delete stale query: {err}"))?;

        Ok(())
    }
}

fn find_project_root(start_dir: &Path) -> Result<PathBuf, String> {
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

fn hash_query_text(query_text: &str) -> String {
    let digest = Sha256::digest(query_text.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn normalize_relative_path(project_root: &Path, file_path: &Path) -> Result<String, String> {
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

fn path_to_posix_string(path: &Path) -> String {
    let mut normalized = String::new();

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.push_str("../");
            }
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

fn capture_code_node_snapshot(
    project_root: &Path,
    normalized_path: &str,
) -> Result<CodeNodeSnapshot, String> {
    let absolute_path = project_root.join(normalized_path);
    let metadata = fs::metadata(&absolute_path).map_err(|err| {
        format!(
            "failed to read metadata for {}: {err}",
            absolute_path.display()
        )
    })?;

    Ok(CodeNodeSnapshot {
        id: normalized_path.to_string(),
        file_path: normalized_path.to_string(),
        last_known_git_sha: git_blob_sha(project_root, &absolute_path),
        last_known_mtime: file_mtime(&metadata)?,
    })
}

fn file_mtime(metadata: &fs::Metadata) -> Result<i64, String> {
    let modified = metadata
        .modified()
        .map_err(|err| format!("failed to read file modification time: {err}"))?;

    modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .map_err(|err| format!("invalid file modification time: {err}"))
}

fn git_blob_sha(project_root: &Path, file_path: &Path) -> Option<String> {
    if !project_root.join(".git").exists() {
        return None;
    }

    let relative_path = file_path
        .strip_prefix(project_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .replace('\\', "/");

    let output = Command::new("git")
        .current_dir(project_root)
        .args(["hash-object", &relative_path])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

fn current_unix_timestamp() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .map_err(|err| format!("system clock is before UNIX epoch: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn unique_temp_project(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();

        std::env::temp_dir().join(format!("mcp-adjutant-cache-{name}-{nanos}"))
    }

    fn init_git_repo(project_root: &Path) {
        Command::new("git")
            .current_dir(project_root)
            .args(["init"])
            .output()
            .expect("git init");

        Command::new("git")
            .current_dir(project_root)
            .args(["config", "user.email", "test@example.com"])
            .output()
            .expect("git config email");

        Command::new("git")
            .current_dir(project_root)
            .args(["config", "user.name", "Cache Test"])
            .output()
            .expect("git config name");
    }

    #[test]
    fn finds_project_root_from_nested_directory() {
        let project_root = unique_temp_project("root-detect");
        fs::create_dir_all(project_root.join("src/nested")).expect("create nested dirs");
        fs::write(
            project_root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .expect("cargo");

        let cache = ProjectCacheManager::new(&project_root.join("src/nested"))
            .expect("cache manager should initialize");

        assert_eq!(
            cache.project_root(),
            fs::canonicalize(&project_root).unwrap()
        );
        assert!(project_root.join(ADJUTANT_DIR).is_dir());

        fs::remove_dir_all(&project_root).ok();
    }

    #[test]
    fn appends_adjutant_directory_to_existing_gitignore() {
        let project_root = unique_temp_project("gitignore");
        fs::create_dir_all(&project_root).expect("create project root");
        fs::write(
            project_root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .expect("cargo");
        fs::write(project_root.join(".gitignore"), "target/\n").expect("gitignore");

        ProjectCacheManager::new(&project_root).expect("cache manager should initialize");

        let gitignore =
            fs::read_to_string(project_root.join(".gitignore")).expect("read gitignore");
        assert!(gitignore.contains(GITIGNORE_ENTRY));

        fs::remove_dir_all(&project_root).ok();
    }

    #[test]
    fn store_and_retrieve_valid_insight() {
        let project_root = unique_temp_project("cache-hit");
        fs::create_dir_all(project_root.join("src")).expect("create src");
        fs::write(
            project_root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .expect("cargo");
        let source_file = project_root.join("src/lib.rs");
        fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

        let mut cache = ProjectCacheManager::new(&project_root).expect("cache manager");
        cache
            .store_insight(
                "how does hello work?",
                "## Insight\nCalls `hello`.",
                vec![source_file.clone()],
            )
            .expect("store insight");

        let cached = cache
            .try_get_valid_insight("how does hello work?")
            .expect("lookup")
            .expect("cache hit");

        assert_eq!(cached, "## Insight\nCalls `hello`.");

        fs::remove_dir_all(&project_root).ok();
    }

    #[test]
    fn modified_file_invalidates_cached_insight() {
        let project_root = unique_temp_project("cache-invalidate");
        fs::create_dir_all(project_root.join("src")).expect("create src");
        fs::write(
            project_root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .expect("cargo");
        let source_file = project_root.join("src/lib.rs");
        fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

        let mut cache = ProjectCacheManager::new(&project_root).expect("cache manager");
        cache
            .store_insight(
                "explain hello",
                "## Insight\nOriginal.",
                vec![source_file.clone()],
            )
            .expect("store insight");

        std::thread::sleep(Duration::from_millis(1100));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&source_file)
            .expect("open source");
        writeln!(file, "// changed").expect("modify source");

        let cached = cache
            .try_get_valid_insight("explain hello")
            .expect("lookup");
        assert!(cached.is_none(), "modified file should invalidate cache");

        let retry = cache
            .try_get_valid_insight("explain hello")
            .expect("lookup after invalidation");
        assert!(retry.is_none(), "invalidated insight should be deleted");

        fs::remove_dir_all(&project_root).ok();
    }

    #[test]
    fn git_content_change_invalidates_cached_insight_without_mtime_change() {
        let project_root = unique_temp_project("git-invalidate");
        fs::create_dir_all(project_root.join("src")).expect("create src");
        fs::write(
            project_root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .expect("cargo");
        init_git_repo(&project_root);

        let source_file = project_root.join("src/lib.rs");
        fs::write(&source_file, "pub fn hello() {}\n").expect("write source");

        Command::new("git")
            .current_dir(&project_root)
            .args(["add", "."])
            .output()
            .expect("git add");

        let mut cache = ProjectCacheManager::new(&project_root).expect("cache manager");
        cache
            .store_insight(
                "git tracked insight",
                "## Insight\nTracked.",
                vec![source_file.clone()],
            )
            .expect("store insight");

        fs::write(&source_file, "pub fn hello() {}\npub fn world() {}\n").expect("rewrite source");

        let cached = cache
            .try_get_valid_insight("git tracked insight")
            .expect("lookup");
        assert!(
            cached.is_none(),
            "git blob change should invalidate cache even if mtime matches in edge cases"
        );

        fs::remove_dir_all(&project_root).ok();
    }
}
