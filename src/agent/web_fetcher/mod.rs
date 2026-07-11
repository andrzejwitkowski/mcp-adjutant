pub mod cache_flow;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use super::orchestrator::run_single_tool_turn;
use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::WebFetcherProfile;
use crate::llm::{required_str, LlmClient, LlmTool, LlmToolSet, ToolDefinition};
use crate::tools::web_fetch::{search_and_fetch, FetchedPage};

pub use cache_flow::{run_web_fetch_with_cache, WebCacheOutcome};

pub const WEB_FETCHER_SYSTEM_PROMPT: &str = r#"You are an autonomous web research agent (WEB_FETCHER). Your goal is to produce a compacted, accurate markdown document of the latest, authoritative web content for a given topic. The topic can be anything the user asks about; adapt your search approach to the kind of information it requires.

Available tools (call exactly one per turn):
- search_web(query, focus?) — search the web via Brave Search, fetch the top result pages, and return grounded markdown with inline source links. Use `focus` to narrow the search. Non-terminal: results are added to your observation history.
- finalize(report) — end research and return your compacted markdown report.

Strategy:
1. Call search_web with the clearest possible query for the topic.
2. If the results are incomplete, refine the query (more specific terms, add a year, name the canonical source) and call search_web again. You may search up to the hop limit.
3. Once you have enough grounded material, call finalize with a single compacted markdown document: keep the authoritative facts relevant to the topic, drop filler, preserve any source links inline as markdown links.

Efficiency: prefer 1-2 well-targeted searches. Do not repeat the same query. Reply with a short Thought, then call exactly one tool."#;

const MAX_PAGES_PER_SEARCH: usize = 3;
const CHARS_PER_TOKEN: usize = 4;

pub struct WebFetcherAgent<RC: LlmClient> {
    reasoning_client: RC,
    tools: LlmToolSet,
    source_collector: Arc<Mutex<Vec<FetchedPage>>>,
}

impl<RC: LlmClient> WebFetcherAgent<RC> {
    pub fn new(reasoning_client: RC, profile: WebFetcherProfile) -> Self {
        let token_budget = profile.token_budget;
        let brave_api_key = profile.brave_api_key.clone();
        let source_collector = Arc::new(Mutex::new(Vec::new()));
        let tools =
            web_fetcher_tool_set(token_budget, brave_api_key, Arc::clone(&source_collector));
        Self {
            reasoning_client,
            tools,
            source_collector,
        }
    }

    pub fn take_sources(&self) -> Vec<FetchedPage> {
        self.source_collector
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default()
    }
}

#[async_trait]
impl<RC: LlmClient> AutonomousAgent for WebFetcherAgent<RC> {
    fn name(&self) -> &'static str {
        "web_fetcher_agent"
    }

    async fn enrich_context(&self, _context: &mut AgentContext) -> Result<(), String> {
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
        if context.iterations >= context.max_iterations.saturating_sub(1) {
            context.input_prompt.push_str(
                "\nFinal turn: call finalize(report) with your best compacted markdown report.",
            );
        } else {
            context
                .input_prompt
                .push_str("\nContinue research. Call exactly one tool: search_web or finalize.");
        }
        Ok(())
    }
}

fn web_fetcher_tool_set(
    token_budget: u32,
    brave_api_key: Option<String>,
    source_collector: Arc<Mutex<Vec<FetchedPage>>>,
) -> LlmToolSet {
    LlmToolSet::new()
        .register(SearchWebTool::new(brave_api_key, source_collector))
        .register(FinalizeWebTool::with_budget(token_budget))
}

struct SearchWebTool {
    brave_api_key: Option<String>,
    source_collector: Arc<Mutex<Vec<FetchedPage>>>,
    definition: ToolDefinition,
}

impl SearchWebTool {
    fn new(brave_api_key: Option<String>, source_collector: Arc<Mutex<Vec<FetchedPage>>>) -> Self {
        Self {
            brave_api_key,
            source_collector,
            definition: ToolDefinition::new(
                "search_web",
                "Search the live web via Brave Search and return grounded, cited markdown.",
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

impl LlmTool for SearchWebTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        let query = required_str(arguments, "query")?;
        let focus = arguments
            .get("focus")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let full_query = match focus {
            Some(focus) => format!("{query} {focus}"),
            None => query.clone(),
        };

        let (markdown, pages) = run_search_blocking(&full_query, self.brave_api_key.as_deref())?;

        if let Ok(mut guard) = self.source_collector.lock() {
            guard.extend(pages);
        }

        Ok(markdown)
    }
}

fn run_search_blocking(
    query: &str,
    brave_api_key: Option<&str>,
) -> Result<(String, Vec<FetchedPage>), String> {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                tokio::task::spawn_blocking({
                    let query = query.to_string();
                    let api_key = brave_api_key.map(str::to_string);
                    move || search_and_fetch(&query, MAX_PAGES_PER_SEARCH, api_key.as_deref())
                })
                .await
                .map_err(|err| format!("web search task failed: {err}"))?
            })
        })
    } else {
        search_and_fetch(query, MAX_PAGES_PER_SEARCH, brave_api_key)
    }
}

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
        let collector = Arc::new(Mutex::new(Vec::new()));
        let tools = web_fetcher_tool_set(8_000, None, collector);
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
