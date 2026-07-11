use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

use crate::cache::project::hash_query_text;

const USER_AGENT: &str = "mcp-adjutant/1.0 (web fetcher)";
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub url: String,
    pub markdown: String,
    pub content_sha256: String,
}

pub fn parse_brave_results(json: &str) -> Result<Vec<SearchResult>, String> {
    let root: Value = serde_json::from_str(json)
        .map_err(|err| format!("Brave Search JSON parse failed: {err}"))?;
    let results = root
        .pointer("/web/results")
        .and_then(Value::as_array)
        .ok_or_else(|| "Brave Search response missing web.results".to_string())?;

    let mut out = Vec::new();
    for item in results {
        let url = item
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if url.is_empty() {
            continue;
        }
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let snippet = item
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        out.push(SearchResult {
            url,
            title,
            snippet,
        });
    }
    Ok(out)
}

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

pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}

fn url_authority(url: &str) -> Result<&str, String> {
    let allow_local = std::env::var("MCP_ADJUTANT_ALLOW_LOCAL_FETCH").is_ok();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| {
            if allow_local {
                url.strip_prefix("http://")
            } else {
                None
            }
        })
        .ok_or_else(|| format!("only HTTPS URLs are allowed: {url}"))?;
    if rest.is_empty() {
        return Err(format!("missing host in URL: {url}"));
    }
    Ok(rest.split(&['/', '?', '#'][..]).next().unwrap_or(rest))
}

pub fn validate_fetch_url(url: &str) -> Result<(), String> {
    let allow_local = std::env::var("MCP_ADJUTANT_ALLOW_LOCAL_FETCH").is_ok();
    let authority = url_authority(url)?;
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or(authority)
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');

    if host.is_empty() || (!allow_local && host.eq_ignore_ascii_case("localhost")) {
        return Err(format!("blocked host: {host}"));
    }
    if allow_local && (host.starts_with("127.0.0.1") || host.eq_ignore_ascii_case("localhost")) {
        return Ok(());
    }
    if host.ends_with(".local") || host.ends_with(".internal") {
        return Err(format!("blocked host: {host}"));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(format!("blocked IP: {ip}"));
        }
        return Ok(());
    }

    Ok(())
}

pub fn web_sources_still_valid(sources: &[(String, String)]) -> bool {
    sources.iter().all(|(url, stored_hash)| {
        validate_fetch_url(url).is_ok()
            && resolve_public_host(url).is_ok()
            && fetch_page_as_markdown(url).is_ok_and(|page| page.content_sha256 == *stored_hash)
    })
}

pub fn fetch_page_as_markdown(url: &str) -> Result<FetchedPage, String> {
    let (final_url, html) = fetch_html_validated(url)?;
    let markdown = html_to_markdown(&html);
    let content_sha256 = hash_query_text(&markdown);

    Ok(FetchedPage {
        url: final_url,
        markdown,
        content_sha256,
    })
}

pub fn search_and_fetch(
    query: &str,
    max_pages: usize,
    brave_api_key: Option<&str>,
) -> Result<(String, Vec<FetchedPage>), String> {
    let results = if std::env::var("MCP_ADJUTANT_DDG_HTML_URL").is_ok() {
        search_ddg(query)?
    } else {
        search_brave(query, max_pages, brave_api_key)?
    };

    assemble_search_report(query, results.into_iter().take(max_pages))
}

fn search_brave(
    query: &str,
    max_pages: usize,
    brave_api_key: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let api_key = brave_api_key
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("MCP_ADJUTANT_BRAVE_API_KEY")
                .ok()
                .filter(|key| !key.is_empty())
        })
        .ok_or_else(|| {
            "Brave Search API key not configured (set web_fetcher.brave_api_key in config)"
                .to_string()
        })?;

    let encoded = url_encode(query);
    let count = max_pages.clamp(1, 20);
    let search_url = std::env::var("MCP_ADJUTANT_BRAVE_SEARCH_URL")
        .map(|template| {
            template
                .replace("{query}", &encoded)
                .replace("{count}", &count.to_string())
        })
        .unwrap_or_else(|_| format!("{BRAVE_SEARCH_URL}?q={encoded}&count={count}"));

    let (status, body) = match http_agent()
        .get(&search_url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/json")
        .set("X-Subscription-Token", api_key.as_str())
        .call()
    {
        Ok(response) => (
            response.status(),
            read_limited_string(response.into_reader())?,
        ),
        Err(ureq::Error::Status(status, response)) => {
            (status, read_limited_string(response.into_reader())?)
        }
        Err(err) => return Err(format!("Brave Search request failed: {err}")),
    };
    if status == 401 || status == 403 {
        return Err("Brave Search API key invalid or unauthorized".to_string());
    }
    if status == 429 {
        return Err("Brave Search rate limit exceeded".to_string());
    }
    if !(200..300).contains(&status) {
        return Err(format!("Brave Search returned HTTP {status}: {body}"));
    }

    let results = parse_brave_results(&body)?;
    if results.is_empty() {
        return Err(format!(
            "Brave Search returned no results for query: {query}"
        ));
    }
    Ok(results)
}

fn search_ddg(query: &str) -> Result<Vec<SearchResult>, String> {
    let encoded = url_encode(query);
    let ddg_url = std::env::var("MCP_ADJUTANT_DDG_HTML_URL")
        .map(|template| template.replace("{query}", &encoded))
        .unwrap_or_else(|_| format!("https://html.duckduckgo.com/html/?q={encoded}"));

    let response = http_agent()
        .get(&ddg_url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|err| format!("DuckDuckGo request failed: {err}"))?;

    let html = read_limited_string(response.into_reader())?;
    if html.contains("Unfortunately, bots use DuckDuckGo too.") {
        return Err(
            "DuckDuckGo blocked the request (bot challenge); use Brave Search instead".to_string(),
        );
    }
    let results = parse_ddg_results(&html);
    if results.is_empty() {
        return Err(format!("no results found for query: {query}"));
    }
    Ok(results)
}

fn assemble_search_report(
    query: &str,
    results: impl Iterator<Item = SearchResult>,
) -> Result<(String, Vec<FetchedPage>), String> {
    let mut pages = Vec::new();
    let mut sections = Vec::new();

    for result in results {
        let snippet_block = if result.snippet.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", result.snippet)
        };
        match fetch_page_as_markdown(&result.url) {
            Ok(page) => {
                sections.push(format!(
                    "## [{}]({})\n\n{}{}",
                    result.title,
                    result.url,
                    snippet_block,
                    truncate_markdown(&page.markdown, 4_000)
                ));
                pages.push(page);
            }
            Err(err) => {
                sections.push(format!(
                    "## [{}]({})\n\n{}*(could not fetch: {err})*\n",
                    result.title, result.url, snippet_block
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

pub fn resolve_public_host(url: &str) -> Result<(), String> {
    resolve_host_addrs(&fetch_host(url)?)?;
    Ok(())
}

fn fetch_html_validated(start_url: &str) -> Result<(String, String), String> {
    let mut current = start_url.to_string();
    for hop in 0..=MAX_REDIRECTS {
        validate_fetch_url(&current)?;
        let host = fetch_host(&current)?;
        let addrs = resolve_host_addrs(&host)?;
        let agent = pinned_http_agent(&host, &addrs);

        let response = match agent.get(&current).set("User-Agent", USER_AGENT).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) if (300..400).contains(&status) => {
                if hop == MAX_REDIRECTS {
                    return Err(format!("too many redirects fetching {start_url}"));
                }
                let location = response
                    .header("Location")
                    .ok_or_else(|| format!("redirect from {current} missing Location header"))?;
                current = resolve_redirect_url(&current, location)?;
                continue;
            }
            Err(ureq::Error::Status(status, _)) => {
                return Err(format!("failed to fetch {current}: HTTP {status}"));
            }
            Err(err) => return Err(format!("failed to fetch {current}: {err}")),
        };

        let status = response.status();
        if (300..400).contains(&status) {
            if hop == MAX_REDIRECTS {
                return Err(format!("too many redirects fetching {start_url}"));
            }
            let location = response
                .header("Location")
                .ok_or_else(|| format!("redirect from {current} missing Location header"))?;
            current = resolve_redirect_url(&current, location)?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(format!("failed to fetch {current}: HTTP {status}"));
        }
        let html = read_limited_string(response.into_reader())?;
        return Ok((current, html));
    }
    Err(format!("too many redirects fetching {start_url}"))
}

fn resolve_host_addrs(host: &str) -> Result<Vec<SocketAddr>, String> {
    let allow_local = std::env::var("MCP_ADJUTANT_ALLOW_LOCAL_FETCH").is_ok();
    if allow_local && (host.starts_with("127.0.0.1") || host.eq_ignore_ascii_case("localhost")) {
        return (host, 80)
            .to_socket_addrs()
            .or_else(|_| (host, 443).to_socket_addrs())
            .map(|iter| iter.collect())
            .map_err(|err| format!("failed to resolve {host}: {err}"));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(format!("blocked IP: {ip}"));
        }
        return Ok(vec![SocketAddr::new(ip, 443)]);
    }

    let addrs: Vec<_> = (host, 443)
        .to_socket_addrs()
        .map_err(|err| format!("failed to resolve {host}: {err}"))?
        .collect();
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(format!("blocked IP for {host}: {}", addr.ip()));
        }
    }
    Ok(addrs)
}

fn resolve_redirect_url(base: &str, location: &str) -> Result<String, String> {
    let location = location.trim();
    if location.starts_with("https://") {
        return Ok(location.to_string());
    }
    let allow_local = std::env::var("MCP_ADJUTANT_ALLOW_LOCAL_FETCH").is_ok();
    if allow_local && location.starts_with("http://") {
        return Ok(location.to_string());
    }
    if location.starts_with("//") {
        return Ok(format!("https:{location}"));
    }
    Err(format!(
        "unsupported redirect target: {location} (from {base})"
    ))
}

fn pinned_http_agent(host: &str, addrs: &[SocketAddr]) -> ureq::Agent {
    let host = host.to_string();
    let pinned = addrs.to_vec();
    ureq::AgentBuilder::new()
        .timeout_connect(FETCH_TIMEOUT)
        .timeout_read(FETCH_TIMEOUT)
        .redirects(0)
        .resolver(move |name: &str| {
            if name == host {
                Ok(pinned.clone())
            } else {
                name.to_socket_addrs().map(Iterator::collect)
            }
        })
        .build()
}

fn fetch_host(url: &str) -> Result<String, String> {
    let authority = url_authority(url)?;
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or(authority)
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if host.is_empty() {
        return Err(format!("missing host in URL: {url}"));
    }
    Ok(host)
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(FETCH_TIMEOUT)
        .timeout_read(FETCH_TIMEOUT)
        .redirects(0)
        .build()
}

fn read_limited_string(reader: impl Read) -> Result<String, String> {
    let mut buf = Vec::new();
    reader
        .take((MAX_RESPONSE_BYTES as u64).saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|err| format!("failed to read response body: {err}"))?;
    if buf.len() > MAX_RESPONSE_BYTES {
        return Err(format!("response exceeds {MAX_RESPONSE_BYTES} byte limit"));
    }
    String::from_utf8(buf).map_err(|err| format!("response body is not valid UTF-8: {err}"))
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] >= 224
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
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
    let cleaned = raw
        .strip_prefix("//duckduckgo.com/l/?uddg=")
        .or_else(|| raw.strip_prefix("https://duckduckgo.com/l/?uddg="))
        .or_else(|| raw.strip_prefix("http://duckduckgo.com/l/?uddg="))
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
    let mut bytes_out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            bytes_out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                bytes_out.push(byte);
                i += 3;
            } else {
                bytes_out.push(bytes[i]);
                i += 1;
            }
        } else {
            bytes_out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&bytes_out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_brave_results_extracts_urls_and_titles() {
        let json = include_str!("../../tests/fixtures/web_fetcher/brave_results.json");
        let results = parse_brave_results(json).expect("parse brave json");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://example.com/tokio/docs");
        assert_eq!(results[0].title, "Tokio Async Runtime");
        assert!(results[0].snippet.contains("Official Tokio"));
    }

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
    fn parse_ddg_results_unwraps_redirect_links() {
        let html = include_str!("../../tests/fixtures/web_fetcher/ddg_redirect_results.html");
        let results = parse_ddg_results(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/tokio/docs");
    }

    #[test]
    fn validate_fetch_url_rejects_private_targets() {
        for url in [
            "http://example.com",
            "https://127.0.0.1/path",
            "https://localhost/path",
            "https://169.254.169.254/latest/meta-data",
            "https://10.0.0.1/internal",
            "file:///etc/passwd",
        ] {
            assert!(
                validate_fetch_url(url).is_err(),
                "expected {url} to be blocked"
            );
        }
    }

    #[test]
    fn validate_fetch_url_allows_public_https() {
        assert!(validate_fetch_url("https://example.com/docs").is_ok());
    }

    #[test]
    fn html_to_markdown_converts_basic_html() {
        let html = "<h1>Title</h1><p>Hello <strong>world</strong>.</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Hello"));
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

    #[test]
    fn url_decode_handles_utf8_percent_sequences() {
        assert_eq!(url_decode("%C3%A9"), "é");
        assert_eq!(url_decode("caf%C3%A9"), "café");
    }
}
