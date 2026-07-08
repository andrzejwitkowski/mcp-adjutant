use std::fs;
use std::path::{Path, PathBuf};

use crate::llm::{LlmClient, LlmRequest, LlmToolSet};

pub const BUILD_DISCOVERY_SYSTEM_PROMPT: &str = r#"You are a build-system detector. You will receive directory structure and file names.
Reply with exactly one line:
- BUILD: command="shell command to check compile/types"
- BUILD: unknown

Rules:
- the command runs in the given working directory
- prefer safe check/compile commands, not install/deploy
- one line, no markdown"#;

const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".cargo",
    ".idea",
    ".vscode",
];

pub trait BuildCommandDiscoverer: Send + Sync {
    fn discover(&self, anchor: &Path, snapshot: &str) -> Result<Option<String>, String>;
}

pub struct NoopBuildDiscoverer;

impl BuildCommandDiscoverer for NoopBuildDiscoverer {
    fn discover(&self, _anchor: &Path, _snapshot: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
}

pub struct LlmBuildDiscoverer<C> {
    client: C,
}

impl<C: LlmClient> LlmBuildDiscoverer<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: LlmClient> BuildCommandDiscoverer for LlmBuildDiscoverer<C> {
    fn discover(&self, anchor: &Path, snapshot: &str) -> Result<Option<String>, String> {
        let user_message = format!(
            "Working directory: {}\n\nFile structure:\n{snapshot}\n\nHow do I compile or type-check this module?",
            anchor.display()
        );
        let tools = LlmToolSet::new();
        let request = LlmRequest::new(BUILD_DISCOVERY_SYSTEM_PROMPT, &user_message, &tools);
        let turn = self.client.complete(request)?;
        let response = turn.content.unwrap_or_default();
        Ok(parse_build_discovery_response(&response))
    }
}

pub fn inference_anchor(path: &Path) -> PathBuf {
    let mut current = if path.is_file() {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };

    loop {
        if current.join(".git").is_dir() {
            return current;
        }
        if !current.pop() {
            return if path.is_file() {
                path.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| path.to_path_buf())
            } else {
                path.to_path_buf()
            };
        }
    }
}

pub fn snapshot_build_context(root: &Path, max_depth: usize) -> Result<String, String> {
    if !root.is_dir() {
        return Err(format!(
            "snapshot root must be a directory: {}",
            root.display()
        ));
    }

    let mut lines = Vec::new();
    snapshot_dir(root, root, 0, max_depth, &mut lines)?;
    Ok(lines.join("\n"))
}

fn snapshot_dir(
    root: &Path,
    current: &Path,
    depth: usize,
    max_depth: usize,
    lines: &mut Vec<String>,
) -> Result<(), String> {
    let indent = "  ".repeat(depth);
    let name = current
        .strip_prefix(root)
        .ok()
        .and_then(|p| p.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(".");
    lines.push(format!("{indent}{name}/"));

    if depth >= max_depth {
        return Ok(());
    }

    let mut entries: Vec<_> = fs::read_dir(current)
        .map_err(|err| format!("failed to read {}: {err}", current.display()))?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if path.is_dir() {
            if SKIP_DIRS.iter().any(|skip| name == *skip) {
                lines.push(format!("{}  {name}/", indent));
                continue;
            }
            snapshot_dir(root, &path, depth + 1, max_depth, lines)?;
        } else {
            lines.push(format!("{}  {name}", indent));
        }
    }

    Ok(())
}

pub fn parse_build_discovery_response(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("BUILD:"))?;

    if line.contains("unknown") {
        return None;
    }

    let command = parse_discovery_value(line, "command")?;
    if command.is_empty() || command.contains('\n') {
        return None;
    }

    Some(command)
}

fn parse_discovery_value(line: &str, key: &str) -> Option<String> {
    let pattern = format!("{key}=\"");
    let start = line.find(&pattern)? + pattern.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("mcp-adjutant-{test_name}-{nanos}"))
    }

    #[test]
    fn parse_build_discovery_response_extracts_command() {
        let cmd = parse_build_discovery_response(
            "Thought: cuda project\nBUILD: command=\"nvcc -std=c++17 -c kernel.cu\"",
        )
        .expect("command");
        assert_eq!(cmd, "nvcc -std=c++17 -c kernel.cu");
    }

    #[test]
    fn parse_build_discovery_response_rejects_unknown() {
        assert!(parse_build_discovery_response("BUILD: unknown").is_none());
    }

    #[test]
    fn snapshot_build_context_lists_files_with_depth_limit() {
        let root = temp_root("snapshot");
        fs::create_dir_all(root.join("kernels")).expect("dirs");
        fs::write(root.join("kernels/kernel.cu"), "code").expect("cu");
        fs::write(root.join("README.md"), "docs").expect("readme");

        let snapshot = snapshot_build_context(&root, 2).expect("snapshot");
        assert!(snapshot.contains("kernels/"));
        assert!(snapshot.contains("kernel.cu"));
        assert!(snapshot.contains("README.md"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn inference_anchor_climbs_to_git_root() {
        let root = temp_root("anchor");
        fs::create_dir_all(root.join(".git")).expect("git");
        let nested = root.join("kernels/sub");
        fs::create_dir_all(&nested).expect("nested");

        let anchor = inference_anchor(&nested.join("kernel.cu"));
        assert_eq!(anchor, root);

        fs::remove_dir_all(&root).ok();
    }
}
