use serde::Deserialize;

use crate::cache::{load_best_desired_output_exemplar, mcp_workspace_root, open_cache_connection};
use crate::domain::AdjutantConfig;
use crate::llm::{create_log_analyzer_llm_client, LlmClient, LlmRequest, LlmToolSet};
use crate::tools::{
    analyze_crash_log, parser_confident, resolve_log_content, sanitize_core, to_report,
    truncate_for_llm, CrashAnalysisCore, ReportEnrichment,
};

pub const LOG_ANALYZER_SYSTEM_PROMPT: &str = r#"You are a hyper-focused log analysis utility. Triage crash/CI logs: isolate the first root cause, extract coordinates, and return actionable coordinator fields. Return ONLY one minified JSON object (no markdown fences).

Schema:
{
  "error_type": "string",
  "error_message": "string (clean — no GH Actions job/step/timestamp prefixes)",
  "target_file": "string or null (repo-relative when possible)",
  "line_number": "integer or null",
  "column_number": "integer or null",
  "isolated_stack_trace": "string (3-5 relevant lines only, cleaned)",
  "summary": "string (one concise line: what failed and where)",
  "failing_test": "string or null",
  "command": "string or null (e.g. cargo fmt -- --check)",
  "exit_code": "integer or null",
  "diagnosis": "string (one short hypothesis grounded in the log)",
  "reproduction": ["1-3 concrete shell commands"],
  "next_steps": ["up to 4 inspect/reproduce actions — never invent patches"]
}

Rules:
- Strip GitHub Actions `job\tstep\tISO8601Z` prefixes from every text field.
- Prefer repo-relative paths (src/..., tests/...).
- next_steps = inspect/reproduce only; NEVER invent patches, unified diffs, or fake compiler output.
- Do not hallucinate log lines that are not in the dump."#;

pub const LOG_ANALYZER_ENRICH_PROMPT: &str = r#"You enrich an already-parsed crash report. Coordinates are LOCKED — do not change error_type, target_file, line_number, column_number, or invent different stack frames. Return ONLY one minified JSON object (no markdown fences):

{
  "summary": "string (one concise clean line)",
  "diagnosis": "string (one short hypothesis grounded in the log)",
  "reproduction": ["1-3 concrete shell commands"],
  "next_steps": ["up to 4 inspect/reproduce actions"],
  "failing_test": "string or null",
  "command": "string or null",
  "exit_code": "integer or null"
}

Rules: strip GH Actions prefixes; no invented patches or hallucinated log lines; fill nulls only when clearly present in the log."#;

#[derive(Debug, Deserialize)]
pub struct LlmAnalysisPayload {
    pub error_type: String,
    pub error_message: String,
    pub target_file: Option<String>,
    pub line_number: Option<u32>,
    pub column_number: Option<u32>,
    pub isolated_stack_trace: String,
    pub summary: Option<String>,
    #[serde(default)]
    pub failing_test: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub diagnosis: Option<String>,
    #[serde(default)]
    pub reproduction: Option<Vec<String>>,
    #[serde(default)]
    pub next_steps: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct LlmEnrichPayload {
    pub summary: Option<String>,
    #[serde(default)]
    pub diagnosis: Option<String>,
    #[serde(default)]
    pub reproduction: Option<Vec<String>>,
    #[serde(default)]
    pub next_steps: Option<Vec<String>>,
    #[serde(default)]
    pub failing_test: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

pub struct LogAnalyzerAgent<C: LlmClient> {
    client: C,
}

impl<C: LlmClient> LogAnalyzerAgent<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    pub fn analyze(
        &self,
        log_text: &str,
        exemplar: Option<&str>,
    ) -> Result<LlmAnalysisPayload, String> {
        let mut user_message = format!("LOG DUMP:\n{}", truncate_for_llm(log_text));
        if let Some(ex) = exemplar.filter(|s| !s.trim().is_empty()) {
            user_message.push_str("\n\n## 10/10 output exemplar (match this JSON shape)\n");
            user_message.push_str(ex);
        }
        let empty_tools = LlmToolSet::new();
        let request = LlmRequest::new(LOG_ANALYZER_SYSTEM_PROMPT, &user_message, &empty_tools);
        let turn = self.client.complete(request)?;
        let raw = turn
            .content
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| "log analyzer model response missing content".to_string())?;
        parse_llm_response(&raw)
    }

    pub fn enrich(
        &self,
        log_text: &str,
        core: &CrashAnalysisCore,
        exemplar: Option<&str>,
    ) -> Result<LlmEnrichPayload, String> {
        let locked = serde_json::json!({
            "error_type": core.error_type,
            "error_message": core.error_message,
            "target_file": core.target_file,
            "line_number": core.line_number,
            "column_number": core.column_number,
            "isolated_stack_trace": core.isolated_stack_trace,
            "failing_test": core.failing_test,
            "command": core.command,
            "exit_code": core.exit_code,
        });
        let mut user_message = format!(
            "LOCKED PARSER RESULT:\n{}\n\nLOG DUMP:\n{}",
            locked,
            truncate_for_llm(log_text)
        );
        if let Some(ex) = exemplar.filter(|s| !s.trim().is_empty()) {
            user_message.push_str("\n\n## 10/10 output exemplar (match enrichment fields)\n");
            user_message.push_str(ex);
        }
        let empty_tools = LlmToolSet::new();
        let request = LlmRequest::new(LOG_ANALYZER_ENRICH_PROMPT, &user_message, &empty_tools);
        let turn = self.client.complete(request)?;
        let raw = turn
            .content
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| "log analyzer enrich response missing content".to_string())?;
        parse_enrich_response(&raw)
    }
}

pub fn analyze_log_at_path(
    config: &AdjutantConfig,
    log_path: &str,
    pretty: bool,
) -> Result<String, String> {
    let resolved = resolve_log_content(log_path)?;
    let mut final_core = analyze_crash_log(&resolved.content);
    let mut enrichment = ReportEnrichment::default();
    let exemplar = load_log_analyzer_exemplar();

    let confident = parser_confident(&final_core);
    match create_log_analyzer_llm_client(config) {
        Ok(client) => {
            let agent = LogAnalyzerAgent::new(client);
            if confident {
                match agent.enrich(&resolved.content, &final_core, exemplar.as_deref()) {
                    Ok(payload) => apply_enrich_payload(&mut final_core, &mut enrichment, payload),
                    Err(err) => tracing::debug!("log analyzer enrich skipped: {err}"),
                }
            } else {
                match agent.analyze(&resolved.content, exemplar.as_deref()) {
                    Ok(payload) => {
                        let (refined, summary) = llm_payload_to_core(payload, &mut enrichment);
                        final_core = refined;
                        sanitize_core(&mut final_core);
                        enrichment.summary = summary.or(enrichment.summary);
                    }
                    Err(err) => enrichment.llm_fallback_error = Some(err),
                }
            }
        }
        Err(err) => {
            if !confident {
                enrichment.llm_fallback_error = Some(err);
            }
        }
    }

    let report = to_report(
        final_core,
        log_path.to_string(),
        resolved.kind.as_str(),
        resolved.truncated,
        enrichment,
    );
    let serialize = if pretty {
        serde_json::to_string_pretty
    } else {
        serde_json::to_string
    };
    serialize(&report).map_err(|err| format!("serialize log report: {err}"))
}

fn load_log_analyzer_exemplar() -> Option<String> {
    let (_, conn) = open_cache_connection(&mcp_workspace_root()).ok()?;
    load_best_desired_output_exemplar(&conn, "LogAnalyzerAgent").ok()?
}

fn apply_enrich_payload(
    core: &mut CrashAnalysisCore,
    enrichment: &mut ReportEnrichment,
    payload: LlmEnrichPayload,
) {
    enrichment.summary = payload.summary.filter(|s| !s.trim().is_empty());
    enrichment.diagnosis = payload.diagnosis.filter(|s| !s.trim().is_empty());
    enrichment.reproduction = payload.reproduction.filter(|v| !v.is_empty());
    enrichment.next_steps = payload
        .next_steps
        .map(|mut v| {
            v.truncate(4);
            v
        })
        .filter(|v| !v.is_empty());
    if core.failing_test.is_none() {
        core.failing_test = payload.failing_test.filter(|s| !s.trim().is_empty());
    }
    if core.command.is_none() {
        core.command = payload.command.filter(|s| !s.trim().is_empty());
    }
    if core.exit_code.is_none() {
        core.exit_code = payload.exit_code;
    }
}

pub fn llm_payload_to_core(
    payload: LlmAnalysisPayload,
    enrichment: &mut ReportEnrichment,
) -> (CrashAnalysisCore, Option<String>) {
    enrichment.diagnosis = payload.diagnosis.filter(|s| !s.trim().is_empty());
    enrichment.reproduction = payload.reproduction.filter(|v| !v.is_empty());
    enrichment.next_steps = payload
        .next_steps
        .map(|mut v| {
            v.truncate(4);
            v
        })
        .filter(|v| !v.is_empty());
    let summary = payload.summary.clone();
    (
        CrashAnalysisCore {
            error_type: payload.error_type,
            error_message: payload.error_message,
            target_file: payload.target_file,
            line_number: payload.line_number,
            column_number: payload.column_number,
            isolated_stack_trace: payload.isolated_stack_trace,
            failing_test: payload.failing_test,
            command: payload.command,
            exit_code: payload.exit_code,
        },
        summary,
    )
}

pub fn parse_llm_response(raw: &str) -> Result<LlmAnalysisPayload, String> {
    let json_body = strip_json_payload(raw);
    serde_json::from_str(json_body)
        .map_err(|err| format!("failed to parse log analyzer JSON: {err}"))
}

pub fn parse_enrich_response(raw: &str) -> Result<LlmEnrichPayload, String> {
    let json_body = strip_json_payload(raw);
    serde_json::from_str(json_body)
        .map_err(|err| format!("failed to parse log analyzer enrich JSON: {err}"))
}

fn strip_json_payload(raw: &str) -> &str {
    let trimmed = raw.trim();
    let fenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    extract_json_object(fenced).unwrap_or(fenced)
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_llm_response_strips_fences() {
        let payload = parse_llm_response(
            "```json\n{\"error_type\":\"Panic\",\"error_message\":\"boom\",\"target_file\":null,\"line_number\":null,\"column_number\":null,\"isolated_stack_trace\":\"x\",\"summary\":\"Panic — boom\"}\n```",
        )
        .expect("parse");
        assert_eq!(payload.error_type, "Panic");
        assert_eq!(payload.summary.as_deref(), Some("Panic — boom"));
    }

    #[test]
    fn parse_enrich_response_accepts_lean_fields() {
        let payload = parse_enrich_response(
            r#"{"summary":"Panic at tests/foo.rs:1","diagnosis":"assertion failed","reproduction":["cargo test foo -- --nocapture"],"next_steps":["Inspect tests/foo.rs:1"]}"#,
        )
        .expect("parse");
        assert_eq!(payload.summary.as_deref(), Some("Panic at tests/foo.rs:1"));
        assert_eq!(payload.reproduction.as_ref().map(|v| v.len()), Some(1));
    }
}
