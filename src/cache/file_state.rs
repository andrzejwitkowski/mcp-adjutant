use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::UNIX_EPOCH;

pub struct CodeNodeSnapshot {
    pub id: String,
    pub file_path: String,
    pub last_known_git_sha: Option<String>,
    pub last_known_mtime: i64,
}

pub fn capture_code_node_snapshot(
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

pub fn is_code_node_dirty(project_root: &Path, node: &CodeNodeSnapshot) -> Result<bool, String> {
    let absolute_path = project_root.join(&node.file_path);

    let metadata = match fs::metadata(&absolute_path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(true),
    };

    if file_mtime(&metadata)? != node.last_known_mtime {
        return Ok(true);
    }

    match (
        &node.last_known_git_sha,
        git_blob_sha(project_root, &absolute_path),
    ) {
        (Some(stored_sha), Some(current_sha)) if stored_sha != &current_sha => Ok(true),
        (Some(_), None) | (None, Some(_)) => Ok(true),
        _ => Ok(false),
    }
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
