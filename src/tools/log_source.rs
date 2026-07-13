use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::cache::resolve_workspace_path_bounded;
use crate::tools::crash_log::{read_log_file, strip_file_url, truncate_log_text};
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

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let (_final_url, body) = fetch_text_validated(trimmed)?;
        let (content, truncated) = truncate_log_text(&body);
        return Ok(ResolvedLog {
            content,
            truncated,
            kind: LogSourceKind::Https,
        });
    }

    let stripped = strip_file_url(trimmed);
    let resolved = resolve_workspace_path_bounded(stripped)?;
    if !resolved.is_file() {
        return Err("log_path must be a file; provide the specific log file path".into());
    }
    let (content, truncated) = read_log_file(&resolved)?;
    Ok(ResolvedLog {
        content,
        truncated,
        kind: LogSourceKind::Local,
    })
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

    let run_id = validate_gh_run_id(run_id)?;
    let child = Command::new("gh")
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
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(GH_LOG_TIMEOUT) {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("gh wait failed: {err}")),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // ponytail: best-effort kill; hung gh may orphan without unix kill
            #[cfg(unix)]
            {
                let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            }
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
    fn resolve_rejects_private_https_url() {
        assert!(resolve_log_content("https://127.0.0.1/log.txt").is_err());
    }
}
