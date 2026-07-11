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

/// Convert raw HTML to markdown using htmd. Falls back to the raw HTML on error.
pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
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
                    result.title,
                    result.url,
                    truncate_markdown(&page.markdown, 4_000)
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
    digest
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
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
        assert_eq!(url_encode("a&b"), "a%26b");
    }
}
