use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::cache::{mcp_workspace_root, resolve_workspace_path_bounded};
use crate::tools::crash_log::{read_log_file, strip_file_url, truncate_log_text, MAX_LOG_BYTES};
use crate::tools::web_fetch::fetch_text_validated;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSourceKind {
    Local,
    Https,
    GhActions,
}

impl LogSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Https => "https",
            Self::GhActions => "gh_actions",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLog {
    pub content: String,
    pub truncated: bool,
    pub kind: LogSourceKind,
}

pub fn resolve_log_content(specifier: &str) -> Result<ResolvedLog, String> {
    let trimmed = specifier.trim();
    if trimmed.is_empty() {
        return Err("log_path is required".into());
    }

    if let Some(run_id) = parse_gh_run_specifier(trimmed) {
        let (content, truncated) = fetch_gh_failed_log(&run_id)?;
        return Ok(ResolvedLog {
            content,
            truncated,
            kind: LogSourceKind::GhActions,
        });
    }

    if trimmed.starts_with("https://") {
        let (_final_url, body) = fetch_text_validated(trimmed)?;
        let (content, truncated) = truncate_log_text(&body);
        return Ok(ResolvedLog {
            content,
            truncated,
            kind: LogSourceKind::Https,
        });
    }
    if trimmed.starts_with("http://") {
        return Err("log_path must use https:// for remote logs".into());
    }

    let resolved = resolve_workspace_log_file(strip_file_url(trimmed))?;
    let (content, truncated) = read_log_file(&resolved)?;
    Ok(ResolvedLog {
        content,
        truncated,
        kind: LogSourceKind::Local,
    })
}

fn resolve_workspace_log_file(path: &str) -> Result<PathBuf, String> {
    let resolved = resolve_workspace_path_bounded(path)?;
    if !resolved.is_file() {
        return Err("log_path must be a file; provide the specific log file path".into());
    }
    let canonical = std::fs::canonicalize(&resolved)
        .map_err(|err| format!("failed to resolve {}: {err}", resolved.display()))?;
    let root = std::fs::canonicalize(mcp_workspace_root()).unwrap_or_else(|_| mcp_workspace_root());
    if !canonical.starts_with(&root) {
        return Err("log_path must stay within the workspace".into());
    }
    Ok(canonical)
}

fn read_pipe_capped(mut reader: impl Read, max: usize, out: &mut Vec<u8>) -> std::io::Result<()> {
    let mut chunk = [0u8; 8192];
    while out.len() < max {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        let room = max - out.len();
        out.extend_from_slice(&chunk[..n.min(room)]);
    }
    Ok(())
}

fn kill_process(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status();
    }
}

fn parse_gh_run_specifier(specifier: &str) -> Option<String> {
    let run_id = specifier
        .strip_prefix("gh-run://")
        .or_else(|| specifier.strip_prefix("gh-run:"))?;
    validate_gh_run_id(run_id).ok()
}

fn validate_gh_run_id(run_id: &str) -> Result<String, String> {
    let id = run_id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("invalid gh run id: {run_id:?}"));
    }
    Ok(id.to_string())
}

fn fetch_gh_failed_log(run_id: &str) -> Result<(String, bool), String> {
    const GH_LOG_TIMEOUT: Duration = Duration::from_secs(90);
    const GH_STDOUT_CAP: usize = MAX_LOG_BYTES.saturating_add(8192);

    let run_id = validate_gh_run_id(run_id)?;
    let mut child = Command::new("gh")
        .args(["run", "view", &run_id, "--log-failed"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "gh CLI not found; install GitHub CLI to fetch Actions logs".into()
            } else {
                format!("failed to spawn gh: {err}")
            }
        })?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(out) = child.stdout.take() {
            let _ = read_pipe_capped(out, GH_STDOUT_CAP, &mut stdout);
        }
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_end(&mut stderr);
        }
        let status = child.wait();
        let output = status.map(|code| Output {
            status: code,
            stdout,
            stderr,
        });
        let _ = tx.send(output);
    });
    let output = match rx.recv_timeout(GH_LOG_TIMEOUT) {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("gh wait failed: {err}")),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_process(pid);
            return Err(format!(
                "gh run view timed out after {}s",
                GH_LOG_TIMEOUT.as_secs()
            ));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err("gh wait thread disconnected".into());
        }
    };

    if !output.status.success() {
        let stderr_output = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr_output.trim();
        let hint = if stderr.contains("auth") || stderr.contains("login") {
            " (run `gh auth login`)"
        } else {
            ""
        };
        return Err(format!(
            "gh run view failed (exit {}): {}{}",
            output.status,
            if stderr.is_empty() {
                "no stderr"
            } else {
                stderr
            },
            hint
        ));
    }

    Ok(truncate_log_text(
        String::from_utf8_lossy(&output.stdout).trim_end(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gh_run_specifier_accepts_colon_and_slashes() {
        assert_eq!(
            parse_gh_run_specifier("gh-run:12345678901").as_deref(),
            Some("12345678901")
        );
        assert_eq!(
            parse_gh_run_specifier("gh-run://9876543210").as_deref(),
            Some("9876543210")
        );
    }

    #[test]
    fn parse_gh_run_specifier_rejects_injection() {
        assert!(parse_gh_run_specifier("gh-run:123;rm").is_none());
        assert!(parse_gh_run_specifier("gh-run:abc").is_none());
    }

    #[test]
    fn resolve_rejects_cleartext_http_url() {
        assert!(resolve_log_content("http://example.com/log.txt").is_err());
    }

    #[test]
    fn resolve_rejects_private_https_url() {
        assert!(resolve_log_content("https://127.0.0.1/log.txt").is_err());
    }
}
