//! LLM context densifier (`AgentPhase::Pruner`) for MCP `compact_context` and auto-compact.

use std::future::Future;
use std::sync::Arc;

use crate::llm::{LlmClient, LlmRequest, LlmToolSet};
use crate::metrics::{current_job_context, estimate_tokens};

pub const CHARS_PER_TOKEN: usize = 4;

/// Reentrancy guard: do not auto-compact while already inside `compact_context`.
const COMPACT_CONTEXT_MCP_TOOL: &str = "compact_context";

const PRUNER_SYSTEM_PROMPT: &str = r#"You are a context densifier (PRUNER). Rewrite the transcript into a shorter main prompt for another agent.

Rules:
- Preserve every actionable fact: paths, file:line, symbols, errors, decisions, open questions, tool outcomes
- Drop chatter, repeated observations, and redundant tool dumps
- Output ONLY the densified prompt text — no preamble, no markdown fences around the whole reply
- Stay under the TARGET_TOKENS budget (approx 4 chars ≈ 1 token)
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactMode {
    Compact,
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

/// Window + densifier client. Proactive/target math lives here (not scattered literals).
#[derive(Clone)]
pub struct AutoCompactGuard {
    /// Calling phase context window (Scout/WebFetcher/…).
    pub window_tokens: u32,
    /// Pruner model window — densify input must fit this, not only the caller window.
    pub pruner_window_tokens: u32,
    pub pruner: Arc<dyn LlmClient>,
}

impl AutoCompactGuard {
    /// Fire proactive compact at this % of the window.
    pub const PROACTIVE_PCT: u32 = 80;
    /// Densify down to this % of the (bounded) window.
    pub const TARGET_PCT: u32 = 50;
    pub const MIN_TARGET_TOKENS: u32 = 512;
    /// Leave headroom in Pruner's window for system prompt + MODE framing.
    pub const PRUNER_INPUT_PCT: u32 = 70;

    pub fn over_proactive_threshold(&self, est_tokens: u64) -> bool {
        if self.window_tokens == 0 {
            return false;
        }
        let limit = (u64::from(self.window_tokens) * u64::from(Self::PROACTIVE_PCT)) / 100;
        est_tokens >= limit
    }

    pub fn densify_target_tokens(&self) -> u32 {
        let caller = (u64::from(self.window_tokens) * u64::from(Self::TARGET_PCT)) / 100;
        let pruner = (u64::from(self.pruner_window_tokens) * u64::from(Self::TARGET_PCT)) / 100;
        caller.min(pruner).max(u64::from(Self::MIN_TARGET_TOKENS)) as u32
    }

    fn pruner_input_budget_chars(&self) -> usize {
        let tokens =
            (u64::from(self.pruner_window_tokens) * u64::from(Self::PRUNER_INPUT_PCT)) / 100;
        let tokens = tokens.max(u64::from(Self::MIN_TARGET_TOKENS)) as usize;
        tokens.saturating_mul(CHARS_PER_TOKEN)
    }
}

tokio::task_local! {
    // ponytail: ambient install via with_phase_auto_compact — pass Option through orchestrator if more call sites need opt-out
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
    AUTO_COMPACT.try_with(Clone::clone).ok()
}

fn is_context_overflow_err(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    (lower.contains("context")
        && (lower.contains("length")
            || lower.contains("window")
            || lower.contains("overflow")
            || lower.contains("too long")
            || lower.contains("maximum")))
        || lower.contains("token limit")
        || lower.contains("too many tokens")
        || lower.contains("context_length_exceeded")
}

fn auto_compact_allowed() -> bool {
    !matches!(
        current_job_context().and_then(|c| c.mcp_tool),
        Some(ref t) if t == COMPACT_CONTEXT_MCP_TOOL
    )
}

/// Densify `context` under `target_tokens` using the Pruner model.
pub fn compact_text<C: LlmClient + ?Sized>(
    client: &C,
    context: &str,
    mode: CompactMode,
    target_tokens: u32,
) -> Result<String, String> {
    if target_tokens == 0 {
        return Err("target_tokens must be greater than zero".to_string());
    }
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

    if mode == CompactMode::Reduce && estimate_tokens(&out) > u64::from(target_tokens) {
        // ponytail: hard truncate if model ignored the cap
        let keep = target_chars.min(out.chars().count());
        return Ok(out.chars().take(keep).collect());
    }
    Ok(out)
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
    if !guard.over_proactive_threshold(est) {
        return Ok(());
    }
    apply_compact_to_context(context, &guard, CompactMode::Reduce)?;
    Ok(())
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
    apply_compact_to_context(context, &guard, CompactMode::Reduce)
}

fn apply_compact_to_context(
    context: &mut super::AgentContext,
    guard: &AutoCompactGuard,
    mode: CompactMode,
) -> Result<bool, String> {
    let blob = super::build_tool_loop_message(context);
    let budget = guard.pruner_input_budget_chars();
    let char_count = blob.chars().count();
    let blob = if char_count > budget {
        // ponytail: keep the tail (recent observations) when caller window ≫ pruner window
        blob.chars().skip(char_count - budget).collect()
    } else {
        blob
    };
    let compacted = match compact_text(
        guard.pruner.as_ref(),
        &blob,
        mode,
        guard.densify_target_tokens(),
    ) {
        Ok(text) => text,
        // Densify is best-effort — don't kill the tool turn when the pruner itself overflows.
        Err(err) if is_context_overflow_err(&err) => return Ok(false),
        Err(err) => return Err(err),
    };
    context.input_prompt = compacted;
    context.accumulated_data.clear();
    Ok(true)
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

    fn guard(window: u32) -> AutoCompactGuard {
        AutoCompactGuard {
            window_tokens: window,
            pruner_window_tokens: window,
            pruner: Arc::new(EchoShorter) as Arc<dyn LlmClient>,
        }
    }

    #[test]
    fn densify_target_bounded_by_pruner_window() {
        let g = AutoCompactGuard {
            window_tokens: 100_000,
            pruner_window_tokens: 4_000,
            pruner: Arc::new(EchoShorter) as Arc<dyn LlmClient>,
        };
        assert_eq!(g.densify_target_tokens(), 2_000);
    }

    #[test]
    fn proactive_threshold_at_eighty_percent() {
        let g = guard(100);
        assert!(!g.over_proactive_threshold(79));
        assert!(g.over_proactive_threshold(80));
    }

    #[test]
    fn densify_target_is_half_window_floored() {
        assert_eq!(guard(10_000).densify_target_tokens(), 5_000);
        assert_eq!(
            guard(100).densify_target_tokens(),
            AutoCompactGuard::MIN_TARGET_TOKENS
        );
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
        assert!(estimate_tokens(&out) <= 100);
    }

    #[test]
    fn compact_text_rejects_zero_target() {
        let err = compact_text(&EchoShorter, "x", CompactMode::Compact, 0).unwrap_err();
        assert!(err.contains("greater than zero"), "{err}");
    }
}
