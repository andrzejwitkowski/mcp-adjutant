use serde::Deserialize;

use crate::llm::{LlmClient, LlmRequest, LlmToolSet};
use crate::tools::{truncate_for_llm, CrashAnalysisCore};

pub const LOG_ANALYZER_SYSTEM_PROMPT: &str = r#"You are a hyper-focused log analysis utility. Triage crash logs: isolate the first root cause, extract coordinates, return ONLY one minified JSON object (no markdown fences, no fixes).

Schema:
{
  "error_type": "string",
  "error_message": "string",
  "target_file": "string or null",
  "line_number": "integer or null",
  "column_number": "integer or null",
  "isolated_stack_trace": "string (3-5 relevant lines only)",
  "summary": "string (one concise line: what failed and where)"
}"#;

#[derive(Debug, Deserialize)]
pub struct LlmAnalysisPayload {
    pub error_type: String,
    pub error_message: String,
    pub target_file: Option<String>,
    pub line_number: Option<u32>,
    pub column_number: Option<u32>,
    pub isolated_stack_trace: String,
    pub summary: Option<String>,
}

pub struct LogAnalyzerAgent<C: LlmClient> {
    client: C,
}

impl<C: LlmClient> LogAnalyzerAgent<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    pub fn analyze(&self, log_text: &str) -> Result<LlmAnalysisPayload, String> {
        let user_message = format!("LOG DUMP:\n{}", truncate_for_llm(log_text));
        let empty_tools = LlmToolSet::new();
        let request = LlmRequest::new(LOG_ANALYZER_SYSTEM_PROMPT, &user_message, &empty_tools);
        let turn = self.client.complete(request)?;
        let raw = turn
            .content
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| "log analyzer model response missing content".to_string())?;
        parse_llm_response(&raw)
    }
}

pub fn llm_payload_to_core(payload: LlmAnalysisPayload) -> (CrashAnalysisCore, Option<String>) {
    let summary = payload.summary.clone();
    (
        CrashAnalysisCore {
            error_type: payload.error_type,
            error_message: payload.error_message,
            target_file: payload.target_file,
            line_number: payload.line_number,
            column_number: payload.column_number,
            isolated_stack_trace: payload.isolated_stack_trace,
        },
        summary,
    )
}

pub fn parse_llm_response(raw: &str) -> Result<LlmAnalysisPayload, String> {
    let trimmed = raw.trim();
    let fenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let json_body = extract_json_object(fenced).unwrap_or(fenced);
    serde_json::from_str(json_body)
        .map_err(|err| format!("failed to parse log analyzer JSON: {err}"))
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
}
