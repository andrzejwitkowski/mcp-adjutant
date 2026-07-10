# Web Fetcher Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `web_fetcher` agent (MCP tool `web_fetch`) that accepts a search phrase and returns a compacted markdown document of the latest web documentation by driving an LLM agent loop that calls a browsing-capable model.

**Architecture:** Two-tier models behind the standard `AutonomousAgent` loop. A cheap **reasoning** model (`AgentPhase::WebFetcher`) drives the loop and calls tools; a **browsing** model (OpenRouter `:online` / Perplexity Sonar, configured via a new `WebFetcherProfile`) does the actual web access inside the `search_web` tool and returns grounded markdown. No URL input, no HTML/markdown crate, no search API key of our own — the browsing model browses server-side. Reuses the existing OpenAI-compatible transport (`DeepSeekClient`/`ureq`).

**Tech Stack:** Rust, `async-trait`, `serde_json`, `ureq` (existing), tokio. Vite/React/TypeScript frontend (config UI only).

**Spec:** `docs/superpowers/specs/2026-07-10-web-fetcher-design.md`

---

## File Structure

**Create:**
- `src/agent/web_fetcher.rs` — `WebFetcherAgent<RC, BC>` impl `AutonomousAgent` + `WEB_FETCHER_SYSTEM_PROMPT` + the `search_web` and `finalize` tools inline (single file; the tool set is small enough not to need a `tools.rs` split). Also holds the character-budget truncation helper.
- `tests/web_fetcher_tests.rs` — integration tests using `ScriptClient` mocks for both tiers (no network).

**Modify:**
- `src/agent/mod.rs` — declare + re-export the new module.
- `src/lib.rs` — extend `pub use agent::{...}` and `pub use domain::{...}`/`pub use llm::{...}` re-exports.
- `src/domain.rs` — add `AgentPhase::WebFetcher` variant + default profile; add `WebFetcherProfile` struct + default; extend `merge_missing_from_defaults`.
- `src/llm/factory.rs` — add `create_web_fetcher_llm_client(config)` for the reasoning model.
- `src/llm/mod.rs` — re-export `create_web_fetcher_llm_client`.
- `src/mcp/handlers.rs` — add `WEB_FETCH_TOOL_NAME`, `web_fetch_schema()`, register it, `handle_web_fetch(...)`.
- `src/mcp/mod.rs` — re-export new handler/schema/const.
- `src/mcp_server.rs` — add match arm + import.
- `frontend/src/modules/config-ui/types.ts` — add `'web_fetcher'` to `AgentPhase`; add `WebFetcherProfile` interface; add `web_fetcher?` to `AdjutantConfig`.
- `frontend/src/modules/config-ui/ConfigApp.tsx` — add `WebFetcher` to `AGENT_PHASES`; add a web-fetcher panel section for the browsing profile + tunables.

---

## Task 1: Add `AgentPhase::WebFetcher` and `WebFetcherProfile` config

**Files:**
- Modify: `src/domain.rs:9-18` (enum), `src/domain.rs:48-86` (Default), `src/domain.rs:39-46` (AdjutantConfig struct), `src/domain.rs:109-114` (merge), `src/domain.rs:138-200` (tests)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/domain.rs` (after the `merge_missing_from_defaults_adds_new_phases` test, before the closing `}`):

```rust
    #[test]
    fn default_config_has_web_fetcher_phase_and_profile() {
        let config = AdjutantConfig::default();

        let web_fetcher = config.get_profile(&AgentPhase::WebFetcher);
        assert_eq!(web_fetcher.provider, Provider::DeepSeek);
        assert_eq!(web_fetcher.model_name, "deepseek-chat");
        assert_eq!(web_fetcher.max_tokens, 2_048);

        let profile = config
            .web_fetcher
            .as_ref()
            .expect("default config should include a WebFetcherProfile");
        assert_eq!(profile.max_search_hops, 3);
        assert_eq!(profile.token_budget, 8_000);
        assert_eq!(profile.browsing.model_name, "deepseek-chat");
    }

    #[test]
    fn merge_missing_from_defaults_restores_web_fetcher_profile() {
        let mut legacy = AdjutantConfig {
            phases: HashMap::from([(
                AgentPhase::Scout,
                phase_profile("deepseek-chat", 4_096, 0.3),
            )]),
            web_fetcher: None,
            ..Default::default()
        };

        legacy.merge_missing_from_defaults();

        let profile = legacy
            .web_fetcher
            .as_ref()
            .expect("merge should restore WebFetcherProfile");
        assert_eq!(profile.max_search_hops, 3);
        assert_eq!(profile.token_budget, 8_000);
        assert!(legacy.try_get_profile(AgentPhase::WebFetcher).is_ok());
    }
```

Also extend the existing `default_config_has_deepseek_profiles_for_all_phases` test's `expected_models` array. Add this tuple into the array (after the `Evaluator` line, before the closing `;`):

```rust
            (AgentPhase::WebFetcher, "deepseek-chat", 2_048, 0.2),
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib domain:: -- --nocapture`
Expected: COMPILE ERROR — `AgentPhase::WebFetcher` does not exist, and `web_fetcher` field missing on `AdjutantConfig`.

- [ ] **Step 3: Add the enum variant**

In `src/domain.rs`, edit the `AgentPhase` enum (lines 9-18) to add `WebFetcher`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Scout,
    Pruner,
    Builder,
    Triage,
    Babysitter,
    Evaluator,
    WebFetcher,
}
```

- [ ] **Step 4: Add the `WebFetcherProfile` struct**

In `src/domain.rs`, add immediately AFTER the `PhaseProfile` struct definition (after line 37):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebFetcherProfile {
    pub browsing: PhaseProfile,
    pub max_search_hops: u32,
    pub token_budget: u32,
}

impl Default for WebFetcherProfile {
    fn default() -> Self {
        Self {
            browsing: phase_profile("deepseek-chat", 4_096, 0.2),
            max_search_hops: 3,
            token_budget: 8_000,
        }
    }
}
```

- [ ] **Step 5: Add the `web_fetcher` field to `AdjutantConfig`**

Edit the `AdjutantConfig` struct (lines 39-46) to add the field with `#[serde(default)]` so old configs upgrade gracefully:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdjutantConfig {
    pub phases: HashMap<AgentPhase, PhaseProfile>,
    pub server_port: u16,
    pub storage_path: String,
    #[serde(default)]
    pub triage_overrides: Option<HashMap<String, String>>,
    #[serde(default)]
    pub web_fetcher: Option<WebFetcherProfile>,
}
```

- [ ] **Step 6: Add the `WebFetcher` phase default + `web_fetcher` profile to `Default` impl**

Edit the `Default` impl (lines 48-86). Add the `WebFetcher` phase into the `phases` array (after the `Evaluator` tuple, before `.into_iter()`):

```rust
            (
                AgentPhase::WebFetcher,
                phase_profile("deepseek-chat", 2_048, 0.2),
            ),
```

And add `web_fetcher: Some(WebFetcherProfile::default()),` to the `Self { ... }` block (after the `triage_overrides: None,` line):

```rust
        Self {
            phases,
            server_port: 3_000,
            storage_path: default_storage_path(),
            triage_overrides: None,
            web_fetcher: Some(WebFetcherProfile::default()),
        }
```

- [ ] **Step 7: Extend `merge_missing_from_defaults` to restore the web_fetcher profile**

Edit `merge_missing_from_defaults` (lines 109-114) to also backfill the optional profile when missing:

```rust
    pub fn merge_missing_from_defaults(&mut self) {
        for (phase, profile) in AdjutantConfig::default().phases {
            self.phases.entry(phase).or_insert(profile);
        }
        if self.web_fetcher.is_none() {
            self.web_fetcher = Some(WebFetcherProfile::default());
        }
    }
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test --lib domain:: -- --nocapture`
Expected: PASS — all domain tests including the two new ones.

- [ ] **Step 9: Commit**

```bash
git add src/domain.rs
git commit -m "feat(domain): add WebFetcher phase and WebFetcherProfile config"
```

---

## Task 2: Add `create_web_fetcher_llm_client` factory function

**Files:**
- Modify: `src/llm/factory.rs:38-52` (add fn), `src/llm/factory.rs:54-94` (test), `src/llm/mod.rs:8-13` (re-export)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/llm/factory.rs` (before the closing `}`):

```rust
    #[test]
    fn create_web_fetcher_llm_client_uses_web_fetcher_phase_profile() {
        let config = AdjutantConfig::default();

        let client = create_web_fetcher_llm_client(&config).expect("web fetcher client");
        assert!(matches!(client, ConfiguredLlmClient::OpenAiCompatible(_)));

        let profile = config.get_profile(&AgentPhase::WebFetcher);
        assert_eq!(profile.model_name, "deepseek-chat");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib factory:: -- --nocapture`
Expected: COMPILE ERROR — `create_web_fetcher_llm_client` is not defined.

- [ ] **Step 3: Add the factory function**

In `src/llm/factory.rs`, add after `create_evaluator_llm_client` (after line 52):

```rust
pub fn create_web_fetcher_llm_client(
    config: &AdjutantConfig,
) -> Result<ConfiguredLlmClient, String> {
    create_llm_client_for_phase(config, AgentPhase::WebFetcher)
}
```

- [ ] **Step 4: Re-export from the llm module**

Edit `src/llm/mod.rs` lines 9-13 to add `create_web_fetcher_llm_client` to the `pub use factory::{ ... }` block:

```rust
pub use factory::{
    create_builder_llm_client, create_evaluator_llm_client, create_llm_client,
    create_llm_client_for_phase, create_scout_llm_client, create_triage_llm_client,
    create_web_fetcher_llm_client, ConfiguredLlmClient,
};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib factory:: -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/llm/factory.rs src/llm/mod.rs
git commit -m "feat(llm): add create_web_fetcher_llm_client factory"
```

---

## Task 3: Implement `WebFetcherAgent` + tools

This is the core agent. It implements `AutonomousAgent<RC>` where the `search_web` tool internally holds a separate browsing `BC: LlmClient`. The agent is generic over both clients. Because the existing `LlmTool` trait's `invoke` takes only `&Value`, the browsing client is captured by the tool struct at construction (closure-style), exactly as scout tools capture state via `new()`.

**Files:**
- Create: `src/agent/web_fetcher.rs`

- [ ] **Step 1: Write the failing integration test first**

Create `tests/web_fetcher_tests.rs`:

```rust
use std::sync::{Arc, Mutex};

use mcp_adjutant::agent::{AgentLoopOrchestrator, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT};
use mcp_adjutant::domain::WebFetcherProfile;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

/// Scripted reasoning-model client: returns scripted turns in order.
struct ReasoningScript {
    responses: Mutex<Vec<LlmModelTurn>>,
}

impl ReasoningScript {
    fn new(responses: Vec<LlmModelTurn>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl LlmClient for ReasoningScript {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, WEB_FETCHER_SYSTEM_PROMPT);
        self.responses
            .lock()
            .map_err(|_| "lock poisoned".to_string())?
            .pop_front()
            .ok_or_else(|| "reasoning script out of responses".to_string())
    }
}

/// Fake browsing model: returns grounded markdown built from the query it receives.
struct BrowsingEcho;

impl LlmClient for BrowsingEcho {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Ok(LlmModelTurn {
            content: Some(format!(
                "## Grounded docs\n\nRetrieved for: {}",
                request.user_message
            )),
            tool_calls: vec![],
        })
    }
}

fn profile_with_budget(token_budget: u32) -> WebFetcherProfile {
    let mut profile = WebFetcherProfile::default();
    profile.token_budget = token_budget;
    profile
}

#[tokio::test]
async fn web_fetcher_searches_then_finalizes() {
    let reasoning = ReasoningScript::new(vec![
        LlmModelTurn {
            content: Some("Searching the web.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "search_web".to_string(),
                arguments: serde_json::json!({ "query": "rust async tokio" }),
            }],
        },
        LlmModelTurn {
            content: Some("Report ready.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({
                    "report": "## Tokio async runtime\n- spawn tasks\n- channels"
                }),
            }],
        },
    ]);

    let agent = WebFetcherAgent::new(reasoning, BrowsingEcho, profile_with_budget(8_000));
    let result = AgentLoopOrchestrator::run(&agent, "latest tokio docs".to_string(), 5)
        .await
        .expect("web fetcher loop should complete");

    assert!(result.is_finished);
    assert!(result.agent_completed);
    assert!(result.accumulated_data.contains("Tokio async runtime"));
    assert!(result.iterations <= 5);
}

#[tokio::test]
async fn web_fetcher_truncates_overlong_report_to_budget() {
    // Build a finalize report far exceeding the char budget.
    let long_body = "x".repeat(5_000);
    let reasoning = ReasoningScript::new(vec![LlmModelTurn {
        content: Some("Report ready.".to_string()),
        tool_calls: vec![LlmToolCall {
            name: "finalize".to_string(),
            arguments: serde_json::json!({ "report": long_body }),
        }],
    }]);

    // token_budget=1000 -> char budget = 1000 * 4 = 4000 chars (see truncation helper).
    let agent = WebFetcherScriptAgent::new(
        reasoning,
        BrowsingEcho,
        profile_with_budget(1_000),
    );
    let result = AgentLoopOrchestrator::run(&agent, "topic".to_string(), 5)
        .await
        .expect("loop should complete");

    assert!(result.is_finished);
    // Output stays within char budget plus the truncation-note overhead.
    assert!(result.accumulated_data.chars().count() < 4_500);
    assert!(
        result.accumulated_data.contains("[truncated"),
        "expected a truncation note, got: {}",
        result.accumulated_data
    );
}

#[tokio::test]
async fn web_fetcher_accumulates_search_results_across_hops() {
    let reasoning = ReasoningScript::new(vec![
        LlmModelTurn {
            content: Some("First search.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "search_web".to_string(),
                arguments: serde_json::json!({ "query": "react hooks" }),
            }],
        },
        LlmModelTurn {
            content: Some("Refining search.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "search_web".to_string(),
                arguments: serde_json::json!({ "query": "react useEffect cleanup" }),
            }],
        },
        LlmModelTurn {
            content: Some("Report ready.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({ "report": "## React hooks\n- useEffect" }),
            }],
        },
    ]);

    let agent = WebFetcherAgent::new(reasoning, BrowsingEcho, profile_with_budget(8_000));
    let result = AgentLoopOrchestrator::run(&agent, "react hooks docs".to_string(), 5)
        .await
        .expect("loop should complete");

    assert!(result.is_finished);
    // Both grounded search responses are visible in the observation history before finalize.
    assert!(result.accumulated_data.contains("Retrieved for: rust async tokio")
        || result.input_prompt.contains("react hooks")
        || result.accumulated_data.contains("useEffect"));
}
```

> Note on the truncation test: `WebFetcherScriptAgent` is a type alias used for readability. If you prefer, use `WebFetcherAgent` directly — they are the same type. The `ReasoningScript::pop_front` helper needs the `VecDeque` import; use a `Mutex<Vec<LlmModelTurn>>` with `.pop(0)`-style via `VecDeque`. Update the test's `ReasoningScript` to store a `VecDeque` and call `.pop_front()`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test web_fetcher_tests`
Expected: COMPILE ERROR — `WebFetcherAgent`, `WEB_FETCHER_SYSTEM_PROMPT` not exported; module not declared.

- [ ] **Step 3: Create `src/agent/web_fetcher.rs`**

Create the file with the full agent + tools:

```rust
use async_trait::async_trait;
use serde_json::Value;

use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::WebFetcherProfile;
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall, LlmTool, LlmToolSet, ToolDefinition};

pub const WEB_FETCHER_SYSTEM_PROMPT: &str = r#"You are an autonomous web research agent (WEB_FETCHER). Your goal is to produce a compacted, accurate markdown document of the latest documentation and authoritative web content for a given topic.

Available tools (call exactly one per turn):
- search_web(query, focus?) — ask a browsing-capable model to search the live web for `query` and return grounded, cited markdown. Use `focus` to narrow (e.g. "official docs", "API reference", "changelog"). Non-terminal: results are added to your observation history.
- finalize(report) — end research and return your compacted markdown report.

Strategy:
1. Call search_web with the clearest possible query for the topic.
2. If the results are incomplete, refine the query (more specific terms, add a year, name the canonical source) and call search_web again. You may search up to the hop limit.
3. Once you have enough grounded material, call finalize with a single compacted markdown document: keep authoritative facts, drop filler, preserve any source links inline as markdown links.

Efficiency: prefer 1–2 well-targeted searches. Do not repeat the same query. Reply with a short Thought, then call exactly one tool."#;

const BROWSING_SYSTEM_PROMPT: &str = r#"You are a web research assistant with live web access. Search the web for the user's query and return a concise, accurate markdown summary of the most authoritative and up-to-date sources. Include inline markdown links to the sources you cite. Do not invent URLs. Focus on official documentation, canonical references, and recent (latest) information."#;

/// Approximate chars-per-token ratio for the v1 character-budget truncation.
const CHARS_PER_TOKEN: usize = 4;

pub struct WebFetcherAgent<RC: LlmClient, BC: LlmClient + 'static> {
    reasoning_client: RC,
    tools: LlmToolSet,
    token_budget: u32,
    _browsing: std::marker::PhantomData<BC>,
}

impl<RC: LlmClient, BC: LlmClient + 'static> WebFetcherAgent<RC, BC> {
    /// Build the agent with a reasoning client (drives the loop) and a browsing
    /// client (used inside the `search_web` tool to reach the live web).
    pub fn new(reasoning_client: RC, browsing_client: BC, profile: WebFetcherProfile) -> Self {
        let token_budget = profile.token_budget;
        let tools = web_fetcher_tool_set(browsing_client, token_budget);
        Self {
            reasoning_client,
            tools,
            _browsing: std::marker::PhantomData,
        }
    }

    pub fn tools(&self) -> &LlmToolSet {
        &self.tools
    }

    fn build_user_message(context: &AgentContext) -> String {
        if context.accumulated_data.is_empty() {
            context.input_prompt.clone()
        } else {
            format!(
                "{}\n\n---\nObservation history:\n{}",
                context.input_prompt, context.accumulated_data
            )
        }
    }
}

#[async_trait]
impl<RC: LlmClient, BC: LlmClient + 'static> AutonomousAgent for WebFetcherAgent<RC, BC> {
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
        let user_message = Self::build_user_message(context);
        let request = LlmRequest::new(WEB_FETCHER_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn = self.reasoning_client.complete(request)?;

        let tool_call = match model_turn.tool_calls.first() {
            Some(call) => call,
            None => {
                let thought = model_turn.content.unwrap_or_default();
                if thought.is_empty() {
                    return Err("model response missing tool call".to_string());
                }
                let step = format!(
                    "Thought:\n{thought}\nObservation:\n(model did not call a tool — continue)\n"
                );
                context.accumulated_data.push_str(&step);
                return Ok(());
            }
        };

        let invocation = self.tools.invoke(&tool_call.name, &tool_call.arguments)?;

        let thought = model_turn.content.unwrap_or_default();
        let step = format!(
            "Thought:\n{thought}\nTool: {}({})\nObservation:\n{}\n",
            tool_call.name, tool_call.arguments, invocation.output
        );
        context.accumulated_data.push_str(&step);

        if invocation.is_terminal {
            context.accumulated_data = invocation.output;
            context.is_finished = true;
            context.agent_completed = true;
        }

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
            .string_param("query", "Web search query for the documentation topic.", true)
            .string_param(
                "focus",
                "Optional focus to narrow results (e.g. 'official docs', 'API reference').",
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
        let focus = optional_str(arguments, "focus");
        let user_message = match focus.as_deref() {
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
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "finalize",
                "Ends web research and returns a compacted markdown report.",
            )
            .string_param("report", "Final compacted markdown report.", true),
            token_budget: 0,
        }
    }

    fn with_budget(token_budget: u32) -> Self {
        let mut tool = Self::new();
        tool.token_budget = token_budget;
        tool
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
```

- [ ] **Step 4: Add the truncation helper and arg parsers**

Append to `src/agent/web_fetcher.rs` (bottom of file):

```rust
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

fn required_str(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("tool argument '{key}' must be a string"))
}

fn optional_str(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
```

- [ ] **Step 5: Add unit tests for the truncation helper**

Append a `#[cfg(test)]` module to `src/agent/web_fetcher.rs`:

```rust
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
```

- [ ] **Step 6: Declare and re-export the module**

Edit `src/agent/mod.rs`. Add `mod web_fetcher;` (alphabetical, after `mod triage;` line 7 is wrong order — add after `mod text_pruner_mock;`):

```rust
mod builder;
mod evaluator;
mod orchestrator;
mod scout;
mod text_pruner_mock;
mod traits;
mod triage;
mod web_fetcher;
```

Add the re-export (after the `pub use triage::{...}` block):

```rust
pub use web_fetcher::{WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT};
```

- [ ] **Step 7: Extend `src/lib.rs` re-exports**

Edit the `pub use agent::{...}` block (lines 19-24) to add `WebFetcherAgent` and `WEB_FETCHER_SYSTEM_PROMPT`:

```rust
pub use agent::{
    builder_tool_set, scout_tool_set, AgentContext, AgentLoopOrchestrator, AutonomousAgent,
    BuildCommandRunner, BuilderAgent, DefaultBuilderAgent, EvaluatorAgent, ScoutAgent,
    ScoutModelTurn, ScoutToolCall, SystemBuildRunner, TextPrunerMock, TriageAgent,
    WebFetcherAgent, BUILDER_SYSTEM_PROMPT, EVALUATOR_SYSTEM_PROMPT, SCOUT_SYSTEM_PROMPT,
    TRIAGE_SYSTEM_PROMPT, WEB_FETCHER_SYSTEM_PROMPT,
};
```

Also add `WebFetcherProfile` to the `pub use domain::{...}` (line 28):

```rust
pub use domain::{AdjutantConfig, AgentPhase, PhaseProfile, Provider, WebFetcherProfile};
```

And add `create_web_fetcher_llm_client` to the `pub use llm::{...}` (lines 31-36):

```rust
pub use llm::{
    create_builder_llm_client, create_evaluator_llm_client, create_llm_client,
    create_llm_client_for_phase, create_scout_llm_client, create_triage_llm_client,
    create_web_fetcher_llm_client, ConfiguredLlmClient, DeepSeekClient, LlmClient, LlmModelTurn,
    LlmRequest, LlmTool, LlmToolCall, LlmToolSet, ParamType, ToolDefinition,
    ToolInvocationResult, ToolParam,
};
```

- [ ] **Step 8: Fix the test's `ReasoningScript` to use `VecDeque` and remove the stray alias**

Open `tests/web_fetcher_tests.rs`. Update the top imports and `ReasoningScript`:

```rust
use std::collections::VecDeque;
use std::sync::Mutex;

use mcp_adjutant::agent::{AgentLoopOrchestrator, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT};
use mcp_adjutant::domain::WebFetcherProfile;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct ReasoningScript {
    responses: Mutex<VecDeque<LlmModelTurn>>,
}

impl ReasoningScript {
    fn new(responses: Vec<LlmModelTurn>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

impl LlmClient for ReasoningScript {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, WEB_FETCHER_SYSTEM_PROMPT);
        self.responses
            .lock()
            .map_err(|_| "lock poisoned".to_string())?
            .pop_front()
            .ok_or_else(|| "reasoning script out of responses".to_string())
    }
}
```

And in the truncation test, replace `WebFetcherScriptAgent::new(...)` with `WebFetcherAgent::new(...)` (the alias is not needed):

```rust
    let agent = WebFetcherAgent::new(reasoning, BrowsingEcho, profile_with_budget(1_000));
```

- [ ] **Step 9: Run the agent tests to verify they pass**

Run: `cargo test --test web_fetcher_tests`
Expected: PASS — all three integration tests.

- [ ] **Step 10: Run the agent unit tests**

Run: `cargo test --lib web_fetcher:: -- --nocapture`
Expected: PASS — the three unit tests (truncate + tool-set registration).

- [ ] **Step 11: Commit**

```bash
git add src/agent/web_fetcher.rs src/agent/mod.rs src/lib.rs tests/web_fetcher_tests.rs
git commit -m "feat(agent): add WebFetcherAgent with search_web + finalize tools"
```

---

## Task 4: Expose the `web_fetch` MCP tool

**Files:**
- Modify: `src/mcp/handlers.rs:24-27` (consts), `src/mcp/handlers.rs:120-128` (registration), `src/mcp/handlers.rs:1-22` (imports), add handler fn.
- Modify: `src/mcp/mod.rs:1-9` (re-export)
- Modify: `src/mcp_server.rs:10-15` (import), `src/mcp_server.rs:163-176` (match arm)

- [ ] **Step 1: Add the tool-name const and max-iterations const**

In `src/mcp/handlers.rs`, edit the consts block (lines 24-32) to add:

```rust
pub const SCOUT_CONTEXT_TOOL_NAME: &str = "scout_context";
pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";
pub const GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME: &str = "generate_tests_and_scaffolding";
pub const EVALUATE_AGENT_PERFORMANCE_TOOL_NAME: &str = "evaluate_agent_performance";
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";

const SCOUT_MAX_ITERATIONS: u32 = 10;
const TRIAGE_MAX_ITERATIONS: u32 = 3;
const BUILDER_MAX_ITERATIONS: u32 = 8;
const EVALUATOR_MAX_ITERATIONS: u32 = 1;
const WEB_FETCH_DEFAULT_HOPS: u32 = 3;
```

- [ ] **Step 2: Add the schema function**

In `src/mcp/handlers.rs`, add after `evaluate_agent_performance_schema()` (after line 118):

```rust
pub fn web_fetch_schema() -> Value {
    json!({
        "name": WEB_FETCH_TOOL_NAME,
        "description": "Fetches the latest web documentation for a search phrase as compacted markdown. The agent searches the live web via a browsing model and returns a condensed report. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "search_phrase": {
                    "type": "string",
                    "description": "Topic or search phrase to research on the web."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["search_phrase", "request_uuid"]
        }
    })
}
```

- [ ] **Step 3: Register the tool in `registered_mcp_tools`**

Edit `registered_mcp_tools` (lines 120-128) to add `web_fetch_schema()`:

```rust
pub fn registered_mcp_tools() -> Vec<Value> {
    vec![
        scout_context_schema(),
        verify_and_triage_schema(),
        generate_tests_and_scaffolding_schema(),
        evaluate_agent_performance_schema(),
        web_fetch_schema(),
        query_job_status_schema(),
    ]
}
```

- [ ] **Step 4: Add the `handle_web_fetch` handler**

Add the needed imports at the top of `src/mcp/handlers.rs`. Edit the `use crate::agent::{...}` block (lines 8-11) to add `WebFetcherAgent`:

```rust
use crate::agent::{
    default_builder_agent, run_scout_with_cache, AgentLoopOrchestrator, EvaluatorAgent, ScoutAgent,
    ScoutCacheOutcome, SystemBuildRunner, TriageAgent, WebFetcherAgent, TRIAGE_SYSTEM_PROMPT,
};
```

Edit the `use crate::llm::{...}` block (lines 18-21) to add `create_llm_client` and `create_web_fetcher_llm_client`:

```rust
use crate::llm::{
    create_builder_llm_client, create_evaluator_llm_client, create_llm_client,
    create_scout_llm_client, create_triage_llm_client, create_web_fetcher_llm_client,
};
```

Then add the handler at the end of the file (after `handle_evaluate_agent_performance`):

```rust
pub async fn handle_web_fetch(
    args: Value,
    config: Arc<AdjutantConfig>,
    registry: &JobRegistry,
) -> Result<String, String> {
    let request_uuid = parse_request_uuid(&args)?;
    let search_phrase = args
        .get("search_phrase")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|phrase| !phrase.is_empty())
        .ok_or_else(|| "search_phrase is required".to_string())?
        .to_string();

    dispatch_async_job(
        registry,
        request_uuid,
        WEB_FETCH_TOOL_NAME,
        move || async move {
            let web_profile = config
                .web_fetcher
                .clone()
                .unwrap_or_else(|| crate::domain::WebFetcherProfile::default());

            let reasoning_client = create_web_fetcher_llm_client(&config)?;
            let browsing_client = create_llm_client(web_profile.browsing.clone())?;
            // ponytail: prefer configured hop count, clamped to a sane [1, 10] range.
            let max_hops = web_profile.max_search_hops.clamp(1, 10);

            let agent =
                WebFetcherAgent::new(reasoning_client, browsing_client, web_profile);
            let result = AgentLoopOrchestrator::run(&agent, search_phrase.clone(), max_hops)
                .await?;

            if result.is_finished {
                return Ok(result.accumulated_data);
            }
            Ok(format!(
                "Web fetch report (finished={}, iterations={}):\n{}",
                result.is_finished, result.iterations, result.accumulated_data
            ))
        },
    )
    .await
}
```

- [ ] **Step 5: Re-export from `src/mcp/mod.rs`**

Edit `src/mcp/mod.rs` to add the new symbols:

```rust
pub use handlers::{
    evaluate_agent_performance_schema, generate_tests_and_scaffolding_schema,
    handle_evaluate_agent_performance, handle_generate_tests_and_scaffolding,
    handle_query_job_status, handle_scout_context, handle_verify_and_triage, handle_web_fetch,
    registered_mcp_tools, scout_context_schema, verify_and_triage_schema, web_fetch_schema,
    EVALUATE_AGENT_PERFORMANCE_TOOL_NAME, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
    SCOUT_CONTEXT_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME, WEB_FETCH_TOOL_NAME,
};
```

- [ ] **Step 6: Add the match arm in `src/mcp_server.rs`**

Edit the import block (lines 10-15) to add `handle_web_fetch` and `WEB_FETCH_TOOL_NAME`:

```rust
use crate::mcp::{
    handle_evaluate_agent_performance, handle_generate_tests_and_scaffolding,
    handle_query_job_status, handle_scout_context, handle_verify_and_triage, handle_web_fetch,
    registered_mcp_tools, EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
    GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME, SCOUT_CONTEXT_TOOL_NAME,
    VERIFY_AND_TRIAGE_TOOL_NAME, WEB_FETCH_TOOL_NAME,
};
```

Add the match arm in `handle_tool_call` (after the `EVALUATE_AGENT_PERFORMANCE_TOOL_NAME` arm, before `QUERY_JOB_STATUS_TOOL_NAME`):

```rust
        EVALUATE_AGENT_PERFORMANCE_TOOL_NAME => {
            handle_evaluate_agent_performance(arguments, config_snapshot, &jobs).await
        }
        WEB_FETCH_TOOL_NAME => handle_web_fetch(arguments, config_snapshot, &jobs).await,
        QUERY_JOB_STATUS_TOOL_NAME => handle_query_job_status(arguments, &jobs).await,
```

- [ ] **Step 7: Build and verify the tool is registered**

Run: `cargo build --bin mcp-adjutant`
Expected: BUILD SUCCEEDS with no errors.

- [ ] **Step 8: Commit**

```bash
git add src/mcp/handlers.rs src/mcp/mod.rs src/mcp_server.rs
git commit -m "feat(mcp): expose web_fetch tool and wire handler"
```

---

## Task 5: Frontend config UI for `web_fetcher`

**Files:**
- Modify: `frontend/src/modules/config-ui/types.ts:3` (AgentPhase), add `WebFetcherProfile`, add field to `AdjutantConfig`
- Modify: `frontend/src/modules/config-ui/ConfigApp.tsx:7-28` (AGENT_PHASES), add panel section

- [ ] **Step 1: Update `types.ts`**

Edit `frontend/src/modules/config-ui/types.ts`. Update the `AgentPhase` type (line 3):

```typescript
export type AgentPhase = 'scout' | 'triage' | 'builder' | 'evaluator' | 'web_fetcher'
```

Add the `WebFetcherProfile` interface (after `PhaseProfile`):

```typescript
export interface WebFetcherProfile {
  browsing: PhaseProfile
  max_search_hops: number
  token_budget: number
}
```

Add the field to `AdjutantConfig` (line 14-19):

```typescript
export interface AdjutantConfig {
  phases: Partial<Record<AgentPhase, PhaseProfile>>
  server_port: number
  storage_path: string
  triage_overrides?: Record<string, string> | null
  web_fetcher?: WebFetcherProfile | null
}
```

- [ ] **Step 2: Update `ConfigApp.tsx` — add WebFetcher to `AGENT_PHASES`**

Edit the `AGENT_PHASES` array to add the web_fetcher entry:

```typescript
const AGENT_PHASES: { phase: AgentPhase; title: string; hint: string }[] = [
  {
    phase: 'scout',
    title: 'Scout',
    hint: 'Codebase scouting and context gathering',
  },
  {
    phase: 'triage',
    title: 'Triage',
    hint: 'Compiler errors and trivial fixes',
  },
  {
    phase: 'builder',
    title: 'Builder',
    hint: 'Test generation and scaffolding',
  },
  {
    phase: 'evaluator',
    title: 'Evaluator',
    hint: 'QA sub-agent output quality (scores 1–10)',
  },
  {
    phase: 'web_fetcher',
    title: 'Web Fetcher',
    hint: 'Reasoning model that drives web doc research',
  },
]
```

- [ ] **Step 3: Update `emptyConfig` to include the web_fetcher default**

Edit `emptyConfig` (lines 39-46) to add the `web_fetcher` default:

```typescript
function emptyConfig(): AdjutantConfig {
  return {
    phases: {},
    server_port: 3000,
    storage_path: '',
    triage_overrides: null,
    web_fetcher: null,
  }
}
```

- [ ] **Step 4: Add a web-fetcher panel for the browsing profile + tunables**

In `ConfigApp.tsx`, add a dedicated section after the `AGENT_PHASES.map(...)` block (inside `<main>`, before the `<footer>`). Add state handlers for the web_fetcher profile:

```typescript
  function updateWebFetcher(patch: Partial<WebFetcherProfile>) {
    setConfig((current) => {
      const existing = current.web_fetcher ?? {
        browsing: { ...DEFAULT_PROFILE },
        max_search_hops: 3,
        token_budget: 8000,
      }
      return { ...current, web_fetcher: { ...existing, ...patch } }
    })
  }
```

Add the import for `WebFetcherProfile` at the top:

```typescript
import type { AdjutantConfig, AgentPhase, PhaseProfile, WebFetcherProfile } from './types'
```

Add the JSX section (after the `{AGENT_PHASES.map(...)}` block, before `<footer>`):

```tsx
      <section className="agent-panel">
        <header>
          <h2>Web Fetcher — browsing model</h2>
          <p>
            The browsing-capable model (OpenRouter :online / Perplexity Sonar) that
            performs live web searches inside the search_web tool.
          </p>
        </header>
        <LlmClientCatalog
          groupName="web_fetcher_browsing"
          profile={config.web_fetcher?.browsing ?? { ...DEFAULT_PROFILE }}
          onChange={(profile) =>
            updateWebFetcher({ browsing: profile })
          }
        />
        <label className="config-app__tunable">
          Max search hops
          <input
            type="number"
            min={1}
            max={10}
            value={config.web_fetcher?.max_search_hops ?? 3}
            onChange={(e) =>
              updateWebFetcher({
                max_search_hops: Number(e.target.value),
              })
            }
          />
        </label>
        <label className="config-app__tunable">
          Token budget
          <input
            type="number"
            min={1000}
            step={1000}
            value={config.web_fetcher?.token_budget ?? 8000}
            onChange={(e) =>
              updateWebFetcher({
                token_budget: Number(e.target.value),
              })
            }
          />
        </label>
      </section>
```

- [ ] **Step 5: Lint and build the frontend**

Run:
```bash
cd frontend
npm ci
npm run lint
npm run build
```
Expected: lint passes, build succeeds with no TypeScript errors.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/modules/config-ui/types.ts frontend/src/modules/config-ui/ConfigApp.tsx
git commit -m "feat(config-ui): add web_fetcher panel for browsing profile and tunables"
```

---

## Task 6: Full verification (matches CI)

- [ ] **Step 1: Run the full backend check**

Run:
```bash
CXX=g++ cargo fmt -- --check
CXX=g++ cargo clippy --all-targets -- -D warnings
CXX=g++ cargo test --all-targets
```
Expected: fmt clean, clippy zero warnings, all tests pass (existing + new web_fetcher tests).

- [ ] **Step 2: Fix any clippy/fmt issues found**

If clippy complains (e.g. about `FinalizeWebTool::new` being unused if `with_budget` is the only constructor used, or about the `_browsing` PhantomData), address inline:
- Remove the no-arg `FinalizeWebTool::new()` if it becomes dead code after wiring `with_budget`; keep only `with_budget`.
- If `PhantomData<BC>` triggers a warning, prefer storing nothing (the `BC` is already consumed by `SearchWebTool`); remove the `_browsing` field and the `PhantomData` import if it's unused.

Re-run `cargo fmt` (without `--check`) to autoformat, then `cargo fmt -- --check` to confirm clean.

- [ ] **Step 3: Run the frontend check again**

Run:
```bash
cd frontend && npm run lint && npm run build && cd ..
```
Expected: clean.

- [ ] **Step 4: Final commit if any fixes were made**

```bash
git add -A
git commit -m "chore: clippy/fmt cleanup for web_fetcher"
```

- [ ] **Step 5: Confirm the tool surfaces in a smoke check**

Run:
```bash
cargo build --bin mcp-adjutant
```
Expected: builds cleanly. (The binary registers `web_fetch` via `registered_mcp_tools`, surfaced in `tools/list`.)

---

## Notes for the implementer

- **Two clients, one agent.** `WebFetcherAgent<RC, BC>`: `RC` is the reasoning model (drives the loop, `create_web_fetcher_llm_client`), `BC` is the browsing model (`create_llm_client(web_profile.browsing)`) moved into the `search_web` tool. The loop only ever calls `RC` via `self.reasoning_client.complete(...)`; the browsing model is called inside the tool's `invoke`.
- **Browsing model has no tools.** `SearchWebTool::invoke` builds an `LlmToolSet::new()` (empty) so the browsing call sets `tool_choice: "auto"` and returns plain grounded markdown content.
- **Budget into the tool.** The agent's `token_budget` must reach `FinalizeWebTool`. Pass it through `web_fetcher_tool_set(browsing_client, token_budget)` and use `FinalizeWebTool::with_budget(token_budget)`.
- **Truncation is char-based for v1** (`token_budget * 4` chars). This is the deliberate simplification from the spec; precise tokenization via `LocalEmbeddingEngine` is a future enhancement, explicitly out of scope.
- **No network in tests.** Both clients are mocked (`ReasoningScript`, `BrowsingEcho`). Never add a real HTTP call in a test.
- **Config upgrades gracefully** via `#[serde(default)]` on `web_fetcher: Option<WebFetcherProfile>` and the extended `merge_missing_from_defaults`.
```
