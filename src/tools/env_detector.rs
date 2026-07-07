use std::path::{Path, PathBuf};
use std::process::Command;

use crate::domain::AdjutantConfig;

const CARGO_CHECK: &str = "cargo check --message-format=json";
const NPM_TYPECHECK: &str = "npm run typecheck";

pub fn find_nearest_module_boundary(
    start_path: &Path,
    config: &AdjutantConfig,
) -> Option<(PathBuf, String)> {
    let mut current = if start_path.is_file() {
        start_path.parent()?.to_path_buf()
    } else {
        start_path.to_path_buf()
    };

    loop {
        if let Some(cmd) = match_triage_override(&current, config) {
            return Some((current.clone(), cmd));
        }
        if current.join("Cargo.toml").is_file() {
            return Some((current.clone(), CARGO_CHECK.to_string()));
        }
        if current.join("package.json").is_file() {
            return Some((current.clone(), NPM_TYPECHECK.to_string()));
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn match_triage_override(dir: &Path, config: &AdjutantConfig) -> Option<String> {
    let overrides = config.triage_overrides.as_ref()?;
    let dir_str = dir.to_string_lossy();
    for (prefix, cmd) in overrides {
        let normalized = prefix.trim_end_matches('/');
        if dir.ends_with(normalized) || dir_str.ends_with(normalized) {
            return Some(cmd.clone());
        }
    }
    None
}

pub fn get_dirty_files_from_git() -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map_err(|err| format!("failed to spawn git: {err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        if !status
            .chars()
            .any(|c| matches!(c, 'M' | 'A' | '?' | 'R' | 'T'))
        {
            continue;
        }

        let mut path_part = line[3..].trim();
        if let Some(arrow) = path_part.rfind(" -> ") {
            path_part = &path_part[arrow + 4..];
        }
        path_part = path_part.trim_matches('"');
        if !path_part.is_empty() {
            files.push(PathBuf::from(path_part));
        }
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("mcp-adjutant-{test_name}-{nanos}"))
    }

    #[test]
    fn find_nearest_module_boundary_prefers_override() {
        let root = temp_root("override");
        let frontend = root.join("monorepo/frontend");
        fs::create_dir_all(frontend.join("src")).expect("dirs");
        fs::write(frontend.join("package.json"), "{}").expect("package.json");
        fs::write(frontend.join("src/App.tsx"), "export {}").expect("app");

        let config = AdjutantConfig {
            triage_overrides: Some(HashMap::from([(
                "frontend/".to_string(),
                "npm run build".to_string(),
            )])),
            ..Default::default()
        };

        let (dir, cmd) =
            find_nearest_module_boundary(&frontend.join("src/App.tsx"), &config).expect("boundary");
        assert_eq!(dir, frontend);
        assert_eq!(cmd, "npm run build");

        fs::remove_dir_all(&root).ok();
    }
}
