//! LLM context densifier (AgentPhase::Pruner). Used by MCP `compact_context` and auto-compact.

use std::future::Future;
use std::sync::Arc;

use crate::llm::{LlmClient, LlmRequest, LlmToolSet};
use crate::metrics::current_job_context;

pub const CHARS_PER_TOKEN: usize = 4;
pub const COMPACT_THRESHOLD_PCT: u32 = 80;
pub const COMPACT_CONTEXT_TOOL_NAME: &str = "compact_context";

pub const PRUNER_SYSTEM_PROMPT: &str = r#"You are a context densifier (PRUNER). Rewrite the transcript into a shorter main prompt for another agent.

Rules:
- Preserve every actionable fact: paths, file:line, symbols, errors, decisions, open questions, tool outcomes
- Drop chatter, repeated observations, and redundant tool dumps
- Output ONLY the densified prompt text — no preamble, no markdown fences around the whole reply
- Stay under the TARGET_TOKENS budget (approx 4 chars ≈ 1 token)
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactMode {
    /// Densify with minimal information loss (may stay near the target).
    Compact,
    /// Hard-fit under target_tokens.
    Reduce,
}

impl CompactMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "compact" => Ok(Self::Compact),
            "reduce" => Ok(Self::Reduce),
            other => Err(format!("mode must be compact|reduce, got {other:?}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Reduce => "reduce",
        }
    }
}

pub struct AutoCompactGuard {
    pub window_tokens: u32,
    pub pruner: Arc<dyn LlmClient>,
}

tokio::task_local! {
    static AUTO_COMPACT: AutoCompactGuard;
}

pub async fn with_auto_compact_async<F, Fut, R>(guard: AutoCompactGuard, work: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R>,
{
    AUTO_COMPACT.scope(guard, work()).await
}

fn current_auto_compact() -> Option<AutoCompactGuard> {
    AUTO_COMPACT
        .try_with(|g| AutoCompactGuard {
            window_tokens: g.window_tokens,
            pruner: Arc::clone(&g.pruner),
        })
        .ok()
}

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN).max(1)
}

pub fn tokens_over_threshold(est_tokens: usize, window_tokens: u32) -> bool {
    if window_tokens == 0 {
        return false;
    }
    let limit = (window_tokens as u64 * COMPACT_THRESHOLD_PCT as u64) / 100;
    est_tokens as u64 >= limit
}

pub fn is_context_overflow_err(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    (lower.contains("context")
        && (lower.contains("length")
            || lower.contains("window")
            || lower.contains("overflow")
            || lower.contains("too long")
            || lower.contains("maximum")))
        || lower.contains("maximum context")
        || lower.contains("token limit")
        || lower.contains("too many tokens")
        || lower.contains("context_length_exceeded")
}

fn auto_compact_allowed() -> bool {
    !matches!(
        current_job_context().and_then(|c| c.mcp_tool),
        Some(ref t) if t == COMPACT_CONTEXT_TOOL_NAME
    )
}

/// Densify `context` under `target_tokens` using the Pruner model.
pub fn compact_text<C: LlmClient + ?Sized>(
    client: &C,
    context: &str,
    mode: CompactMode,
    target_tokens: u32,
) -> Result<String, String> {
    let target_tokens = target_tokens.max(256);
    let target_chars = (target_tokens as usize).saturating_mul(CHARS_PER_TOKEN);
    let mode_line = match mode {
        CompactMode::Compact => {
            "MODE=compact — densify with the smallest possible loss; prefer staying under TARGET."
        }
        CompactMode::Reduce => {
            "MODE=reduce — HARD CAP: output MUST fit under TARGET_TOKENS; drop lowest-value detail first."
        }
    };
    let user = format!(
        "{mode_line}\nTARGET_TOKENS={target_tokens} (~{target_chars} chars)\n\n---\nCONTEXT TO DENSIFY:\n{context}"
    );
    let empty = LlmToolSet::new();
    let turn = client.complete(LlmRequest::new(PRUNER_SYSTEM_PROMPT, &user, &empty))?;
    let out = turn
        .content
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "pruner returned empty content".to_string())?;

    if mode == CompactMode::Reduce && estimate_tokens(&out) > target_tokens as usize {
        // ponytail: hard truncate if model ignored the cap
        let keep = target_chars.min(out.chars().count());
        return Ok(out.chars().take(keep).collect());
    }
    Ok(out)
}

/// Replace observation history (or whole prompt) so the next turn fits the window.
pub fn rewrite_context_for_window(
    input_prompt: &str,
    accumulated_data: &str,
    compacted: &str,
) -> (String, String) {
    if accumulated_data.is_empty() {
        (compacted.to_string(), String::new())
    } else {
        (
            input_prompt.to_string(),
            format!("[compacted observation history]\n{compacted}"),
        )
    }
}

/// Proactive densify when estimated prompt is ≥80% of the caller window.
pub fn maybe_proactive_compact(
    system_prompt: &str,
    context: &mut super::AgentContext,
) -> Result<(), String> {
    let Some(guard) = current_auto_compact() else {
        return Ok(());
    };
    if !auto_compact_allowed() {
        return Ok(());
    }
    let user = super::build_tool_loop_message(context);
    let est = estimate_tokens(system_prompt).saturating_add(estimate_tokens(&user));
    if !tokens_over_threshold(est, guard.window_tokens) {
        return Ok(());
    }
    apply_compact_to_context(context, &guard, CompactMode::Reduce)
}

/// After a context-overflow LLM error, densify once and signal caller to retry.
pub fn maybe_overflow_compact(
    err: &str,
    context: &mut super::AgentContext,
) -> Result<bool, String> {
    if !is_context_overflow_err(err) {
        return Ok(false);
    }
    let Some(guard) = current_auto_compact() else {
        return Ok(false);
    };
    if !auto_compact_allowed() {
        return Ok(false);
    }
    apply_compact_to_context(context, &guard, CompactMode::Reduce)?;
    Ok(true)
}

fn apply_compact_to_context(
    context: &mut super::AgentContext,
    guard: &AutoCompactGuard,
    mode: CompactMode,
) -> Result<(), String> {
    let blob = if context.accumulated_data.is_empty() {
        context.input_prompt.clone()
    } else {
        context.accumulated_data.clone()
    };
    let target = ((guard.window_tokens as u64 * 50) / 100).max(512) as u32;
    let compacted = compact_text(guard.pruner.as_ref(), &blob, mode, target)?;
    let (prompt, acc) =
        rewrite_context_for_window(&context.input_prompt, &context.accumulated_data, &compacted);
    context.input_prompt = prompt;
    context.accumulated_data = acc;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmModelTurn, LlmRequest};

    struct EchoShorter;

    impl LlmClient for EchoShorter {
        fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
            let body = request.user_message.chars().take(200).collect::<String>();
            Ok(LlmModelTurn {
                content: Some(format!("DENSE:{body}")),
                tool_calls: vec![],
                usage: None,
            })
        }
    }

    #[test]
    fn estimate_tokens_uses_chars_per_token() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn threshold_at_eighty_percent() {
        assert!(!tokens_over_threshold(79, 100));
        assert!(tokens_over_threshold(80, 100));
    }

    #[test]
    fn overflow_err_detects_common_phrases() {
        assert!(is_context_overflow_err("context_length_exceeded"));
        assert!(is_context_overflow_err("Error: maximum context length"));
        assert!(!is_context_overflow_err("temperature unsupported"));
    }

    #[test]
    fn compact_mode_parse() {
        assert_eq!(CompactMode::parse("compact").unwrap(), CompactMode::Compact);
        assert_eq!(CompactMode::parse("REDUCE").unwrap(), CompactMode::Reduce);
        assert!(CompactMode::parse("squash").is_err());
    }

    #[test]
    fn compact_text_returns_model_output() {
        let out = compact_text(&EchoShorter, "hello world stuff", CompactMode::Compact, 512)
            .expect("compact");
        assert!(out.starts_with("DENSE:"));
    }

    #[test]
    fn reduce_hard_truncates_when_over_target() {
        struct Verbose;
        impl LlmClient for Verbose {
            fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
                Ok(LlmModelTurn {
                    content: Some("x".repeat(10_000)),
                    tool_calls: vec![],
                    usage: None,
                })
            }
        }
        let out = compact_text(&Verbose, "src", CompactMode::Reduce, 100).expect("reduce");
        // floor is 256 tokens even if caller asks for less
        assert!(estimate_tokens(&out) <= 256);
    }
}
