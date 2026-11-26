//! The `WebSearch` tool and its providers.

use super::config::{ProviderConf, SearchConfig, PROVIDERS};
use super::net::{self, NetPolicy};
use super::{truncate_chars, WebConfig, WEB_SEARCH};
use crate::{str_arg, Tool, ToolContext, ToolKind, ToolOutput};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Url;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::time::Instant;

const DEFAULT_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;
const SNIPPET_CHARS: usize = 300;
const MAX_BODY: usize = 2 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

const DDG_URL: &str = "https://html.duckduckgo.com/html/";
const BRAVE_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const TAVILY_URL: &str = "https://api.tavily.com/search";

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, PartialEq)]
enum SearchError {
    /// The provider is rate limiting or challenging us; try again later.
    Unavailable(String),
    Failed(String),
}

fn failed<T>(m: impl Into<String>) -> Result<T, SearchError> {
    Err(SearchError::Failed(m.into()))
}

/// Spaces successive requests at least `min` apart. Callers wait (holding their place in line)
/// rather than being refused.
pub struct RateLimiter {
    min: Duration,
    last: tokio::sync::Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new(min: Duration) -> Self {
        Self {
            min,
            last: tokio::sync::Mutex::new(None),
        }
    }

    /// The process-wide limiter for DuckDuckGo (shared by every tool instance and session).
    pub fn ddg(min: Duration) -> Arc<Self> {
        static G: OnceLock<Arc<RateLimiter>> = OnceLock::new();
        G.get_or_init(|| Arc::new(RateLimiter::new(min))).clone()
    }

    /// Wait for this caller's slot.
    pub async fn acquire(&self) {
        let mut last = self.last.lock().await;
        if let Some(l) = *last {
            let ready = l + self.min;
            let now = Instant::now();
            if ready > now {
                tokio::time::sleep(ready - now).await;
            }
        }
        *last = Some(Instant::now());
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing

/// Decode the HTML entities that occur in search snippets.
fn decode_entities(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
    re.replace_all(s, |c: &regex::Captures| {
        let e = &c[1];
        let named = match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "hellip" => Some('\u{2026}'),
            "ndash" => Some('\u{2013}'),
            "mdash" => Some('\u{2014}'),
            _ => None,
        };
        let code = if let Some(h) = e.strip_prefix("#x") {
            u32::from_str_radix(h, 16).ok()
        } else if let Some(d) = e.strip_prefix('#') {
            d.parse().ok()
        } else {
            None
        };
        named
            .or_else(|| code.and_then(char::from_u32))
            .map_or_else(|| c[0].to_string(), String::from)
    })
    .into_owned()
}

/// Strip tags, decode entities and collapse whitespace.
fn text_of(html: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let tags = RE.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    let plain = decode_entities(&tags.replace_all(html, ""));
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?s)\b{name}\s*=\s*(?:"([^"]*)"|'([^']*)')"#)).ok()?;
    let c = re.captures(tag)?;
    Some(decode_entities(c.get(1).or_else(|| c.get(2))?.as_str()))
}

/// DuckDuckGo wraps result links as `//duckduckgo.com/l/?uddg=<percent-encoded URL>&rut=...`.
fn is_ddg_host(h: &str) -> bool {
    let h = h.trim_end_matches('.').to_ascii_lowercase();
    h == "duckduckgo.com" || h.ends_with(".duckduckgo.com")
}

pub fn unwrap_ddg_link(href: &str) -> Option<String> {
    let href = href.trim();
    let abs = if href.starts_with("//") {
        format!("https:{href}")
    } else if href.starts_with('/') {
        format!("https://duckduckgo.com{href}")
    } else {
        href.to_string()
    };
    let u = Url::parse(&abs).ok()?;
    if u.host_str().is_some_and(is_ddg_host) {
        if u.path() == "/y.js" {
            return None; // an ad redirect
        }
        if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "uddg") {
            return matches!(Url::parse(&v).ok()?.scheme(), "http" | "https")
                .then(|| v.into_owned());
        }
        return None;
    }
    matches!(u.scheme(), "http" | "https").then(|| u.to_string())
}

fn is_ddg_anomaly(body: &str) -> bool {
    body.contains("anomaly-modal") || body.contains("anomaly.js") || body.contains("anomaly_modal")
}

/// Parse the HTML results page of `html.duckduckgo.com/html/`.
fn parse_ddg(html: &str) -> Result<Vec<Hit>, SearchError> {
    if is_ddg_anomaly(html) {
        return Err(SearchError::Unavailable(
            "DuckDuckGo is asking for a CAPTCHA / blocking automated searches".into(),
        ));
    }
    static LINK: OnceLock<Regex> = OnceLock::new();
    static SNIP: OnceLock<Regex> = OnceLock::new();
    let link = LINK.get_or_init(|| {
        Regex::new(r#"(?s)<a\s[^>]*class="[^"]*\bresult__a\b[^"]*"[^>]*>(.*?)</a>"#).unwrap()
    });
    let snip = SNIP.get_or_init(|| {
        Regex::new(r#"(?s)<a\s[^>]*class="[^"]*\bresult__snippet\b[^"]*"[^>]*>(.*?)</a>"#).unwrap()
    });
    let mut hits = Vec::new();
    for block in html.split("<div class=\"result ").skip(1) {
        let class_end = block.find('>').unwrap_or(0);
        if block[..class_end].contains("result--ad") {
            continue;
        }
        let Some(m) = link.find(block) else { continue };
        let open_end = m.as_str().find('>').unwrap_or(0) + 1;
        let open_tag = &m.as_str()[..open_end];
        let Some(url) = attr(open_tag, "href").and_then(|h| unwrap_ddg_link(&h)) else {
            continue;
        };
        let title = text_of(&link.captures(block).unwrap()[1]);
        let snippet = snip
            .captures(block)
            .map(|c| text_of(&c[1]))
            .unwrap_or_default();
        if !title.is_empty() {
            hits.push(Hit {
                title,
                url,
                snippet,
            });
        }
    }
    if hits.is_empty() && !html.contains("no-results") && !html.contains("No results") {
        return failed(
            "DuckDuckGo returned a page with no recognisable results (its markup may have changed)",
        );
    }
    Ok(hits)
}

fn str_field(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn hits_from(arr: Option<&Vec<Value>>, snippet_key: &str) -> Vec<Hit> {
    arr.map(|a| {
        a.iter()
            .filter_map(|r| {
                let url = str_field(r, "url");
                (!url.is_empty()).then(|| Hit {
                    title: text_of(&str_field(r, "title")),
                    url,
                    snippet: text_of(&str_field(r, snippet_key)),
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn parse_searxng(body: &str) -> Result<Vec<Hit>, SearchError> {
    let v: Value = serde_json::from_str(body).map_err(|_| {
        SearchError::Failed(
            "the SearXNG instance did not return JSON (enable the json format under `search.formats` in its settings.yml)"
                .into(),
        )
    })?;
    Ok(hits_from(
        v.get("results").and_then(Value::as_array),
        "content",
    ))
}

fn parse_brave(body: &str) -> Result<Vec<Hit>, SearchError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| SearchError::Failed(format!("unreadable Brave response: {e}")))?;
    Ok(hits_from(
        v.pointer("/web/results").and_then(Value::as_array),
        "description",
    ))
}

fn parse_tavily(body: &str) -> Result<Vec<Hit>, SearchError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| SearchError::Failed(format!("unreadable Tavily response: {e}")))?;
    Ok(hits_from(
        v.get("results").and_then(Value::as_array),
        "content",
    ))
}

// ---------------------------------------------------------------------------------------------
// The tool

type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct WebSearch {
    cfg: SearchConfig,
    policy: NetPolicy,
    ddg_limiter: Arc<RateLimiter>,
    env: EnvFn,
}

impl WebSearch {
    pub fn new(cfg: &WebConfig) -> Self {
        let mut s = Self::with_policy(
            cfg.search.clone(),
            NetPolicy::new(cfg.allow_private),
            RateLimiter::ddg(Duration::from_secs_f64(
                cfg.search.ddg_min_interval_secs.clamp(0.0, 3600.0),
            )),
        );
        s.env = Arc::new(|n| std::env::var(n).ok().filter(|v| !v.is_empty()));
        s
    }

    pub fn with_policy(
        cfg: SearchConfig,
        policy: NetPolicy,
        ddg_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            cfg,
            policy,
            ddg_limiter,
            env: Arc::new(|_| None),
        }
    }

    #[cfg(test)]
    fn with_env(mut self, f: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.env = Arc::new(f);
        self
    }

    fn key(
        &self,
        provider: &str,
        conf: &ProviderConf,
        default_env: &str,
    ) -> Result<String, SearchError> {
        if let Some(k) = conf
            .api_key
            .as_ref()
            .map(|k| k.expose())
            .filter(|k| !k.is_empty())
        {
            return Ok(k.to_string());
        }
        let var = conf.api_key_env.as_deref().unwrap_or(default_env);
        if let Some(k) = (self.env)(var) {
            return Ok(k);
        }
        failed(format!(
            "the `{provider}` search provider needs an API key: set `web.search.{provider}.api_key_name` (a named key in credentials.toml, see `rc keys add`) or the {var} environment variable"
        ))
    }

    /// Send one provider request through the network gate; returns status and body.
    async fn send(
        &self,
        ctx: &ToolContext,
        provider: &str,
        req: impl FnOnce(&reqwest::Client) -> reqwest::RequestBuilder,
        url: &Url,
    ) -> Result<(u16, String), SearchError> {
        // A self-hosted SearXNG commonly lives on localhost or the LAN: configuring its URL is
        // consent to reach it.
        let mut policy = self.policy.clone();
        if provider == "searxng" {
            policy.allow_private = true;
        }
        net::check_host(url, &policy).map_err(SearchError::Failed)?;
        if let Some(h) = url.host_str() {
            net::check_network(ctx, h, true)
                .await
                .map_err(SearchError::Failed)?;
        }
        let client =
            net::client(&policy, CONNECT_TIMEOUT, TOTAL_TIMEOUT).map_err(SearchError::Failed)?;
        let mut resp = tokio::select! {
            r = req(&client).send() => r.map_err(|e| SearchError::Failed(format!("request to {} failed: {}", url.host_str().unwrap_or("?"), chain(&e))))?,
            _ = ctx.cancel.cancelled() => return failed("cancelled"),
        };
        let status = resp.status().as_u16();
        let mut body = Vec::new();
        loop {
            let c = tokio::select! {
                c = resp.chunk() => c.map_err(|e| SearchError::Failed(format!("reading the response failed: {}", chain(&e))))?,
                _ = ctx.cancel.cancelled() => return failed("cancelled"),
            };
            match c {
                Some(c) => {
                    body.extend_from_slice(&c);
                    if body.len() >= MAX_BODY {
                        break;
                    }
                }
                None => break,
            }
        }
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }

    async fn run_provider(
        &self,
        name: &str,
        query: &str,
        site: Option<&str>,
        n: usize,
        ctx: &ToolContext,
    ) -> Result<Vec<Hit>, SearchError> {
        let conf = self.cfg.conf(name);
        let q = match (site, name) {
            (Some(s), "duckduckgo" | "searxng" | "brave") => format!("{query} site:{s}"),
            _ => query.to_string(),
        };
        let endpoint = |default: Option<&str>| -> Result<Url, SearchError> {
            let raw = conf.url.as_deref().or(default).ok_or_else(|| {
                SearchError::Failed(
                    "the `searxng` provider needs `web.search.url` (or `web.search.searxng.url`)"
                        .into(),
                )
            })?;
            Url::parse(raw)
                .map_err(|e| SearchError::Failed(format!("invalid {name} url `{raw}`: {e}")))
        };
        match name {
            "duckduckgo" => {
                let mut url = endpoint(Some(DDG_URL))?;
                url.query_pairs_mut().append_pair("q", &q);
                self.ddg_limiter.acquire().await;
                let (status, body) = self.send(ctx, name, |c| c.get(url.clone()), &url).await?;
                if status == 202 || status == 429 || is_ddg_anomaly(&body) {
                    return Err(SearchError::Unavailable(format!(
                        "DuckDuckGo is rate limiting or challenging automated searches (HTTP {status})"
                    )));
                }
                if status >= 400 {
                    return failed(format!("DuckDuckGo returned HTTP {status}"));
                }
                parse_ddg(&body)
            }
            "searxng" => {
                let base = endpoint(None)?;
                let mut url = base.clone();
                let path = format!("{}/search", base.path().trim_end_matches('/'));
                url.set_path(&path);
                url.query_pairs_mut()
                    .append_pair("q", &q)
                    .append_pair("format", "json");
                let (status, body) = self.send(ctx, name, |c| c.get(url.clone()), &url).await?;
                match status {
                    200 => parse_searxng(&body),
                    403 => failed("the SearXNG instance refused the request (HTTP 403): the json format is probably not enabled (`search.formats` in its settings.yml)"),
                    429 => Err(SearchError::Unavailable("the SearXNG instance is rate limiting requests (HTTP 429)".into())),
                    s => failed(format!("the SearXNG instance returned HTTP {s}")),
                }
            }
            "brave" => {
                let key = self.key(name, &conf, "BRAVE_API_KEY")?;
                let mut url = endpoint(Some(BRAVE_URL))?;
                url.query_pairs_mut()
                    .append_pair("q", &q)
                    .append_pair("count", &n.to_string());
                let (status, body) = self
                    .send(
                        ctx,
                        name,
                        |c| {
                            c.get(url.clone())
                                .header("X-Subscription-Token", key)
                                .header(reqwest::header::ACCEPT, "application/json")
                        },
                        &url,
                    )
                    .await?;
                match status {
                    200 => parse_brave(&body),
                    401 | 403 => failed(format!("Brave rejected the API key (HTTP {status})")),
                    429 => Err(SearchError::Unavailable(
                        "Brave rate limit reached (HTTP 429)".into(),
                    )),
                    s => failed(format!("Brave returned HTTP {s}")),
                }
            }
            "tavily" => {
                let key = self.key(name, &conf, "TAVILY_API_KEY")?;
                let url = endpoint(Some(TAVILY_URL))?;
                let mut payload = json!({"query": q, "max_results": n});
                if let Some(s) = site {
                    payload["include_domains"] = json!([s]);
                }
                let body_text = payload.to_string();
                let (status, body) = self
                    .send(
                        ctx,
                        name,
                        |c| {
                            c.post(url.clone())
                                .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
                                .header(reqwest::header::CONTENT_TYPE, "application/json")
                                .body(body_text)
                        },
                        &url,
                    )
                    .await?;
                match status {
                    200 => parse_tavily(&body),
                    401 | 403 => failed(format!("Tavily rejected the API key (HTTP {status})")),
                    429 => Err(SearchError::Unavailable(
                        "Tavily rate limit reached (HTTP 429)".into(),
                    )),
                    s => failed(format!("Tavily returned HTTP {s}")),
                }
            }
            other => failed(format!(
                "unknown search provider `{other}` (known: {})",
                PROVIDERS.join(", ")
            )),
        }
    }
}

fn chain(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// A bare domain from `site` (scheme, path and `www.` noise removed), if it looks like one.
fn clean_site(s: &str) -> Option<String> {
    let s = s
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let s = s.split('/').next().unwrap_or("").trim_start_matches("www.");
    (!s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
    .then(|| s.to_ascii_lowercase())
}

fn format_hits(provider: &str, query: &str, hits: &[Hit]) -> String {
    if hits.is_empty() {
        return format!("No results for \"{query}\" ({provider}).");
    }
    let mut out = format!("Search results for \"{query}\" ({provider}):\n");
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!("\n{}. {}\n   {}\n", i + 1, h.title, h.url));
        if !h.snippet.is_empty() {
            out.push_str(&format!(
                "   {}\n",
                truncate_chars(&h.snippet, SNIPPET_CHARS).0
            ));
        }
    }
    out
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        WEB_SEARCH
    }

    fn description(&self) -> String {
        "Search the web and return a numbered list of results (title, URL, snippet). Use \
         WebFetch on a result's URL to read the page."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "The search query."},
                "max_results": {
                    "type": "integer",
                    "description": format!("Number of results (default {DEFAULT_RESULTS}, max {MAX_RESULTS}).")
                },
                "site": {"type": "string", "description": "Only results from this domain, e.g. docs.rs."}
            },
            "required": ["query"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Fetch
    }

    fn title(&self, input: &Value) -> String {
        match input.get("query").and_then(Value::as_str) {
            Some(q) => format!("Search \"{q}\""),
            None => "WebSearch".into(),
        }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let query = str_arg(&input, "query")?.trim();
        if query.is_empty() {
            return Ok(ToolOutput::error("the query is empty"));
        }
        let n = input
            .get("max_results")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_RESULTS, |n| (n as usize).clamp(1, MAX_RESULTS));
        let site = match input
            .get("site")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            Some(s) => match clean_site(s) {
                Some(d) => Some(d),
                None => return Ok(ToolOutput::error(format!("`{s}` is not a domain name"))),
            },
            None => None,
        };
        // Providers in order; others are tried only when `fallback` lists them.
        let mut order = vec![self.cfg.provider.clone()];
        for f in &self.cfg.fallback {
            if !order.contains(f) {
                order.push(f.clone());
            }
        }
        let mut errors = Vec::new();
        for name in &order {
            match self
                .run_provider(name, query, site.as_deref(), n, ctx)
                .await
            {
                Ok(mut hits) => {
                    hits.truncate(n);
                    return Ok(ToolOutput::ok(format_hits(name, query, &hits)));
                }
                Err(e) => {
                    let msg = match &e {
                        SearchError::Unavailable(m) => {
                            format!("search temporarily unavailable ({name}): {m}. Try again later, or WebFetch a URL you already know.")
                        }
                        SearchError::Failed(m) => format!("search failed ({name}): {m}"),
                    };
                    errors.push(msg);
                }
            }
        }
        Ok(ToolOutput::error(errors.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ctx;
    use crate::SandboxHandle;
    use ruckcode_sandbox::proxy::Gate;
    use ruckcode_sandbox::{NetworkMode, SandboxConfig};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const DDG_FIXTURE: &str = include_str!("../../tests/fixtures/ddg_results.html");
    const DDG_ANOMALY: &str = include_str!("../../tests/fixtures/ddg_anomaly.html");

    #[test]
    fn parses_the_ddg_fixture() {
        let hits = parse_ddg(DDG_FIXTURE).unwrap();
        assert_eq!(hits.len(), 3, "{hits:#?}");
        assert_eq!(hits[0].title, "Tokio - An asynchronous Rust runtime");
        assert_eq!(hits[0].url, "https://tokio.rs/");
        assert!(hits[0]
            .snippet
            .starts_with("Tokio is a library for writing fast"));
        assert!(!hits[0].snippet.contains("<b>"));
        // The uddg parameter is percent-decoded ("%2D" -> "-") and the rut tracker dropped.
        assert_eq!(hits[2].url, "https://github.com/tokio-rs/tokio");
        // Entities are decoded.
        assert!(
            hits[2].snippet.contains("operating system's event queue"),
            "{}",
            hits[2].snippet
        );
        // The sponsored result is skipped.
        assert!(hits.iter().all(|h| !h.title.contains("Sponsored")));
    }

    #[test]
    fn detects_the_anomaly_page_and_empty_results() {
        assert!(matches!(
            parse_ddg(DDG_ANOMALY),
            Err(SearchError::Unavailable(_))
        ));
        assert_eq!(
            parse_ddg("<html><div class=\"no-results\">No results.</div></html>").unwrap(),
            vec![]
        );
        assert!(matches!(
            parse_ddg("<html>who knows</html>"),
            Err(SearchError::Failed(_))
        ));
    }

    #[test]
    fn unwraps_ddg_redirect_links() {
        assert_eq!(
            unwrap_ddg_link(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1&rut=abc"
            )
            .as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(
            unwrap_ddg_link("/l/?uddg=https%3A%2F%2Fexample.com%2F").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            unwrap_ddg_link("https://example.org/x").as_deref(),
            Some("https://example.org/x")
        );
        assert_eq!(
            unwrap_ddg_link("https://html.duckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2F")
                .as_deref(),
            Some("https://a.example/")
        );
        // A look-alike host is an ordinary external link, not a DDG redirect.
        assert_eq!(
            unwrap_ddg_link("https://notduckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2F")
                .as_deref(),
            Some("https://notduckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2F")
        );
        assert!(
            is_ddg_host("DuckDuckGo.com")
                && is_ddg_host("html.duckduckgo.com")
                && !is_ddg_host("notduckduckgo.com")
        );
        assert_eq!(unwrap_ddg_link("//duckduckgo.com/y.js?ad_provider=x"), None);
        assert_eq!(
            unwrap_ddg_link("//duckduckgo.com/l/?uddg=javascript%3Aalert(1)"),
            None
        );
    }

    #[test]
    fn json_providers_parse() {
        let s = parse_searxng(r#"{"results":[{"title":"A <b>x</b>","url":"https://a.example/","content":"snip &amp; more"},{"title":"no url"}]}"#).unwrap();
        assert_eq!(
            s,
            vec![Hit {
                title: "A x".into(),
                url: "https://a.example/".into(),
                snippet: "snip & more".into()
            }]
        );
        assert!(
            matches!(parse_searxng("<html>not json</html>"), Err(SearchError::Failed(m)) if m.contains("json format"))
        );
        let b = parse_brave(
            r#"{"web":{"results":[{"title":"B","url":"https://b.example/","description":"d"}]}}"#,
        )
        .unwrap();
        assert_eq!(b[0].snippet, "d");
        assert!(parse_brave("{}").unwrap().is_empty());
        let t = parse_tavily(
            r#"{"results":[{"title":"T","url":"https://t.example/","content":"c","score":0.9}]}"#,
        )
        .unwrap();
        assert_eq!(t[0].url, "https://t.example/");
    }

    #[tokio::test]
    async fn the_rate_limiter_spaces_requests() {
        let l = RateLimiter::new(Duration::from_millis(250));
        let t0 = Instant::now();
        l.acquire().await;
        assert!(t0.elapsed() < Duration::from_millis(200));
        l.acquire().await;
        assert!(t0.elapsed() >= Duration::from_millis(250));
        l.acquire().await;
        assert!(t0.elapsed() >= Duration::from_millis(500));
    }

    // ---- mock servers -------------------------------------------------------------------

    struct Seen {
        head: String,
        body: String,
    }

    /// Serves `(status, content_type, body)` for every request and records them.
    async fn mock(
        status: u16,
        ct: &'static str,
        body: String,
    ) -> (u16, Arc<std::sync::Mutex<Vec<Seen>>>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    break;
                };
                let (body, s2) = (body.clone(), s2.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let (head_end, len) = loop {
                        let n = c.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (p + 4, len);
                        }
                    };
                    while buf.len() < head_end + len {
                        let n = c.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    s2.lock().unwrap().push(Seen {
                        head: String::from_utf8_lossy(&buf[..head_end]).into_owned(),
                        body: String::from_utf8_lossy(&buf[head_end..]).into_owned(),
                    });
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = c.write_all(resp.as_bytes()).await;
                    let _ = c.shutdown().await;
                });
            }
        });
        (port, seen)
    }

    fn tool(cfg: &str) -> WebSearch {
        let c: WebConfig = toml::from_str(cfg).unwrap();
        WebSearch::with_policy(
            c.search,
            NetPolicy::new(true),
            Arc::new(RateLimiter::new(Duration::ZERO)),
        )
    }

    fn cx() -> (tempfile::TempDir, ToolContext) {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        (d, c)
    }

    async fn search(t: &WebSearch, args: Value, c: &ToolContext) -> ToolOutput {
        t.call(args, c).await.unwrap()
    }

    #[tokio::test]
    async fn duckduckgo_end_to_end_against_a_mock() {
        let (port, seen) = mock(200, "text/html", DDG_FIXTURE.to_string()).await;
        let t = tool(&format!(
            "[search.duckduckgo]\nurl = \"http://127.0.0.1:{port}/html/\""
        ));
        let (_d, c) = cx();
        let out = search(
            &t,
            json!({"query": "rust tokio", "max_results": 2, "site": "https://www.Docs.rs/x"}),
            &c,
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("1. Tokio - An asynchronous Rust runtime\n   https://tokio.rs/\n"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("2. ") && !out.content.contains("3. "),
            "{}",
            out.content
        );
        let s = seen.lock().unwrap();
        assert_eq!(s.len(), 1, "one request per search");
        assert!(
            s[0].head
                .starts_with("GET /html/?q=rust+tokio+site%3Adocs.rs "),
            "{}",
            s[0].head
        );
        assert!(
            s[0].head
                .to_ascii_lowercase()
                .contains("user-agent: ruckcode/"),
            "{}",
            s[0].head
        );
    }

    #[tokio::test]
    async fn duckduckgo_blocked_pages_say_unavailable_and_do_not_retry() {
        for (status, body) in [
            (202u16, DDG_ANOMALY.to_string()),
            (429, String::new()),
            (200, DDG_ANOMALY.to_string()),
        ] {
            let (port, seen) = mock(status, "text/html", body).await;
            let t = tool(&format!(
                "[search.duckduckgo]\nurl = \"http://127.0.0.1:{port}/html/\""
            ));
            let (_d, c) = cx();
            let out = search(&t, json!({"query": "q"}), &c).await;
            assert!(
                out.is_error && out.content.contains("temporarily unavailable"),
                "{status}: {}",
                out.content
            );
            assert_eq!(seen.lock().unwrap().len(), 1, "never retried");
        }
    }

    #[tokio::test]
    async fn searxng_requests_json_and_parses_it() {
        let body = r#"{"results":[{"title":"One","url":"https://one.example/","content":"first"},{"title":"Two","url":"https://two.example/","content":"second"}]}"#;
        let (port, seen) = mock(200, "application/json", body.into()).await;
        let t = tool(&format!(
            "[search]\nprovider = \"searxng\"\nurl = \"http://127.0.0.1:{port}\""
        ));
        let (_d, c) = cx();
        let out = search(&t, json!({"query": "hello world"}), &c).await;
        assert!(
            !out.is_error
                && out
                    .content
                    .contains("1. One\n   https://one.example/\n   first"),
            "{}",
            out.content
        );
        {
            let s = seen.lock().unwrap();
            assert!(
                s[0].head
                    .starts_with("GET /search?q=hello+world&format=json "),
                "{}",
                s[0].head
            );
        }
        // A 403 means the JSON format is disabled.
        let (port, _) = mock(403, "text/html", "forbidden".into()).await;
        let t = tool(&format!(
            "[search]\nprovider = \"searxng\"\nurl = \"http://127.0.0.1:{port}\""
        ));
        let out = search(&t, json!({"query": "x"}), &c).await;
        assert!(
            out.is_error && out.content.contains("json format"),
            "{}",
            out.content
        );
        // And the URL is required.
        let out = search(
            &tool("[search]\nprovider = \"searxng\""),
            json!({"query": "x"}),
            &c,
        )
        .await;
        assert!(
            out.is_error && out.content.contains("needs `web.search.url`"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn brave_request_shape_and_parsing() {
        let body = r#"{"web":{"results":[{"title":"B1","url":"https://b1.example/","description":"desc one"}]}}"#;
        let (port, seen) = mock(200, "application/json", body.into()).await;
        let t = tool(&format!("[search]\nprovider = \"brave\"\napi_key = \"k-123\"\nurl = \"http://127.0.0.1:{port}/res/v1/web/search\""));
        let (_d, c) = cx();
        let out = search(
            &t,
            json!({"query": "rust", "max_results": 5, "site": "docs.rs"}),
            &c,
        )
        .await;
        assert!(
            !out.is_error
                && out
                    .content
                    .contains("1. B1\n   https://b1.example/\n   desc one"),
            "{}",
            out.content
        );
        let h = seen.lock().unwrap()[0].head.clone();
        assert!(
            h.starts_with("GET /res/v1/web/search?q=rust+site%3Adocs.rs&count=5 "),
            "{h}"
        );
        assert!(
            h.to_ascii_lowercase()
                .contains("x-subscription-token: k-123"),
            "{h}"
        );
        assert!(!out.content.contains("k-123"));
    }

    #[tokio::test]
    async fn tavily_request_shape_and_parsing() {
        let body = r#"{"results":[{"title":"T1","url":"https://t1.example/","content":"about t1","score":0.8}]}"#;
        let (port, seen) = mock(200, "application/json", body.into()).await;
        let t = tool(&format!("[search.tavily]\nurl = \"http://127.0.0.1:{port}/search\"\napi_key = \"tv-9\"\n[search]\nprovider = \"tavily\""))
            .with_env(|_| None);
        let (_d, c) = cx();
        let out = search(
            &t,
            json!({"query": "rust", "max_results": 3, "site": "docs.rs"}),
            &c,
        )
        .await;
        assert!(
            !out.is_error
                && out
                    .content
                    .contains("1. T1\n   https://t1.example/\n   about t1"),
            "{}",
            out.content
        );
        let s = seen.lock().unwrap();
        assert!(s[0].head.starts_with("POST /search "), "{}", s[0].head);
        assert!(
            s[0].head
                .to_ascii_lowercase()
                .contains("authorization: bearer tv-9"),
            "{}",
            s[0].head
        );
        let v: Value = serde_json::from_str(&s[0].body).unwrap();
        assert_eq!(v["query"], "rust");
        assert_eq!(v["max_results"], 3);
        assert_eq!(v["include_domains"], json!(["docs.rs"]));
    }

    #[tokio::test]
    async fn a_missing_key_is_a_clear_error_and_env_vars_are_honoured() {
        let (_d, c) = cx();
        for (p, var) in [("brave", "BRAVE_API_KEY"), ("tavily", "TAVILY_API_KEY")] {
            let t = tool(&format!("[search]\nprovider = \"{p}\""));
            let out = search(&t, json!({"query": "x"}), &c).await;
            assert!(
                out.is_error
                    && out.content.contains("needs an API key")
                    && out.content.contains(var),
                "{}",
                out.content
            );
        }
        let (port, seen) = mock(200, "application/json", r#"{"web":{"results":[]}}"#.into()).await;
        let t = tool(&format!(
            "[search]\nprovider = \"brave\"\nurl = \"http://127.0.0.1:{port}/\""
        ))
        .with_env(|n| (n == "BRAVE_API_KEY").then(|| "from-env".to_string()));
        let out = search(&t, json!({"query": "x"}), &c).await;
        assert!(
            !out.is_error && out.content.starts_with("No results"),
            "{}",
            out.content
        );
        assert!(seen.lock().unwrap()[0]
            .head
            .to_ascii_lowercase()
            .contains("x-subscription-token: from-env"));
    }

    #[tokio::test]
    async fn no_silent_fallback_unless_configured() {
        let (bad, _) = mock(500, "text/plain", "boom".into()).await;
        let good_body = r#"{"results":[{"title":"G","url":"https://g.example/","content":"ok"}]}"#;
        let (good, _) = mock(200, "application/json", good_body.into()).await;
        let (_d, c) = cx();
        let base = format!("[search]\nprovider = \"searxng\"\nurl = \"http://127.0.0.1:{bad}\"\n[search.tavily]\nurl = \"http://127.0.0.1:{good}/s\"\napi_key = \"k\"\n");
        let out = search(&tool(&base), json!({"query": "x"}), &c).await;
        assert!(
            out.is_error
                && out.content.contains("search failed (searxng)")
                && !out.content.contains("tavily"),
            "{}",
            out.content
        );
        let with_fb = base.replace("[search]\n", "[search]\nfallback = [\"tavily\"]\n");
        let out = search(&tool(&with_fb), json!({"query": "x"}), &c).await;
        assert!(
            !out.is_error && out.content.contains("(tavily)") && out.content.contains("1. G"),
            "{}",
            out.content
        );
        let out = search(
            &tool("[search]\nprovider = \"bing\""),
            json!({"query": "x"}),
            &c,
        )
        .await;
        assert!(
            out.is_error && out.content.contains("unknown search provider"),
            "{}",
            out.content
        );
    }

    fn gated(net: NetworkMode, gate: Arc<Gate>) -> (tempfile::TempDir, ToolContext) {
        let (d, mut c) = cx();
        c.sandbox = Some(SandboxHandle {
            sandbox: None,
            cfg: SandboxConfig {
                network: net,
                allowed_domains: vec![],
                ..SandboxConfig::default()
            },
            protected_write: vec![],
            deny_read: vec![],
            gate: Some(gate),
        });
        (d, c)
    }

    #[tokio::test]
    async fn the_provider_host_is_preallowed_but_network_off_still_refuses() {
        let body = r#"{"results":[{"title":"A","url":"https://a.example/","content":"x"}]}"#;
        let (port, seen) = mock(200, "application/json", body.into()).await;
        let t = tool(&format!(
            "[search]\nprovider = \"searxng\"\nurl = \"http://127.0.0.1:{port}\""
        ));
        // No prompter and an empty allowlist: only the pre-allow lets this through.
        let (_d, c) = gated(NetworkMode::Allowlist, Arc::new(Gate::new(None, None)));
        let out = search(&t, json!({"query": "x"}), &c).await;
        assert!(!out.is_error, "{}", out.content);
        let (_d, off) = gated(NetworkMode::Off, Arc::new(Gate::new(None, None)));
        let before = seen.lock().unwrap().len();
        let out = search(&t, json!({"query": "x"}), &off).await;
        assert!(
            out.is_error && out.content.contains("network access is disabled"),
            "{}",
            out.content
        );
        assert_eq!(seen.lock().unwrap().len(), before);
    }

    #[test]
    fn site_filter_is_cleaned() {
        assert_eq!(
            clean_site("https://www.docs.rs/tokio").as_deref(),
            Some("docs.rs")
        );
        assert_eq!(clean_site("Example.COM").as_deref(), Some("example.com"));
        assert_eq!(clean_site("a b"), None);
        assert_eq!(clean_site("x;y"), None);
    }
}
