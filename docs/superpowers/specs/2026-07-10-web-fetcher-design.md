# Web Fetcher Agent — Design Spec

**Date:** 2026-07-10
**Status:** Approved (pending spec review)
**Author:** brainstorming session

## Purpose

Add a `web_fetcher` agent to mcp-adjutant that accepts a **search phrase** and
returns a **compacted markdown document** of the latest documentation / web
content on that topic. Intended as the primary tool for fetching up-to-date
documentation from the web.

## Scope decisions (locked)

| Decision | Choice |
|---|---|
| Agent type | LLM-driven agent loop (full `AutonomousAgent`) |
| Input | **Search phrases only** — no URL input |
| Web access | Browsing-capable model (OpenRouter `:online` / Perplexity Sonar) |
| Loop style | Full multi-hop tool loop |
| Search backend | None of our own — the browsing model does web access server-side |

### Out of scope

- URL input / `fetch_url` tool / HTML→markdown crate. We never fetch or parse
  HTML ourselves; the browsing model returns grounded markdown.
- A search-API key of our own (Tavily / Brave / Serper).
- Precise tokenizer-based budgeting (character/truncation cut for v1).

## Architecture

### Two-tier model design

```
                 ┌─────────────────────────────────────┐
   search_phrase │  WebFetcherAgent (reasoning model)   │
       ────────▶ │  drives AutonomousAgent loop          │
                 │                                       │
                 │  tools:                               │
                 │   search_web(query, focus?) ──┐       │
                 │   finalize(report) ◀──────────┤       │
                 └────────────────────────────────┼─────┘
                                              │
                       ┌──────────────────────▼─────────────────────┐
                       │  Browsing model (web-capable)               │
                       │  OpenRouter :online / Perplexity Sonar      │
                       │  receives a strong research prompt,         │
                       │  returns grounded, cited markdown           │
                       └─────────────────────────────────────────────┘
```

- **Reasoning model** — the agent's `self.client` (the `WebFetcher`
  `AgentPhase` profile). A cheap model that drives the `AutonomousAgent` loop:
  it reasons about what to search, calls `search_web` (possibly several times —
  multi-hop: refine query → dig deeper), then calls `finalize`.
- **Browsing model** — invoked *inside* the `search_web` tool. A web-capable
  model. Receives a comprehensive "search the web for X, return authoritative
  cited markdown" prompt and returns grounded markdown. This is the "proxy to
  cheaper model" mechanism: the reasoning agent hands it a refined phrase +
  strong prompt.

Both tiers reuse the existing OpenAI-compatible transport (`DeepSeekClient` /
`ureq`). **No new HTTP or HTML/markdown crate is required.**

### Why no new crate

Because the browsing model does its own HTTP server-side and returns grounded
markdown, we never need to fetch URLs or convert HTML ourselves. This
eliminates the `html2md`/`htmd`/`scraper` dependency that a URL-fetching design
would have required.

## Construction

`WebFetcherAgent<RC, BC>` is constructed in `handle_web_fetch` with two clients
built from config:

- `RC` (reasoning) — `create_web_fetcher_llm_client(config)` →
  `AgentPhase::WebFetcher` profile.
- `BC` (browsing) — a second client built from `config.web_fetcher.browsing`
  (`PhaseProfile`), via the existing `create_llm_client(profile)` factory fn.

The `BC` client is moved into the `search_web` tool at construction time
(`web_fetcher_tool_set(browsing_client)`), so the tool calls the browsing model
without the agent loop touching it directly. The agent's `self.client` (the
reasoning `RC`) drives only the tool-selection loop.

## Tools (`web_fetcher_tool_set(browsing_client)`)

1. **`search_web(query, focus?)`** — non-terminal.
   - Builds the research prompt from `query` (and optional `focus`).
   - Calls the browsing model via the `BC: LlmClient` it holds.
   - Returns the browsing model's grounded markdown to be appended to
     `accumulated_data`.
   - Multi-hop: the agent may call repeatedly with refined queries.

2. **`finalize(report)`** — terminal.
   - Returns the agent's compacted markdown report.
   - Truncates to the token budget if the report exceeds it, with a truncation
     note appended when cut.

### Compaction

No tokenizer crate dependency for v1. The `finalize` tool applies a
deterministic character-budget cut (reuse the `TextPrunerMock` prune idiom) on
its `report` argument before returning. A truncation note is appended if the
report is cut. Precise token counting via the existing `LocalEmbeddingEngine`
tokenizer can be added later if needed.

## Configuration

Reuse the standard phase system for the **reasoning** model: add
`AgentPhase::WebFetcher` + a default `PhaseProfile`.

The **browsing** model + tunables live in an optional block on
`AdjutantConfig`, mirroring how `triage_overrides` is already an opt-in extra
field, so old configs upgrade via `serde(default)` +
`merge_missing_from_defaults`:

```rust
#[serde(default)] pub web_fetcher: Option<WebFetcherProfile>,

pub struct WebFetcherProfile {
    pub browsing: PhaseProfile,   // the browsing-capable model
    pub max_search_hops: u32,     // default 3 — orchestrator max_iterations cap
    pub token_budget: u32,        // default 8000 — finalize truncation target
}
```

## MCP tool

`web_fetch` — input `{ search_phrase: String, request_uuid: String }`.

Spawns a tracked async job (copy `handle_scout_context`'s shape), runs
`AgentLoopOrchestrator::run` on a `WebFetcherAgent` wired with both clients.
The caller polls `query_job_status` with the same UUID. No URL param.

## Files to touch

1. `src/agent/web_fetcher.rs` — `WebFetcherAgent<RC, BC>` impl
   `AutonomousAgent` + `WEB_FETCHER_SYSTEM_PROMPT`.
2. `src/agent/web_fetcher/tools.rs` — `search_web` tool (holds `BC: LlmClient`)
   + `finalize` tool; `web_fetcher_tool_set(browsing_client)`.
3. `src/agent/mod.rs` + `src/lib.rs` — declare/re-export.
4. `src/domain.rs` — `AgentPhase::WebFetcher` + default profile;
   `WebFetcherProfile` + default; merge logic.
5. `src/llm/factory.rs` — `create_web_fetcher_llm_client(config)` for the
   reasoning model.
6. `src/mcp/handlers.rs` — `WEB_FETCH_TOOL_NAME`, `web_fetch_schema()`,
   register in `registered_mcp_tools()`, `handle_web_fetch(...)`.
7. `src/mcp_server.rs:163` — add the match arm.
8. `frontend/src/modules/config-ui/types.ts` + `ConfigApp.tsx` — add
   `'web_fetcher'` phase + panel.
9. `tests/web_fetcher_tests.rs` — `ScriptClient` mocks for *both* tiers (no
   network), fixture grounded-markdown strings.

## Testing

Follow the existing `ScriptClient` mock pattern (`tests/scout_agent_tests.rs`):

- Mock the reasoning `LlmClient` to return scripted `LlmModelTurn`s with
  `search_web` and `finalize` tool calls.
- Mock the browsing `LlmClient` (a separate, simpler mock) to return fixture
  grounded-markdown strings — no network access.
- Assert: the agent calls `search_web` then `finalize`; the loop terminates;
  the output is the finalized report (with truncation applied if over budget).
- Add a fixture under `tests/fixtures/web_fetcher/` for sample grounded markdown.

## Success criteria

- `web_fetch` MCP tool appears in `tools/list`.
- With valid config (reasoning + browsing profiles + keys), a search phrase
  returns a compacted markdown document grounded in live web content.
- All existing tests continue to pass; new unit/integration tests cover the
  agent loop and both tool behaviors offline.
- Config UI shows a `web_fetcher` panel; persisted configs without the new
  fields upgrade gracefully.
