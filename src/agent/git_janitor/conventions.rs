use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const ADJUTANT_TOML: &str = ".adjutant.toml";
pub const GITJANITOR_JSON: &str = ".gitjanitor.json";
pub const DEFAULT_TICKET_REGEX: &str = r"([A-Z]+-\d+)";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitConventions {
    #[serde(default)]
    pub git_rules: GitRules,
    #[serde(default)]
    pub commit_format: CommitFormat,
    #[serde(default)]
    pub pr: PrConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitRules {
    #[serde(default = "default_commit_style")]
    pub commit_style: String,
    #[serde(default = "default_ticket_regex")]
    pub ticket_regex: String,
    #[serde(default = "default_true")]
    pub require_ticket_in_commit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitFormat {
    #[serde(default = "default_commit_pattern")]
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrConfig {
    #[serde(default = "default_pr_template")]
    pub template_file: String,
}

fn default_commit_style() -> String {
    "conventional".into()
}
fn default_ticket_regex() -> String {
    DEFAULT_TICKET_REGEX.into()
}
fn default_true() -> bool {
    true
}
fn default_commit_pattern() -> String {
    "[{ticket}] {type}: {summary}".into()
}
fn default_pr_template() -> String {
    ".github/PULL_REQUEST_TEMPLATE.md".into()
}

impl Default for GitRules {
    fn default() -> Self {
        Self {
            commit_style: default_commit_style(),
            ticket_regex: default_ticket_regex(),
            require_ticket_in_commit: true,
        }
    }
}

impl Default for CommitFormat {
    fn default() -> Self {
        Self {
            pattern: default_commit_pattern(),
        }
    }
}

impl Default for PrConfig {
    fn default() -> Self {
        Self {
            template_file: default_pr_template(),
        }
    }
}

impl Default for GitConventions {
    fn default() -> Self {
        Self {
            git_rules: GitRules::default(),
            commit_format: CommitFormat::default(),
            pr: PrConfig::default(),
        }
    }
}

pub fn load_conventions(root: &Path) -> (GitConventions, Option<PathBuf>, bool) {
    let toml_path = root.join(ADJUTANT_TOML);
    if toml_path.is_file() {
        if let Ok(raw) = std::fs::read_to_string(&toml_path) {
            if let Ok(parsed) = toml::from_str::<GitConventions>(&raw) {
                return (parsed, Some(toml_path), true);
            }
        }
    }
    let json_path = root.join(GITJANITOR_JSON);
    if json_path.is_file() {
        if let Ok(raw) = std::fs::read_to_string(&json_path) {
            if let Ok(parsed) = serde_json::from_str::<GitConventions>(&raw) {
                return (parsed, Some(json_path), true);
            }
        }
    }
    (GitConventions::default(), None, false)
}

pub fn write_adjutant_toml(root: &Path, conventions: &GitConventions) -> Result<PathBuf, String> {
    let path = root.join(ADJUTANT_TOML);
    let body = toml::to_string_pretty(conventions)
        .map_err(|err| format!("serialize {ADJUTANT_TOML}: {err}"))?;
    std::fs::write(&path, body).map_err(|err| format!("write {}: {err}", path.display()))?;
    Ok(path)
}

pub fn merge_conventions_patch(
    base: &GitConventions,
    patch: &serde_json::Value,
) -> Result<GitConventions, String> {
    let mut merged =
        serde_json::to_value(base).map_err(|err| format!("serialize conventions: {err}"))?;
    merge_json(&mut merged, patch);
    serde_json::from_value(merged).map_err(|err| format!("invalid conventions patch: {err}"))
}

fn merge_json(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                match base_map.get_mut(k) {
                    Some(existing) => merge_json(existing, v),
                    None => {
                        base_map.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

pub fn extract_ticket(text: &str, ticket_regex: &str) -> Option<String> {
    let re = regex_lite_find(ticket_regex, text)?;
    Some(re)
}

/// Minimal capture: use first `([A-Z]+-\d+)`-style match via std when regex crate absent.
/// ponytail: std-only ticket scan; full regex crate if exotic ticket_regex needed
fn regex_lite_find(pattern: &str, text: &str) -> Option<String> {
    if pattern.contains("A-Z") && pattern.contains("\\d") {
        return find_jira_ticket(text);
    }
    find_jira_ticket(text)
}

pub fn find_jira_ticket(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_uppercase() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_uppercase() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'-' {
                i += 1;
                let digit_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > digit_start {
                    return Some(text[start..i].to_string());
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

pub fn conventions_toml_string(conventions: &GitConventions) -> Result<String, String> {
    toml::to_string_pretty(conventions).map_err(|err| format!("toml serialize: {err}"))
}
