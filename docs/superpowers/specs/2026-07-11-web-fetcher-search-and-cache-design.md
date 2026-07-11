# Web Fetcher: Real Search + Semantic Cache — Design Spec

**Date:** 2026-07-11
**Status:** Approved (pending spec review)
**Builds on:** `docs/superpowers/specs/2026-07-10-web-fetcher-design.md` (the agent skeleton)
**Parent branch:** `feat/web-fetcher-agent`

## Purpose

Upgrade the `web_fetcher` agent's `search_web` tool from a browsing-LLM wrapper to a **real server-side web search** (Brave Search API by default → fetch top pages → convert to markdown), and add a **vector-backed semantic cache** (search phrase → compacted report) with TTL + content-hash hybrid invalidation, a UI view, and source tracking.

## Scope decisions (locked in brainstorm)

| Decision | Choice |
|---|---|
| Search backend | Brave Search API (requires `web_fetcher.brave_api_key` or `MCP_ADJUTANT_BRAVE_API_KEY`; DuckDuckGo HTML scrape available for tests via `MCP_ADJUTANT_DDG_HTML_URL`) |
| DB location | Same `.adjutant/cache.db`, new tables via `MIGRATIONS` |
| Cache shape | Phrase → report (mirrors scout's query → insight) |
| Invalidation | TTL + content-hash hybrid (fast path within TTL, re-fetch + hash-check past TTL) |
| Embedding model | Reuse existing `LocalEmbeddingEngine` (bge-small-en-v1.5, 384-dim) |

### Out of scope

- robots.txt / rate-limit politeness layer (low volume; can add later)
- Two-layer results-cache (phrase→results + URL→page); we cache only phrase→report
- ANN index / sqlite-vec (brute-force scan, same as scout — fine at this scale)
- The browsing-capable model tier is **removed**: `search_web` now scrapes directly. `WebFetcherProfile.browsing` becomes unused and will be dropped.

## Architecture

### `search_web` becomes a real two-step scrape

```
search_web(query)
   │
   ├─► Brave Search API (ureq GET https://api.search.brave.com/res/v1/web/search, X-Subscription-Token)
   │       → parse top-N result links + titles + snippets
   │       (tests/fallback: DuckDuckGo HTML scrape when MCP_ADJUTANT_DDG_HTML_URL is set)
   │
   └─► for each top URL: HTTP GET (ureq, manual redirect validation + DNS pinning) → HTML→markdown (htmd) → truncate
                                                       │
   returns: assembled grounded markdown (sources + content) ▼
                                            appended to accumulated_data
reasoning model calls search_web (multi-hop ok), then finalize(report)
                                                       │
                              report + source URLs ───┘
                                                       ▼
                                    on finalize: store in cache
   next time same phrase: cosine match → TTL check → content-hash re-fetch
```

The reasoning model still drives the `AutonomousAgent` loop. `search_web` no longer delegates to a browsing LLM — it scrapes itself. **Any OpenAI-compatible model works for the reasoning tier** (no special browsing model needed).

### New crate

`htmd` — lightweight HTML-to-markdown converter. Added to `Cargo.toml`. HTTP reuses `ureq` (existing dep).

## Cache schema (new tables in `MIGRATIONS`, same `cache.db`)

Mirrors scout's 4-table shape, adapted for web content + TTL:

```sql
CREATE TABLE IF NOT EXISTS web_queries (
    id TEXT PRIMARY KEY,              -- sha256(search_phrase), reuse hash_query_text
    raw_text TEXT NOT NULL,
    embedding BLOB                    -- 1536 bytes (384 * f32), nullable
);

CREATE TABLE IF NOT EXISTS web_reports (
    id TEXT PRIMARY KEY,              -- same id as web_queries (shared key, 1:1)
    content TEXT NOT NULL,            -- the agent's finalized markdown report
    created_at INTEGER NOT NULL       -- unix seconds
);

CREATE TABLE IF NOT EXISTS web_sources (
    id TEXT PRIMARY KEY,              -- sha256(url)
    url TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,     -- hash of fetched+converted markdown at store time
    fetched_at INTEGER NOT NULL       -- unix seconds (for TTL math)
);

CREATE TABLE IF NOT EXISTS web_fetch_dependencies (
    report_id TEXT,
    source_id TEXT,
    PRIMARY KEY (report_id, source_id),
    FOREIGN KEY(report_id) REFERENCES web_reports(id) ON DELETE CASCADE,
    FOREIGN KEY(source_id) REFERENCES web_sources(id) ON DELETE CASCADE
);
```

All four append to the existing `MIGRATIONS` array in `src/cache/project.rs` (after `agent_evaluations`). `PRAGMA foreign_keys = ON` is already set, so cascade deletes work.

## Invalidation: TTL + content-hash hybrid

On cache lookup (cosine match found at similarity ≥ `WEB_CACHE_THRESHOLD`):

```
for each web_source linked to the matched report:
    age = now - source.fetched_at
    if age < ttl_seconds (default 7 days):
        continue                      # TRUST WINDOW: fresh enough, skip re-fetch
    else:
        re-fetch the URL, convert to markdown, SHA-256 it
        if content hash != stored content_sha256:
            INVALIDATE (delete report + query + deps via cascade)
            return MISS               # content changed under us
return HIT                            # all sources valid (within TTL or hash-unchanged)
```

- **Fast path** (recent reports): zero HTTP fetches — pure timestamp check.
- **Slow path** (stale): re-fetches only the *expired* sources, not all of them.
- A single changed source invalidates the whole report (conservative: a report is only as fresh as its stalest source).
- If a source URL is now unreachable (404/timeout), treat as dirty → invalidate.

## Cache manager API (new methods on `ProjectCacheManager`)

```rust
/// Returns a cached report when a semantically similar search phrase exists
/// and every linked web source is still valid (TTL + content-hash).
pub fn try_get_valid_web_report(&self, search_phrase: &str) -> Result<Option<String>, String>

/// Stores a web report, snapshots each source URL (url + content_sha256 + fetched_at),
/// and links them as dependencies.
pub fn store_web_report(
    &mut self,
    search_phrase: &str,
    report_content: &str,
    sources: Vec<WebSourceSnapshot>,
) -> Result<(), String>
```

`WebSourceSnapshot { url, content_sha256, fetched_at }` is produced by `search_web` during the scrape (it already has the fetched markdown + URL when it converts pages).

## Source tracking

The `search_web` tool collects every URL it fetches into a `Mutex<Vec<WebSourceSnapshot>>` held by `WebFetcherAgent`. The agent reads this list after the loop completes (at store time) and passes it to `store_web_report` — exactly how scout's `record_touched_file` populates `context.touched_files`.

Because `LlmTool::invoke` takes `&self`, the tool cannot mutate the agent directly; it writes through the shared `Mutex`. The handler reads the mutex after `AgentLoopOrchestrator::run` returns.

## Cache flow (mirrors `run_scout_with_cache`)

A new `run_web_fetch_with_cache` in `src/agent/web_fetcher/cache_flow.rs`:

1. `try_get_valid_web_report(phrase)` → if `Some(report)`, return `WebCacheOutcome::Hit(report)` (agent never runs).
2. Else run `AgentLoopOrchestrator::run`. If `result.agent_completed`, collect sources from the agent, call `store_web_report(phrase, report, sources)`.
3. Return `WebCacheOutcome::Fresh(report)`.

`handle_web_fetch` calls this instead of running the orchestrator directly.

## Config

Extend `WebFetcherProfile` (drop `browsing`, add cache tunables):

```rust
pub struct WebFetcherProfile {
    pub max_search_hops: u32,     // default 3
    pub token_budget: u32,        // default 8000
    pub cache_ttl_seconds: u64,   // default 604_800 (7 days)
    pub web_cache_threshold: f32, // default 0.78 (web paraphrases cluster lower than code)
}
```

`browsing: PhaseProfile` is removed — `search_web` scrapes directly, no second model.

## HTTP API + frontend

### `/api/cache` snapshot

`CacheSnapshot` gains:

```rust
pub web_queries: Vec<WebQueryRow>,           // { id, raw_text, has_embedding }
pub web_reports: Vec<WebReportRow>,           // { id, query_text, content, created_at }
pub web_sources: Vec<WebSourceRow>,           // { id, url, content_sha256, fetched_at, is_stale }
pub web_dependencies: Vec<WebFetchDependencyRow>, // { report_id, source_id }
```

`CacheOverview` gains: `web_query_count`, `web_report_count`, `web_source_count`, `web_dependency_count`.

`WebSourceRow.is_stale` is computed at snapshot time (`now - fetched_at > ttl`) — mirrors how `CodeNodeRow.is_dirty` is computed live.

### Frontend

New `WebCacheView.tsx` (parallel to `ScoutCacheView.tsx`):
- Stats row: web queries / reports / sources / dependencies counts.
- Web queries table (query / embedding).
- Web reports list (expandable cards, same pattern as insights).
- Web sources table (URL / fetched_at / stale chip — green=fresh, red=stale).
- Dependencies table.

Routed via `#/web-cache` in `NavBar.tsx`, with a quick-link from `ConfigApp.tsx`.

## Files to touch (summary)

**New:**
- `src/cache/web_source.rs` — `WebSourceSnapshot`, `WebSourceRow`, hash/fetch helpers.
- `src/agent/web_fetcher/cache_flow.rs` — `run_web_fetch_with_cache`, `WebCacheOutcome`.
- `frontend/src/modules/config-ui/WebCacheView.tsx`.

**Modify:**
- `Cargo.toml` — add `htmd` crate.
- `src/cache/project.rs` — 4 new migrations.
- `src/cache/manager.rs` — `try_get_valid_web_report`, `store_web_report`, web semantic match + invalidation.
- `src/cache/inspect.rs` — web snapshot rows + overview fields.
- `src/cache/mod.rs` — re-exports.
- `src/domain.rs` — `WebFetcherProfile` field changes (drop `browsing`, add cache tunables).
- `src/agent/web_fetcher.rs` — rewrite `SearchWebTool` to scrape DDG + track sources; drop `BC` generic entirely; add source mutex.
- `src/agent/web_fetcher/tools.rs` — (optional split if search_web grows large).
- `src/mcp/handlers.rs` — `handle_web_fetch` uses cache flow.
- `src/config_server.rs` — snapshot already extended via inspect.rs (no route changes).
- `frontend/src/modules/config-ui/types.ts` — web cache row types.
- `frontend/src/modules/config-ui/ConfigApp.tsx` + `NavBar.tsx` — web-cache route/link.

## Testing

- `search_web` scrape logic: unit test with a fixture HTML file (no network) — parse DDG HTML, assert URLs extracted.
- HTML→markdown: unit test on fixture HTML.
- Cache store/lookup: integration test using the existing `ScriptClient` mock for the reasoning tier + fixture source snapshots (no network).
- Invalidation: unit test TTL fast-path + content-hash slow-path with fake timestamps.
- Full existing suite continues to pass.

## Success criteria

- `web_fetch` with a search phrase returns a compacted markdown report grounded in real scraped web content.
- Repeat of a semantically similar phrase (within TTL) returns the cached report instantly (zero HTTP fetches).
- Past TTL, changed source content invalidates the cache; unchanged content preserves it.
- UI shows web queries/reports/sources with live staleness chips.
- All existing tests pass; new tests cover scrape + cache offline.
