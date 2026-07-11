# Web Fetcher: Real Search + Semantic Cache — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `web_fetcher`'s `search_web` tool to scrape DuckDuckGo + fetch real pages (no API key, no browsing LLM), and add a vector-backed phrase→report semantic cache with TTL + content-hash hybrid invalidation, plus a UI view.

**Architecture:** `search_web` scrapes DDG HTML server-side via `ureq`, fetches top result pages, converts HTML→markdown via `htmd`, and tracks every fetched URL + content hash. On cache lookup, cosine similarity matches the search phrase (reuse the existing `LocalEmbeddingEngine`); within TTL, serve immediately; past TTL, re-fetch expired sources and compare content hashes. New tables in the shared `.adjutant/cache.db`.

**Tech Stack:** Rust, `ureq` (existing), `htmd` (new), `rusqlite` (existing), `sha2` (existing), `bytemuck` (existing). React/TS frontend.

**Spec:** `docs/superpowers/specs/2026-07-11-web-fetcher-search-and-cache-design.md`

---

## File Structure

**Create:**
- `src/tools/web_fetch.rs` — DDG scrape + page fetch + HTML→markdown + result parsing. Pure functions, no agent coupling.
- `src/agent/web_fetcher/cache_flow.rs` — `run_web_fetch_with_cache` + `WebCacheOutcome` (mirrors `scout/cache_flow.rs`).
- `tests/web_fetcher_cache_tests.rs` — cache store/lookup/invalidation tests (offline).
- `tests/fixtures/web_fetcher/ddg_results.html` — fixture DDG HTML for scrape tests.
- `frontend/src/modules/config-ui/WebCacheView.tsx` — web cache UI view.

**Modify:**
- `Cargo.toml` — add `htmd`.
- `src/cache/project.rs` — 4 new table migrations.
- `src/cache/manager.rs` — `try_get_valid_web_report`, `store_web_report`, web semantic match, web invalidation.
- `src/cache/inspect.rs` — web snapshot rows + overview fields.
- `src/cache/mod.rs` — re-exports.
- `src/domain.rs` — `WebFetcherProfile` changes (drop `browsing`, add `cache_ttl_seconds`, `web_cache_threshold`).
- `src/tools/mod.rs` — re-export web_fetch module.
- `src/agent/web_fetcher.rs` — rewrite `SearchWebTool` to scrape; add source mutex; drop `BC` generic + `browsing_client` param.
- `src/agent/mod.rs` — re-export cache_flow.
- `src/mcp/handlers.rs` — `handle_web_fetch` uses cache flow.
- `frontend/src/modules/config-ui/types.ts` — web cache row types + overview fields.
- `frontend/src/modules/config-ui/ConfigApp.tsx` — drop browsing panel; add web-cache link.
- `frontend/src/modules/config-ui/NavBar.tsx` — add `#/web-cache` link.
- `frontend/src/modules/config-ui/index.ts` — export `WebCacheView`.

---

## Task 1: Add `htmd` crate + DDG scrape module

**Files:**
- Modify: `Cargo.toml`
- Create: `src/tools/web_fetch.rs`
- Modify: `src/tools/mod.rs`

- [ ] **Step 1: Add the `htmd` dependency**

Add to `Cargo.toml` under `[dependencies]` (alphabetical, after `home`):

```toml
htmd = "0.1"
```

- [ ] **Step 2: Create the scrape module with a failing test**

Create `tests/fixtures/web_fetcher/ddg_results.html` with a minimal DDG-lite result page:

```html
<html><body>
<div class="result">
  <h2 class="result__title"><a href="https://example.com/tokio/docs" class="result__a">Tokio Async Runtime</a></h2>
  <a class="result__snippet" href="https://example.com/tokio/docs">The official Tokio async runtime documentation covering spawn, channels, and timers.</a>
</div>
<div class="result">
  <h2 class="result__title"><a href="https://example.com/rust-async/book" class="result__a">Asynchronous Programming in Rust</a></h2>
  <a class="result__snippet" href="https://example.com/rust-async/book">A guide to async Rust including executors and futures.</a>
</div>
</body></html>
```

Create `src/tools/web_fetch.rs`:

```rust
use sha2::{Digest, Sha256};

/// A single search result extracted from DuckDuckGo's HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

/// A fetched source page ready for caching.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub url: String,
    pub markdown: String,
    pub content_sha256: String,
}

/// Parse DuckDuckGo lite-HTML result links from raw HTML.
/// Extracts result URLs, titles, and snippets from `<div class="result">` blocks.
pub fn parse_ddg_results(html: &str) -> Vec<SearchResult> {
    html.split(r#"<div class="result">"#)
        .skip(1)
        .filter_map(|block| {
            let url = extract_href(block)?;
            let title = extract_text_after(block, "result__a").unwrap_or_default();
            let snippet = extract_text_after(block, "result__snippet").unwrap_or_default();
            if url.is_empty() {
                None
            } else {
                Some(SearchResult {
                    url,
                    title: unescape_html(&title),
                    snippet: unescape_html(&snippet),
                })
            }
        })
        .collect()
}

/// Convert raw HTML to markdown using htmd.
pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html)
}

/// Fetch a URL and return its content as markdown + SHA-256 hash.
pub fn fetch_page_as_markdown(url: &str) -> Result<FetchedPage, String> {
    let agent = ureq::AgentBuilder::new().build();
    let response = agent
        .get(url)
        .set("User-Agent", "mcp-adjutant/1.0 (web fetcher)")
        .call()
        .map_err(|err| format!("failed to fetch {url}: {err}"))?;

    let html: String = response
        .into_string()
        .map_err(|err| format!("failed to read body from {url}: {err}"))?;

    let markdown = html_to_markdown(&html);
    let content_sha256 = hash_content(&markdown);

    Ok(FetchedPage {
        url: url.to_string(),
        markdown,
        content_sha256,
    })
}

/// Scrape DuckDuckGo for a query, fetch the top-N result pages, return
/// assembled grounded markdown + the list of fetched sources.
pub fn search_and_fetch(query: &str, max_pages: usize) -> Result<(String, Vec<FetchedPage>), String> {
    let encoded = url_encode(query);
    let ddg_url = format!("https://html.duckduckgo.com/html/?q={encoded}");

    let agent = ureq::AgentBuilder::new().build();
    let response = agent
        .get(&ddg_url)
        .set("User-Agent", "mcp-adjutant/1.0 (web fetcher)")
        .call()
        .map_err(|err| format!("DuckDuckGo request failed: {err}"))?;

    let html: String = response
        .into_string()
        .map_err(|err| format!("failed to read DDG response: {err}"))?;

    let results = parse_ddg_results(&html);
    let top = results.into_iter().take(max_pages);

    let mut pages = Vec::new();
    let mut sections = Vec::new();

    for result in top {
        match fetch_page_as_markdown(&result.url) {
            Ok(page) => {
                sections.push(format!(
                    "## [{}]({})\n\n{}\n",
                    result.title, result.url, truncate_markdown(&page.markdown, 4_000)
                ));
                pages.push(page);
            }
            Err(err) => {
                sections.push(format!(
                    "## [{}]({})\n\n*(could not fetch: {err})*\n",
                    result.title, result.url
                ));
            }
        }
    }

    if sections.is_empty() {
        return Err(format!("no results found for query: {query}"));
    }

    let markdown = format!(
        "# Search results for: {query}\n\n{}",
        sections.join("\n---\n\n")
    );

    Ok((markdown, pages))
}

fn hash_content(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn truncate_markdown(markdown: &str, max_chars: usize) -> String {
    if markdown.chars().count() <= max_chars {
        return markdown.to_string();
    }
    let kept: String = markdown.chars().take(max_chars).collect();
    format!("{kept}…")
}

fn url_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => '+'.to_string(),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' => {
                c.to_string()
            }
            c => format!("%{:02X}", c as u8),
        })
        .collect()
}

fn extract_href(block: &str) -> Option<String> {
    let link_start = block.find(r#"class="result__a""#)?;
    let href_start = block[..link_start].rfind("href=\"")?;
    let after_href = &block[href_start + 6..];
    let end = after_href.find('"')?;
    let raw = &after_href[..end];
    // DDG sometimes wraps URLs in a redirect prefix; strip it.
    let cleaned = raw
        .strip_prefix("https://duckduckgo.com/l/?uddg=")
        .and_then(|s| s.split('&').next())
        .unwrap_or(raw);
    Some(url_decode(cleaned))
}

fn extract_text_after(block: &str, class: &str) -> Option<String> {
    let marker = format!(r#"class="{class}""#);
    let pos = block.find(&marker)?;
    let after_tag = block[pos..].find('>')?;
    let after_close = &block[pos + after_tag + 1..];
    let end = after_close.find('<').unwrap_or(after_close.len());
    Some(after_close[..end].trim().to_string())
}

fn unescape_html(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            result.push(' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                result.push(byte as char);
                i += 3;
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ddg_results_extracts_urls_and_titles() {
        let html = include_str!("../../tests/fixtures/web_fetcher/ddg_results.html");
        let results = parse_ddg_results(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://example.com/tokio/docs");
        assert_eq!(results[0].title, "Tokio Async Runtime");
        assert!(results[0].snippet.contains("official Tokio"));
    }

    #[test]
    fn html_to_markdown_converts_basic_html() {
        let html = "<h1>Title</h1><p>Hello <strong>world</strong>.</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Hello"));
    }

    #[test]
    fn hash_content_is_deterministic() {
        assert_eq!(hash_content("test"), hash_content("test"));
        assert_ne!(hash_content("test"), hash_content("other"));
    }

    #[test]
    fn truncate_markdown_preserves_short_input() {
        assert_eq!(truncate_markdown("short", 100), "short");
    }

    #[test]
    fn truncate_markdown_cuts_long_input() {
        let long = "x".repeat(200);
        let out = truncate_markdown(&long, 50);
        assert!(out.chars().count() <= 51);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn url_encode_replaces_spaces() {
        assert_eq!(url_encode("rust async"), "rust+async");
        assert_eq!(url_encode("a&b"), "a%26B");
    }
}
```

- [ ] **Step 3: Declare the module**

Add to `src/tools/mod.rs` (find the existing module declarations, add):

```rust
pub mod web_fetch;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib tools::web_fetch:: -- --nocapture`
Expected: PASS — all 6 unit tests (parse, html_to_markdown, hash, truncate ×2, url_encode).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/tools/web_fetch.rs src/tools/mod.rs tests/fixtures/web_fetcher/ddg_results.html
git commit -m "feat(tools): add DDG scrape + HTML-to-markdown web fetch module"
```

---

## Task 2: Add cache migrations (4 new tables)

**Files:**
- Modify: `src/cache/project.rs:12-45` (MIGRATIONS array)

- [ ] **Step 1: Add the 4 table migrations**

In `src/cache/project.rs`, append to the `MIGRATIONS` array (after the `agent_evaluations` entry, before the closing `];`):

```rust
    "CREATE TABLE IF NOT EXISTS web_queries (
        id TEXT PRIMARY KEY,
        raw_text TEXT NOT NULL,
        embedding BLOB
    );",
    "CREATE TABLE IF NOT EXISTS web_reports (
        id TEXT PRIMARY KEY,
        content TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS web_sources (
        id TEXT PRIMARY KEY,
        url TEXT NOT NULL,
        content_sha256 TEXT NOT NULL,
        fetched_at INTEGER NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS web_fetch_dependencies (
        report_id TEXT,
        source_id TEXT,
        PRIMARY KEY (report_id, source_id),
        FOREIGN KEY(report_id) REFERENCES web_reports(id) ON DELETE CASCADE,
        FOREIGN KEY(source_id) REFERENCES web_sources(id) ON DELETE CASCADE
    );",
```

- [ ] **Step 2: Verify migrations run**

Run: `cargo build --lib && cargo test --lib cache:: -- --nocapture`
Expected: BUILD SUCCEEDS, existing cache tests still pass.

- [ ] **Step 3: Commit**

```bash
git add src/cache/project.rs
git commit -m "feat(cache): add web_queries/reports/sources/dependencies migrations"
```

---

## Task 3: Add cache manager methods for web report store + lookup

**Files:**
- Modify: `src/cache/manager.rs`

- [ ] **Step 1: Add `WebSourceSnapshot` struct + threshold constant**

At the top of `src/cache/manager.rs`, after the existing `SEMANTIC_SIMILARITY_THRESHOLD` (line 14), add:

```rust
/// Minimum cosine similarity for a web cache hit.
/// Web-search paraphrases cluster lower than code-research paraphrases.
pub const WEB_CACHE_THRESHOLD: f32 = 0.78;

/// A fetched web source snapshot, produced by the scrape tool, stored as a dependency.
#[derive(Debug, Clone)]
pub struct WebSourceSnapshot {
    pub url: String,
    pub content_sha256: String,
    pub fetched_at: i64,
}
```

- [ ] **Step 2: Add `store_web_report` method**

Inside the `impl ProjectCacheManager` block, after `store_insight` (after line 143), add:

```rust
    /// Stores a web report: embeds the search phrase, inserts the report,
    /// snapshots each source URL, and links them as dependencies.
    pub fn store_web_report(
        &mut self,
        search_phrase: &str,
        report_content: &str,
        sources: Vec<WebSourceSnapshot>,
    ) -> Result<(), String> {
        let query_id = hash_query_text(search_phrase);
        let created_at = current_unix_timestamp()?;
        let embedding = self.embedding_engine.generate(search_phrase)?;
        let embedding_blob = bytemuck::cast_slice::<f32, u8>(&embedding).to_vec();

        let transaction = self
            .conn
            .transaction()
            .map_err(|err| format!("failed to start web cache transaction: {err}"))?;

        transaction
            .execute(
                "INSERT INTO web_queries (id, raw_text, embedding) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET raw_text = excluded.raw_text, embedding = excluded.embedding",
                params![query_id, search_phrase, embedding_blob],
            )
            .map_err(|err| format!("failed to store web query: {err}"))?;

        transaction
            .execute(
                "INSERT INTO web_reports (id, content, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET content = excluded.content, created_at = excluded.created_at",
                params![query_id, report_content, created_at],
            )
            .map_err(|err| format!("failed to store web report: {err}"))?;

        transaction
            .execute(
                "DELETE FROM web_fetch_dependencies WHERE report_id = ?1",
                params![query_id],
            )
            .map_err(|err| format!("failed to clear old web dependencies: {err}"))?;

        for source in sources {
            let source_id = hash_query_text(&source.url);

            transaction
                .execute(
                    "INSERT INTO web_sources (id, url, content_sha256, fetched_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET
                         url = excluded.url,
                         content_sha256 = excluded.content_sha256,
                         fetched_at = excluded.fetched_at",
                    params![source_id, source.url, source.content_sha256, source.fetched_at],
                )
                .map_err(|err| format!("failed to store web source {}: {err}", source.url))?;

            transaction
                .execute(
                    "INSERT OR IGNORE INTO web_fetch_dependencies (report_id, source_id)
                     VALUES (?1, ?2)",
                    params![query_id, source_id],
                )
                .map_err(|err| format!("failed to link web dependency: {err}"))?;
        }

        transaction
            .commit()
            .map_err(|err| format!("failed to commit web cache transaction: {err}"))?;

        Ok(())
    }
```

- [ ] **Step 3: Add `try_get_valid_web_report` method**

After `store_web_report`, add:

```rust
    /// Returns a cached web report when a semantically similar search phrase
    /// exists and every linked source is valid (within TTL or content unchanged).
    /// `ttl_seconds` is the trust window; sources older than TTL are re-fetched
    /// and their content hash compared. One changed source invalidates the report.
    pub fn try_get_valid_web_report(
        &self,
        search_phrase: &str,
        ttl_seconds: i64,
    ) -> Result<Option<String>, String> {
        let query_embedding = self.embedding_engine.generate(search_phrase)?;
        let matched_query_id = match self.find_web_semantic_match(&query_embedding)? {
            Some(query_id) => query_id,
            None => return Ok(None),
        };

        let report_content = match self.conn.query_row(
            "SELECT content FROM web_reports WHERE id = ?1",
            params![matched_query_id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(content) => content,
            Err(RusqliteError::QueryReturnedNoRows) => return Ok(None),
            Err(err) => {
                return Err(format!("failed to load cached web report: {err}"));
            }
        };

        let sources = self.load_web_source_urls(&matched_query_id)?;
        let now = current_unix_timestamp()?;

        for (source_id, url, stored_hash, fetched_at) in sources {
            let age = now - fetched_at;
            if age < ttl_seconds {
                continue; // trust window: fresh enough
            }
            // Past TTL: re-fetch and compare content hash.
            match crate::tools::web_fetch::fetch_page_as_markdown(&url) {
                Ok(page) if page.content_sha256 == stored_hash => continue, // unchanged
                _ => {
                    // Changed or unreachable: invalidate.
                    self.invalidate_web_report(&matched_query_id)?;
                    return Ok(None);
                }
            }
        }

        Ok(Some(report_content))
    }
```

- [ ] **Step 4: Add private helpers**

After `invalidate_insight` (before `decode_embedding_blob`), add:

```rust
    fn find_web_semantic_match(&self, query_embedding: &[f32]) -> Result<Option<String>, String> {
        let mut statement = self
            .conn
            .prepare("SELECT id, embedding FROM web_queries WHERE embedding IS NOT NULL")
            .map_err(|err| format!("failed to prepare web semantic lookup: {err}"))?;

        let rows = statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })
            .map_err(|err| format!("failed to query web embeddings: {err}"))?;

        let mut best_match: Option<(String, f32)> = None;
        for row in rows {
            let (query_id, blob) =
                row.map_err(|err| format!("failed to read web embedding row: {err}"))?;
            let Some(stored) = decode_embedding_blob(&blob) else {
                continue;
            };
            let similarity = LocalEmbeddingEngine::dot_product(query_embedding, stored);
            if similarity >= WEB_CACHE_THRESHOLD
                && best_match
                    .as_ref()
                    .is_none_or(|(_, best)| similarity > *best)
            {
                best_match = Some((query_id, similarity));
            }
        }
        Ok(best_match.map(|(id, _)| id))
    }

    fn load_web_source_urls(
        &self,
        report_id: &str,
    ) -> Result<Vec<(String, String, String, i64)>, String> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT s.id, s.url, s.content_sha256, s.fetched_at
                 FROM web_sources s
                 INNER JOIN web_fetch_dependencies dep ON dep.source_id = s.id
                 WHERE dep.report_id = ?1",
            )
            .map_err(|err| format!("failed to prepare web source lookup: {err}"))?;

        let rows = statement
            .query_map(params![report_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|err| format!("failed to query web sources: {err}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to read web source row: {err}"))
    }

    fn invalidate_web_report(&self, report_id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM web_reports WHERE id = ?1", params![report_id])
            .map_err(|err| format!("failed to delete stale web report: {err}"))?;
        self.conn
            .execute("DELETE FROM web_queries WHERE id = ?1", params![report_id])
            .map_err(|err| format!("failed to delete stale web query: {err}"))?;
        Ok(())
    }
```

- [ ] **Step 5: Build and verify**

Run: `cargo build --lib`
Expected: BUILD SUCCEEDS.

- [ ] **Step 6: Commit**

```bash
git add src/cache/manager.rs
git commit -m "feat(cache): add web report store + semantic lookup with TTL+hash invalidation"
```

---

## Task 4: Update `WebFetcherProfile` (drop browsing, add cache tunables)

**Files:**
- Modify: `src/domain.rs`

- [ ] **Step 1: Update the struct + Default**

Replace the `WebFetcherProfile` struct and its `Default` impl (added in the earlier branch) with:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebFetcherProfile {
    pub max_search_hops: u32,
    pub token_budget: u32,
    pub cache_ttl_seconds: u64,
    pub web_cache_threshold: f32,
}

impl Default for WebFetcherProfile {
    fn default() -> Self {
        Self {
            max_search_hops: 3,
            token_budget: 8_000,
            cache_ttl_seconds: 604_800, // 7 days
            web_cache_threshold: 0.78,
        }
    }
}
```

- [ ] **Step 2: Update the domain tests**

In the `tests` module, update `default_config_has_web_fetcher_phase_and_profile` to assert the new fields instead of `browsing`:

```rust
        let profile = config
            .web_fetcher
            .as_ref()
            .expect("default config should include a WebFetcherProfile");
        assert_eq!(profile.max_search_hops, 3);
        assert_eq!(profile.token_budget, 8_000);
        assert_eq!(profile.cache_ttl_seconds, 604_800);
        assert!((profile.web_cache_threshold - 0.78).abs() < f32::EPSILON);
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib domain:: -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/domain.rs
git commit -m "feat(domain): drop WebFetcherProfile.browsing, add cache_ttl_seconds + web_cache_threshold"
```

---

## Task 5: Rewrite `SearchWebTool` to scrape + track sources

**Files:**
- Modify: `src/agent/web_fetcher.rs`
- Modify: `tests/web_fetcher_tests.rs`

The `search_web` tool currently delegates to a browsing LLM. We replace it with a real DDG scraper that fetches pages and tracks sources. The `&self` constraint on `LlmTool::invoke` means the tool cannot borrow the agent\'s state directly, so source collection uses an `Arc<Mutex<Vec<FetchedPage>>>` shared between the agent and the tool at construction time (same pattern scout uses for `context.touched_files`, adapted for the tool layer).

- [ ] **Step 1: Replace `src/agent/web_fetcher.rs` with the scraper version**

Replace the entire file with:

```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use super::orchestrator::run_single_tool_turn;
use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::WebFetcherProfile;
use crate::llm::{required_str, LlmClient, LlmTool, LlmToolSet, ToolDefinition};
use crate::tools::web_fetch::{search_and_fetch, FetchedPage};

pub const WEB_FETCHER_SYSTEM_PROMPT: &str = r#"You are an autonomous web research agent (WEB_FETCHER). Your goal is to produce a compacted, accurate markdown document of the latest, authoritative web content for a given topic. The topic can be anything the user asks about; adapt your search approach to the kind of information it requires.

Available tools (call exactly one per turn):
- search_web(query, focus?) — scrape DuckDuckGo for the query, fetch the top result pages, and return grounded markdown with inline source links. Use `focus` to narrow the search. Non-terminal: results are added to your observation history.
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
    /// Build the agent with a reasoning client that drives the loop.
    /// `search_web` scrapes the web directly (no browsing model needed).
    pub fn new(reasoning_client: RC, profile: WebFetcherProfile) -> Self {
        let token_budget = profile.token_budget;
        let source_collector = Arc::new(Mutex::new(Vec::new()));
        let tools = web_fetcher_tool_set(token_budget, Arc::clone(&source_collector));
        Self {
            reasoning_client,
            tools,
            source_collector,
        }
    }

    /// Drain the collected source pages (called by the cache flow after the loop).
    pub fn take_sources(&self) -> Vec<FetchedPage> {
        self.source_collector
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default()
    }
}

#[async_trait]
impl<RC: LlmClient> AutonomousAgent for WebFetcherAgent<RC> {
    fn name(&self) -> &\'static str {
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

fn web_fetcher_tool_set(
    token_budget: u32,
    source_collector: Arc<Mutex<Vec<FetchedPage>>>,
) -> LlmToolSet {
    LlmToolSet::new()
        .register(SearchWebTool::new(source_collector))
        .register(FinalizeWebTool::with_budget(token_budget))
}

/// `search_web(query, focus?)`: scrapes DDG + fetches top pages.
/// Appends fetched pages to the shared source collector for caching.
struct SearchWebTool {
    source_collector: Arc<Mutex<Vec<FetchedPage>>>,
    definition: ToolDefinition,
}

impl SearchWebTool {
    fn new(source_collector: Arc<Mutex<Vec<FetchedPage>>>) -> Self {
        Self {
            source_collector,
            definition: ToolDefinition::new(
                "search_web",
                "Search the live web via DuckDuckGo and return grounded, cited markdown.",
            )
            .string_param("query", "Web search query.", true)
            .string_param(
                "focus",
                "Optional focus to narrow results (e.g. \'official source\', \'recent news\').",
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

        let (markdown, pages) = search_and_fetch(&full_query, MAX_PAGES_PER_SEARCH)?;

        if let Ok(mut guard) = self.source_collector.lock() {
            guard.extend(pages);
        }

        Ok(markdown)
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
        let tools = web_fetcher_tool_set(8_000, collector);
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

- [ ] **Step 2: Update the integration tests (signature changed)**

Replace `tests/web_fetcher_tests.rs` with:

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

fn profile_with_budget(token_budget: u32) -> WebFetcherProfile {
    WebFetcherProfile {
        token_budget,
        ..Default::default()
    }
}

#[tokio::test]
async fn web_fetcher_finalizes_report() {
    let reasoning = ReasoningScript::new(vec![LlmModelTurn {
        content: Some("Report ready.".to_string()),
        tool_calls: vec![LlmToolCall {
            name: "finalize".to_string(),
            arguments: serde_json::json!({
                "report": "## Tokio async runtime\n- spawn tasks\n- channels"
            }),
        }],
    }]);

    let agent = WebFetcherAgent::new(reasoning, profile_with_budget(8_000));
    let result = AgentLoopOrchestrator::run(&agent, "latest tokio docs".to_string(), 5)
        .await
        .expect("web fetcher loop should complete");

    assert!(result.is_finished);
    assert!(result.agent_completed);
    assert!(result.accumulated_data.contains("Tokio async runtime"));
}

#[tokio::test]
async fn web_fetcher_truncates_overlong_report_to_budget() {
    let long_body = "x".repeat(5_000);
    let reasoning = ReasoningScript::new(vec![LlmModelTurn {
        content: Some("Report ready.".to_string()),
        tool_calls: vec![LlmToolCall {
            name: "finalize".to_string(),
            arguments: serde_json::json!({ "report": long_body }),
        }],
    }]);

    let agent = WebFetcherAgent::new(reasoning, profile_with_budget(1_000));
    let result = AgentLoopOrchestrator::run(&agent, "topic".to_string(), 5)
        .await
        .expect("loop should complete");

    assert!(result.is_finished);
    assert!(result.accumulated_data.chars().count() < 4_500);
    assert!(result.accumulated_data.contains("[truncated"));
}
```

These tests only exercise `finalize` (no live network). `search_web` is tested via the fixture HTML in Task 1.

- [ ] **Step 3: Build and run tests**

Run: `cargo test --test web_fetcher_tests && cargo test --lib web_fetcher::`
Expected: PASS (2 integration + 3 unit tests).

- [ ] **Step 4: Commit**

```bash
git add src/agent/web_fetcher.rs tests/web_fetcher_tests.rs
git commit -m "feat(agent): rewrite SearchWebTool to scrape DDG, track sources via Arc<Mutex>"
```

---

## Task 6: Add cache flow + wire handler

**Files:**
- Create: `src/agent/web_fetcher/cache_flow.rs`
- Modify: `src/agent/web_fetcher.rs` (add `mod cache_flow`)
- Modify: `src/cache/mod.rs` (re-export `WebSourceSnapshot`)
- Modify: `src/mcp/handlers.rs`

- [ ] **Step 1: Create the cache flow module**

Create `src/agent/web_fetcher/cache_flow.rs`:

```rust
use std::sync::{Arc, Mutex};

use crate::cache::{ProjectCacheManager, WebSourceSnapshot};
use crate::tools::web_fetch::FetchedPage;

use super::WebFetcherAgent;

pub enum WebCacheOutcome {
    Hit(String),
    Fresh(String),
}

/// Run the web fetcher with a semantic cache.
/// 1. Check cache for a semantically similar phrase (TTL + content-hash validated).
/// 2. If miss, run the agent, then store the report + sources.
pub async fn run_web_fetch_with_cache<RC: crate::llm::LlmClient>(
    cache: &Mutex<ProjectCacheManager>,
    agent: &WebFetcherAgent<RC>,
    search_phrase: &str,
    max_iterations: u32,
    ttl_seconds: i64,
    use_cache: bool,
) -> Result<WebCacheOutcome, String> {
    if use_cache {
        let report = {
            let cache = cache
                .lock()
                .map_err(|_| "cache manager lock poisoned".to_string())?;
            cache.try_get_valid_web_report(search_phrase, ttl_seconds)?
        };
        if let Some(cached) = report {
            return Ok(WebCacheOutcome::Hit(cached));
        }
    }

    let result = crate::agent::AgentLoopOrchestrator::run(
        agent,
        search_phrase.to_string(),
        max_iterations,
    )
    .await?;

    if use_cache && result.agent_completed {
        let sources = collect_source_snapshots(agent);
        let _ = {
            let mut cache = cache
                .lock()
                .map_err(|_| "cache manager lock poisoned".to_string())?;
            cache.store_web_report(search_phrase, &result.accumulated_data, sources)
        };
    }

    Ok(WebCacheOutcome::Fresh(result.accumulated_data))
}

fn collect_source_snapshots<RC: crate::llm::LlmClient>(
    agent: &WebFetcherAgent<RC>,
) -> Vec<WebSourceSnapshot> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    agent
        .take_sources()
        .into_iter()
        .map(|page: FetchedPage| WebSourceSnapshot {
            url: page.url,
            content_sha256: page.content_sha256,
            fetched_at: now,
        })
        .collect()
}
```

- [ ] **Step 2: Declare the module + re-export**

In `src/agent/web_fetcher.rs`, the module is a subdir. Change `src/agent/web_fetcher.rs` to declare the submodule. Add at the top of `src/agent/web_fetcher.rs` (after the imports, before the prompt const):

```rust
pub mod cache_flow;
```

Wait — `web_fetcher.rs` is a file, not a directory. To add a submodule, either convert to `web_fetcher/mod.rs` or use `web_fetcher/` directory with `web_fetcher.rs` renamed. The simplest: move the content. Actually in Rust 2018+, you can have `src/agent/web_fetcher.rs` (the module) and `src/agent/web_fetcher/cache_flow.rs` (submodule) simultaneously if `web_fetcher.rs` declares `mod cache_flow;`.

Create the directory: `mkdir -p src/agent/web_fetcher`

Then move `src/agent/web_fetcher.rs` to `src/agent/web_fetcher/mod.rs`:

```bash
git mv src/agent/web_fetcher.rs src/agent/web_fetcher/mod.rs
```

Then create `src/agent/web_fetcher/cache_flow.rs` (the file from Step 1).

Add `pub mod cache_flow;` at the top of `src/agent/web_fetcher/mod.rs` (after the imports), and `pub use cache_flow::{run_web_fetch_with_cache, WebCacheOutcome};` at the bottom.

In `src/agent/mod.rs`, update the re-export:

```rust
pub use web_fetcher::{run_web_fetch_with_cache, WebCacheOutcome, WebFetcherAgent, WEB_FETCHER_SYSTEM_PROMPT};
```

- [ ] **Step 3: Re-export `WebSourceSnapshot` from cache**

In `src/cache/mod.rs`, add to the `pub use manager::{...}` line:

```rust
pub use manager::{ProjectCacheManager, WebSourceSnapshot, SEMANTIC_SIMILARITY_THRESHOLD, WEB_CACHE_THRESHOLD};
```

- [ ] **Step 4: Update `handle_web_fetch` to use the cache flow**

In `src/mcp/handlers.rs`, replace the body of `handle_web_fetch`'s async closure to use `run_web_fetch_with_cache`. Replace the existing handler (the one added in the prior branch) with:

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
            let web_profile = config.web_fetcher.clone().unwrap_or_default();
            let cache_manager =
                Arc::new(Mutex::new(open_cache_manager_near(&mcp_workspace_root())?));
            let reasoning_client = create_web_fetcher_llm_client(&config)?;
            let max_hops = web_profile.max_search_hops;
            let ttl = web_profile.cache_ttl_seconds as i64;

            let agent = WebFetcherAgent::new(reasoning_client, web_profile);
            match run_web_fetch_with_cache(
                &cache_manager,
                &agent,
                &search_phrase,
                max_hops,
                ttl,
                true,
            )
            .await?
            {
                WebCacheOutcome::Hit(report) => Ok(format!("[CACHE HIT]\n{report}")),
                WebCacheOutcome::Fresh(report) => Ok(report),
            }
        },
    )
    .await
}
```

- [ ] **Step 5: Update imports in handlers.rs**

Add `run_web_fetch_with_cache`, `WebCacheOutcome` to the `use crate::agent::{...}` import, and remove `create_llm_client` if it's now unused (it was only for the browsing client). The import should be:

```rust
use crate::agent::{
    default_builder_agent, run_scout_with_cache, run_web_fetch_with_cache, AgentLoopOrchestrator,
    EvaluatorAgent, ScoutAgent, ScoutCacheOutcome, SystemBuildRunner, TriageAgent, WebCacheOutcome,
    WebFetcherAgent, TRIAGE_SYSTEM_PROMPT,
};
```

Remove `create_llm_client` from the `use crate::llm::{...}` import if it's no longer used elsewhere in handlers.

- [ ] **Step 6: Build and verify**

Run: `cargo build --bin mcp-adjutant`
Expected: BUILD SUCCEEDS.

- [ ] **Step 7: Commit**

```bash
git add src/agent/web_fetcher/ src/agent/mod.rs src/cache/mod.rs src/mcp/handlers.rs
git commit -m "feat(web-fetcher): add cache flow and wire handle_web_fetch to use semantic cache"
```

---

## Task 7: Extend inspect.rs for web cache snapshot

**Files:**
- Modify: `src/cache/inspect.rs`
- Modify: `src/cache/mod.rs` (re-exports)

- [ ] **Step 1: Add web row structs**

At the end of `src/cache/inspect.rs` (before the free functions, after `InsightDependencyRow`), add:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct WebQueryRow {
    pub id: String,
    pub raw_text: String,
    pub has_embedding: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebReportRow {
    pub id: String,
    pub query_text: Option<String>,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebSourceRow {
    pub id: String,
    pub url: String,
    pub content_sha256: String,
    pub fetched_at: i64,
    pub is_stale: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebFetchDependencyRow {
    pub report_id: String,
    pub source_id: String,
}
```

- [ ] **Step 2: Extend `CacheOverview`**

Add the four new count fields to `CacheOverview`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct CacheOverview {
    pub project_root: String,
    pub query_count: usize,
    pub insight_count: usize,
    pub code_node_count: usize,
    pub embedding_count: usize,
    pub dependency_count: usize,
    pub evaluation_count: usize,
    pub web_query_count: usize,
    pub web_report_count: usize,
    pub web_source_count: usize,
    pub web_dependency_count: usize,
}
```

- [ ] **Step 3: Extend `CacheSnapshot`**

Add the four new Vec fields:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct CacheSnapshot {
    pub overview: CacheOverview,
    pub queries: Vec<CachedQueryRow>,
    pub insights: Vec<CachedInsightRow>,
    pub code_nodes: Vec<CodeNodeRow>,
    pub dependencies: Vec<InsightDependencyRow>,
    pub web_queries: Vec<WebQueryRow>,
    pub web_reports: Vec<WebReportRow>,
    pub web_sources: Vec<WebSourceRow>,
    pub web_dependencies: Vec<WebFetchDependencyRow>,
}
```

- [ ] **Step 4: Add list functions + extend `load_cache_snapshot`**

Add the four list functions (before `load_cache_snapshot`):

```rust
fn list_web_queries(conn: &Connection) -> Result<Vec<WebQueryRow>, String> {
    let mut statement = conn
        .prepare("SELECT id, raw_text, embedding FROM web_queries ORDER BY id")
        .map_err(|err| format!("failed to prepare web_queries query: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            let embedding: Option<Vec<u8>> = row.get(2)?;
            Ok(WebQueryRow {
                id: row.get(0)?,
                raw_text: row.get(1)?,
                has_embedding: embedding.is_some_and(|blob| !blob.is_empty()),
            })
        })
        .map_err(|err| format!("failed to query web_queries: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_query row: {err}"))
}

fn list_web_reports(conn: &Connection) -> Result<Vec<WebReportRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT r.id, q.raw_text, r.content, r.created_at
             FROM web_reports r
             LEFT JOIN web_queries q ON q.id = r.id
             ORDER BY r.created_at DESC",
        )
        .map_err(|err| format!("failed to prepare web_reports query: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(WebReportRow {
                id: row.get(0)?,
                query_text: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|err| format!("failed to query web_reports: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_report row: {err}"))
}

fn list_web_sources(conn: &Connection, ttl_seconds: i64) -> Result<Vec<WebSourceRow>, String> {
    let now = current_unix_timestamp()?;
    let mut statement = conn
        .prepare("SELECT id, url, content_sha256, fetched_at FROM web_sources ORDER BY url")
        .map_err(|err| format!("failed to prepare web_sources query: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            let fetched_at: i64 = row.get(3)?;
            let is_stale = now - fetched_at > ttl_seconds;
            Ok(WebSourceRow {
                id: row.get(0)?,
                url: row.get(1)?,
                content_sha256: row.get(2)?,
                fetched_at,
                is_stale,
            })
        })
        .map_err(|err| format!("failed to query web_sources: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_source row: {err}"))
}

fn list_web_dependencies(conn: &Connection) -> Result<Vec<WebFetchDependencyRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT report_id, source_id FROM web_fetch_dependencies ORDER BY report_id, source_id",
        )
        .map_err(|err| format!("failed to prepare web_dependencies query: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(WebFetchDependencyRow {
                report_id: row.get(0)?,
                source_id: row.get(1)?,
            })
        })
        .map_err(|err| format!("failed to query web_dependencies: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read web_dependency row: {err}"))
}
```

You need `current_unix_timestamp` — it's already in `src/cache/project.rs`. Import it at the top of `inspect.rs`:

```rust
use super::project::current_unix_timestamp;
```

Update `load_cache_snapshot` to include the web rows. Change its signature to accept `ttl_seconds`:

```rust
pub fn load_cache_snapshot(
    conn: &Connection,
    project_root: &Path,
    ttl_seconds: i64,
) -> Result<CacheSnapshot, String> {
    let queries = list_queries(conn)?;
    let insights = list_insights(conn)?;
    let code_nodes = list_code_nodes(conn, project_root)?;
    let dependencies = list_dependencies(conn)?;
    let web_queries = list_web_queries(conn)?;
    let web_reports = list_web_reports(conn)?;
    let web_sources = list_web_sources(conn, ttl_seconds)?;
    let web_dependencies = list_web_dependencies(conn)?;

    let overview = CacheOverview {
        project_root: project_root.display().to_string(),
        query_count: queries.len(),
        insight_count: insights.len(),
        code_node_count: code_nodes.len(),
        embedding_count: queries.iter().filter(|q| q.has_embedding).count(),
        dependency_count: dependencies.len(),
        evaluation_count: count_rows(conn, "agent_evaluations")?,
        web_query_count: web_queries.len(),
        web_report_count: web_reports.len(),
        web_source_count: web_sources.len(),
        web_dependency_count: web_dependencies.len(),
    };

    Ok(CacheSnapshot {
        overview,
        queries,
        insights,
        code_nodes,
        dependencies,
        web_queries,
        web_reports,
        web_sources,
        web_dependencies,
    })
}
```

- [ ] **Step 5: Update the config server call site**

In `src/config_server.rs`, update `get_cache` to pass the TTL:

```rust
async fn get_cache(
    State(_state): State<ConfigServerState>,
) -> Result<Json<CacheSnapshot>, CacheApiError> {
    let (project_root, conn) = open_workspace_cache().map_err(CacheApiError::from)?;
    let snapshot = load_cache_snapshot(&conn, &project_root, 604_800)
        .map_err(CacheApiError::from)?;
    Ok(Json(snapshot))
}
```

- [ ] **Step 6: Update re-exports in cache/mod.rs**

```rust
pub use inspect::{
    list_evaluations, list_evaluations_page, load_cache_snapshot, AgentEvaluationRow,
    CacheSnapshot, EvaluationsPage, WebFetchDependencyRow, WebQueryRow, WebReportRow,
    WebSourceRow, EVALUATIONS_PAGE_SIZE,
};
```

- [ ] **Step 7: Build and verify**

Run: `cargo build --lib`
Expected: BUILD SUCCEEDS.

- [ ] **Step 8: Commit**

```bash
git add src/cache/inspect.rs src/cache/mod.rs src/config_server.rs
git commit -m "feat(cache): extend snapshot with web queries/reports/sources/dependencies"
```

---

## Task 8: Frontend — web cache view

**Files:**
- Modify: `frontend/src/modules/config-ui/types.ts`
- Create: `frontend/src/modules/config-ui/WebCacheView.tsx`
- Modify: `frontend/src/modules/config-ui/NavBar.tsx`
- Modify: `frontend/src/modules/config-ui/ConfigApp.tsx`
- Modify: `frontend/src/modules/config-ui/index.ts`

- [ ] **Step 1: Add web cache types**

In `frontend/src/modules/config-ui/types.ts`, extend `CacheOverview` and `CacheSnapshot`, and add the new row types:

```typescript
export interface CacheOverview {
  project_root: string
  query_count: number
  insight_count: number
  code_node_count: number
  embedding_count: number
  dependency_count: number
  evaluation_count: number
  web_query_count: number
  web_report_count: number
  web_source_count: number
  web_dependency_count: number
}

export interface WebQueryRow {
  id: string
  raw_text: string
  has_embedding: boolean
}

export interface WebReportRow {
  id: string
  query_text: string | null
  content: string
  created_at: number
}

export interface WebSourceRow {
  id: string
  url: string
  content_sha256: string
  fetched_at: number
  is_stale: boolean
}

export interface WebFetchDependencyRow {
  report_id: string
  source_id: string
}

export interface CacheSnapshot {
  overview: CacheOverview
  queries: CachedQueryRow[]
  insights: CachedInsightRow[]
  code_nodes: CodeNodeRow[]
  dependencies: InsightDependencyRow[]
  web_queries: WebQueryRow[]
  web_reports: WebReportRow[]
  web_sources: WebSourceRow[]
  web_dependencies: WebFetchDependencyRow[]
}
```

- [ ] **Step 2: Create `WebCacheView.tsx`**

Create `frontend/src/modules/config-ui/WebCacheView.tsx`:

```tsx
import { useEffect, useState } from 'react'
import { NavBar, PageShell } from './NavBar'
import type { CacheSnapshot } from './types'

export function WebCacheView() {
  const [snapshot, setSnapshot] = useState<CacheSnapshot | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [message, setMessage] = useState('')
  const [expandedReport, setExpandedReport] = useState<string | null>(null)

  function load() {
    setStatus('loading')
    fetch('/api/cache')
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<CacheSnapshot>
      })
      .then((data) => {
        setSnapshot(data)
        setStatus('ready')
      })
      .catch((error: Error) => {
        setStatus('error')
        setMessage(error.message)
      })
  }

  useEffect(load, [])

  if (status === 'loading') {
    return (
      <PageShell title="Web fetcher cache" subtitle="Vector-backed web research store">
        <p>Loading web cache…</p>
      </PageShell>
    )
  }

  if (!snapshot) {
    return (
      <PageShell title="Web fetcher cache" subtitle="Vector-backed web research store">
        <p className="config-app__message is-error">Failed to load: {message}</p>
      </PageShell>
    )
  }

  const o = snapshot.overview

  return (
    <PageShell
      title="Web fetcher cache"
      subtitle="Vector-backed web research store in .adjutant/cache.db"
      actions={
        <button type="button" onClick={load} className="config-app__refresh">
          Refresh
        </button>
      }
    >
      <NavBar />

      <section className="cache-stats">
        <div className="cache-stat"><span className="cache-stat__value">{o.web_query_count}</span><span className="cache-stat__label">Web queries</span></div>
        <div className="cache-stat"><span className="cache-stat__value">{o.web_report_count}</span><span className="cache-stat__label">Web reports</span></div>
        <div className="cache-stat"><span className="cache-stat__value">{o.web_source_count}</span><span className="cache-stat__label">Web sources</span></div>
        <div className="cache-stat"><span className="cache-stat__value">{o.web_dependency_count}</span><span className="cache-stat__label">Dependencies</span></div>
      </section>

      <section className="cache-section">
        <h2>Web queries</h2>
        <table className="cache-table">
          <thead><tr><th>Query</th><th>Embedding</th></tr></thead>
          <tbody>
            {snapshot.web_queries.map((q) => (
              <tr key={q.id}>
                <td>{q.raw_text}</td>
                <td>{q.has_embedding ? '✓' : '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="cache-section">
        <h2>Web reports</h2>
        <ul className="cache-list">
          {snapshot.web_reports.map((r) => (
            <li key={r.id} className="cache-list__item">
              <button
                type="button"
                className="cache-list__toggle"
                onClick={() => setExpandedReport(expandedReport === r.id ? null : r.id)}
              >
                {r.query_text ?? r.id} <span className="cache-list__hint">(click to expand)</span>
              </button>
              {expandedReport === r.id && (
                <pre className="cache-list__content">{r.content}</pre>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className="cache-section">
        <h2>Web sources</h2>
        <table className="cache-table">
          <thead><tr><th>URL</th><th>Status</th></tr></thead>
          <tbody>
            {snapshot.web_sources.map((s) => (
              <tr key={s.id}>
                <td><a href={s.url} target="_blank" rel="noreferrer">{s.url}</a></td>
                <td><span className={`chip ${s.is_stale ? 'is-dirty' : 'is-clean'}`}>{s.is_stale ? 'stale' : 'fresh'}</span></td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
    </PageShell>
  )
}
```

- [ ] **Step 3: Add nav link + export**

In `NavBar.tsx`, add to `LINKS`:

```typescript
  { hash: '#/web-cache', label: 'Web cache' },
```

In `index.ts`, add:

```typescript
export { WebCacheView } from './WebCacheView'
```

- [ ] **Step 4: Update `ConfigApp.tsx`**

Three changes: (a) add a web-cache quick-link, (b) replace the "Web Fetcher — browsing model" panel (from the prior branch) with a cache-tunables panel, (c) update the `updateWebFetcher` default to match the new `WebFetcherProfile` fields (drop `browsing`, add `cache_ttl_seconds` + `web_cache_threshold`).

First, add the web-cache link to the existing quick-links `<div>`:

```tsx
      <div className="config-app__quick-links">
        <a href="#/evaluations">Agent evaluations</a>
        <a href="#/cache">Scout semantic cache</a>
        <a href="#/web-cache">Web fetcher cache</a>
      </div>
```

Second, update the `updateWebFetcher` default to the new fields (replacing the old one that referenced `browsing`):

```typescript
  function updateWebFetcher(patch: Partial<WebFetcherProfile>) {
    setConfig((current) => {
      const existing = current.web_fetcher ?? {
        max_search_hops: 3,
        token_budget: 8000,
        cache_ttl_seconds: 604800,
        web_cache_threshold: 0.78,
      }
      return { ...current, web_fetcher: { ...existing, ...patch } }
    })
  }
```

Third, replace the entire "Web Fetcher — browsing model" `<section>` (the one with `groupName="web_fetcher_browsing"` + `LlmClientCatalog`) with a cache-tunables panel:

```tsx
      <section className="agent-panel">
        <header>
          <h2>Web Fetcher — cache tunables</h2>
          <p>Cache TTL and similarity threshold for the web research cache.</p>
        </header>
        <label className="config-app__tunable">
          Cache TTL (seconds)
          <input
            type="number"
            min={3600}
            step={3600}
            value={config.web_fetcher?.cache_ttl_seconds ?? 604800}
            onChange={(e) =>
              updateWebFetcher({ cache_ttl_seconds: Number(e.target.value) })
            }
          />
        </label>
        <label className="config-app__tunable">
          Web cache similarity threshold
          <input
            type="number"
            min={0.5}
            max={1}
            step={0.01}
            value={config.web_fetcher?.web_cache_threshold ?? 0.78}
            onChange={(e) =>
              updateWebFetcher({ web_cache_threshold: Number(e.target.value) })
            }
          />
        </label>
      </section>
```

- [ ] **Step 5: Lint and build**

Run:
```bash
cd frontend
npm run lint
npm run build
```
Expected: lint passes, build succeeds.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/modules/config-ui/
git commit -m "feat(config-ui): add WebCacheView, web cache types, nav link, drop browsing panel"
```

---

## Task 9: Full verification (matches CI)

- [ ] **Step 1: Run the full backend check**

```bash
CXX=g++ cargo fmt -- --check
CXX=g++ cargo clippy --all-targets -- -D warnings
CXX=g++ cargo test --all-targets
```
Expected: fmt clean, clippy zero warnings, all tests pass.

- [ ] **Step 2: Fix any issues found inline**

Common issues to watch for:
- Unused imports after dropping `create_llm_client` / `browsing`.
- `Arc` / `Mutex` imports in `web_fetcher/mod.rs`.
- The `current_unix_timestamp` import in `inspect.rs`.
- The `FetchedPage` / `WebSourceSnapshot` import chain.
- Frontend: unused `WebFetcherProfile` import if the browsing panel was the only consumer.

- [ ] **Step 3: Run the frontend check**

```bash
cd frontend && npm run lint && npm run build && cd ..
```
Expected: clean.

- [ ] **Step 4: Final commit if fixes were made**

```bash
git add -A
git commit -m "chore: clippy/fmt/frontend cleanup for web fetcher search + cache"
```

- [ ] **Step 5: Smoke test the binary**

```bash
cargo build --bin mcp-adjutant
```
Expected: builds cleanly.

---

## Notes for the implementer

- **`search_web` makes real HTTP requests.** Tests that exercise `search_web` will hit the live network. Keep search_web out of the offline test suite — test it via the fixture HTML in `parse_ddg_results`, not via `search_and_fetch`.
- **The `&self` constraint on `LlmTool::invoke`** is why source collection uses `Arc<Mutex<Vec<FetchedPage>>>` shared between the agent and the tool, not a direct `&mut` borrow. This is the same pattern scout uses for `context.touched_files`, adapted for the tool layer.
- **`current_unix_timestamp` returns `i64` seconds.** Web sources use seconds for both `fetched_at` and TTL math (unlike `code_nodes` which use millis for mtime).
- **`hash_query_text` is reused** for both query IDs and source IDs (SHA-256 hex of the URL string). No new hash function needed.
- **`WEB_CACHE_THRESHOLD = 0.78`** is lower than scout's 0.82 because web-search paraphrases ("latest tokio docs" vs "tokio async runtime documentation") cluster further apart than code queries. Tune after seeing real data.
- **The frontend `load_cache_snapshot` now takes `ttl_seconds`** — the config server hard-codes 604_800. If you later want the TTL configurable per-request, pass it as a query param. For now it matches the default.
- **Graceful config upgrade**: `WebFetcherProfile` changed fields. Old configs with `browsing` will fail to deserialize. Either (a) add `#[serde(default)]` on all new fields and ignore unknown fields, or (b) accept that the config resets. Since `#[serde(default)]` is already on the `web_fetcher: Option<WebFetcherProfile>` field, a deserialization failure inside the profile will bubble up. Add `#[serde(default)]` to each new field in `WebFetcherProfile` to handle old configs gracefully.
