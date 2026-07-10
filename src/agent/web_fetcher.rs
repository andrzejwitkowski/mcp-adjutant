use async_trait::async_trait;
use serde_json::Value;

use super::orchestrator::run_single_tool_turn;
use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::WebFetcherProfile;
use crate::llm::{
    required_str, LlmClient, LlmModelTurn, LlmRequest, LlmTool, LlmToolSet, ToolDefinition,
};

pub const WEB_FETCHER_SYSTEM_PROMPT: &str = r#"You are an autonomous web research agent (WEB_FETCHER). Your goal is to produce a compacted, accurate markdown document of the latest, authoritative web content for a given topic. The topic can be anything the user asks about; adapt your search approach to the kind of information it requires.

Available tools (call exactly one per turn):
- search_web(query, focus?) — ask a browsing-capable model to search the live web for `query` and return grounded, cited markdown. Use `focus` to narrow (e.g. "official source", "recent news", "API reference", "primary source"). Non-terminal: results are added to your observation history.
- finalize(report) — end research and return your compacted markdown report.

Strategy:
1. Call search_web with the clearest possible query for the topic.
2. If the results are incomplete, refine the query (more specific terms, add a year, name the canonical source) and call search_web again. You may search up to the hop limit.
3. Once you have enough grounded material, call finalize with a single compacted markdown document: keep the authoritative facts relevant to the topic, drop filler, preserve any source links inline as markdown links.

Efficiency: prefer 1-2 well-targeted searches. Do not repeat the same query. Reply with a short Thought, then call exactly one tool."#;

const BROWSING_SYSTEM_PROMPT: &str = r#"You are a web research assistant with live web access. Search the web for the user's query and return a concise, accurate markdown summary of the most authoritative and up-to-date sources for the topic. Include inline markdown links to the sources you cite. Do not invent URLs."#;

/// Approximate chars-per-token ratio for the v1 character-budget truncation.
const CHARS_PER_TOKEN: usize = 4;

pub struct WebFetcherAgent<RC: LlmClient> {
    reasoning_client: RC,
    tools: LlmToolSet,
}

impl<RC: LlmClient> WebFetcherAgent<RC> {
    /// Build the agent with a reasoning client (drives the loop) and a browsing
    /// client (used inside the `search_web` tool to reach the live web).
    pub fn new<BC: LlmClient + 'static>(
        reasoning_client: RC,
        browsing_client: BC,
        profile: WebFetcherProfile,
    ) -> Self {
        let tools = web_fetcher_tool_set(browsing_client, profile.token_budget);
        Self {
            reasoning_client,
            tools,
        }
    }
}

#[async_trait]
impl<RC: LlmClient> AutonomousAgent for WebFetcherAgent<RC> {
    fn name(&self) -> &'static str {
        "web_fetcher_agent"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("WEB_FETCHER") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(WEB_FETCHER_SYSTEM_PROMPT);
        }
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        run_single_tool_turn(
            &self.reasoning_client,
            &self.tools,
            WEB_FETCHER_SYSTEM_PROMPT,
            context,
        )?;
        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context
            .input_prompt
            .push_str("\nContinue research based on the latest grounded observation.");
        Ok(())
    }
}

/// Build the web-fetcher tool set. The browsing client is moved into the
/// `search_web` tool so it can reach the live web on each invocation.
/// `token_budget` configures the `finalize` truncation.
fn web_fetcher_tool_set<BC: LlmClient + 'static>(
    browsing_client: BC,
    token_budget: u32,
) -> LlmToolSet {
    LlmToolSet::new()
        .register(SearchWebTool::new(browsing_client))
        .register(FinalizeWebTool::with_budget(token_budget))
}

/// `search_web(query, focus?)`: calls the browsing model and returns grounded markdown.
struct SearchWebTool<BC: LlmClient> {
    browsing_client: BC,
    definition: ToolDefinition,
}

impl<BC: LlmClient> SearchWebTool<BC> {
    fn new(browsing_client: BC) -> Self {
        Self {
            browsing_client,
            definition: ToolDefinition::new(
                "search_web",
                "Search the live web via a browsing model and return grounded, cited markdown.",
            )
            .string_param("query", "Web search query.", true)
            .string_param(
                "focus",
                "Optional focus to narrow results (e.g. 'official source', 'recent news').",
                false,
            ),
        }
    }
}

impl<BC: LlmClient> LlmTool for SearchWebTool<BC> {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        let query = required_str(arguments, "query")?;
        let focus = arguments
            .get("focus")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let user_message = match focus {
            Some(focus) => format!("Query: {query}\nFocus: {focus}"),
            None => format!("Query: {query}"),
        };

        let empty_tools = LlmToolSet::new();
        let request = LlmRequest::new(BROWSING_SYSTEM_PROMPT, &user_message, &empty_tools);
        let turn: LlmModelTurn = self.browsing_client.complete(request)?;

        turn.content
            .filter(|content| !content.trim().is_empty())
            .ok_or_else(|| "browsing model returned no content".to_string())
    }
}

/// `finalize(report)`: terminal tool. Applies the token-budget truncation.
struct FinalizeWebTool {
    definition: ToolDefinition,
    token_budget: u32,
}

impl FinalizeWebTool {
    fn with_budget(token_budget: u32) -> Self {
        Self {
            definition: ToolDefinition::new(
                "finalize",
                "Ends web research and returns a compacted markdown report.",
            )
            .string_param("report", "Final compacted markdown report.", true),
            token_budget,
        }
    }
}

impl LlmTool for FinalizeWebTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        let report = required_str(arguments, "report")?;
        Ok(truncate_to_token_budget(&report, self.token_budget))
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

/// Truncates `report` to an approximate character budget derived from
/// `token_budget` (token_budget * CHARS_PER_TOKEN). When truncated, appends a
/// visible `[truncated]` note. Reuses the TextPrunerMock prune idiom (char-based).
fn truncate_to_token_budget(report: &str, token_budget: u32) -> String {
    let char_budget = (token_budget as usize)
        .saturating_mul(CHARS_PER_TOKEN)
        .max(1);
    if report.chars().count() <= char_budget {
        return report.to_string();
    }
    let kept: String = report.chars().take(char_budget).collect();
    format!("{kept}\n\n[truncated to fit {token_budget}-token budget]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_report_unchanged() {
        let report = "short report";
        assert_eq!(truncate_to_token_budget(report, 8_000), report);
    }

    #[test]
    fn truncate_cuts_long_report_and_adds_note() {
        let long = "a".repeat(5_000);
        let out = truncate_to_token_budget(&long, 1_000);
        assert!(out.chars().count() < 5_000);
        assert!(out.contains("[truncated"));
    }

    #[test]
    fn web_fetcher_tool_set_registers_both_tools() {
        struct NoopBrowsing;
        impl LlmClient for NoopBrowsing {
            fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
                Ok(LlmModelTurn {
                    content: Some("noop".to_string()),
                    tool_calls: vec![],
                })
            }
        }
        let tools = web_fetcher_tool_set(NoopBrowsing, 8_000);
        let names: Vec<_> = tools
            .definitions()
            .into_iter()
            .map(|d| d.name.clone())
            .collect();
        assert_eq!(
            names,
            vec!["search_web".to_string(), "finalize".to_string()]
        );
    }
}
