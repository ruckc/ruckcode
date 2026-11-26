//! The `WebFetch` tool.

use super::net::{self, NetPolicy};
use super::{html, truncate_chars, WebConfig, WEB_FETCH};
use crate::{str_arg, Tool, ToolContext, ToolKind, ToolOutput};
use async_trait::async_trait;
use reqwest::Url;
use serde_json::{json, Value};
use std::time::Duration;

pub const MAX_REDIRECTS: usize = 5;
pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
pub const DEFAULT_MAX_CHARS: usize = 20_000;
/// Upper bound on `max_chars` (the per-result cap of `[context]` still applies on top).
pub const HARD_MAX_CHARS: usize = 200_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct WebFetch {
    policy: NetPolicy,
    max_body: usize,
}

impl WebFetch {
    pub fn new(cfg: &WebConfig) -> Self {
        Self::with_policy(NetPolicy::new(cfg.allow_private))
    }

    pub fn with_policy(policy: NetPolicy) -> Self {
        Self {
            policy,
            max_body: MAX_BODY_BYTES,
        }
    }

    #[cfg(test)]
    fn with_max_body(mut self, n: usize) -> Self {
        self.max_body = n;
        self
    }
}

impl Default for WebFetch {
    fn default() -> Self {
        Self::new(&WebConfig::default())
    }
}

/// The URLs to try for `u`, best first: `http://host/...` is upgraded to https (only when no
/// explicit port was given; the original is kept as a fallback when https cannot connect).
fn candidates(u: &Url) -> Vec<Url> {
    if u.scheme() == "http" && u.port().is_none() {
        let mut s = u.clone();
        if s.set_scheme("https").is_ok() {
            return vec![s, u.clone()];
        }
    }
    vec![u.clone()]
}

fn err_chain(e: &reqwest::Error) -> String {
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

#[derive(Debug, PartialEq)]
enum Kind {
    Html,
    Text,
    Binary,
}

fn mime_of(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn classify(content_type: Option<&str>, body: &[u8]) -> (Kind, String) {
    let mime = content_type.map(mime_of).unwrap_or_default();
    let head = &body[..body.len().min(2048)];
    let sniff_html = || {
        let t = String::from_utf8_lossy(head)
            .trim_start()
            .to_ascii_lowercase();
        t.starts_with("<!doctype html") || t.starts_with("<html")
    };
    let kind = match mime.as_str() {
        "text/html" | "application/xhtml+xml" => Kind::Html,
        m if m.starts_with("text/")
            || m == "application/json"
            || m == "application/xml"
            || m == "application/javascript"
            || m == "application/x-ndjson"
            || m == "application/yaml"
            || m == "application/x-yaml"
            || m == "application/toml"
            || m.ends_with("+json")
            || m.ends_with("+xml") =>
        {
            Kind::Text
        }
        "" | "application/octet-stream" => {
            if head.contains(&0) {
                Kind::Binary
            } else if sniff_html() {
                Kind::Html
            } else {
                Kind::Text
            }
        }
        _ => Kind::Binary,
    };
    (kind, mime)
}

/// Decode a body: a BOM wins, then the `Content-Type` charset, then (for HTML) a `<meta
/// charset>` / `http-equiv` declaration in the first 1 KiB, else UTF-8. Invalid sequences
/// become U+FFFD.
fn decode(content_type: Option<&str>, body: &[u8], html: bool) -> String {
    use encoding_rs::Encoding;
    let enc = Encoding::for_bom(body)
        .map(|(e, _)| e)
        .or_else(|| {
            content_type
                .and_then(|c| {
                    c.split(';').skip(1).find_map(|p| {
                        let p = p.trim();
                        p.get(..8)
                            .filter(|k| k.eq_ignore_ascii_case("charset="))
                            .map(|_| p[8..].trim_matches('"').trim())
                    })
                })
                .and_then(|l| Encoding::for_label(l.as_bytes()))
        })
        .or_else(|| {
            if !html {
                return None;
            }
            static RE: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
            let re = RE.get_or_init(|| {
                regex::bytes::Regex::new(r#"(?i)charset\s*=\s*["']?\s*([A-Za-z0-9_:.\-]+)"#)
                    .unwrap()
            });
            re.captures(&body[..body.len().min(1024)])
                .and_then(|c| Encoding::for_label(&c[1]))
        })
        .unwrap_or(encoding_rs::UTF_8);
    enc.decode(body).0.into_owned()
}

impl WebFetch {
    /// Validate, gate and send one hop; returns the response and the URL actually requested.
    async fn hop(
        &self,
        client: &reqwest::Client,
        url: &Url,
        ctx: &ToolContext,
    ) -> Result<(reqwest::Response, Url), String> {
        let tries = candidates(url);
        let last = tries.len() - 1;
        for (i, u) in tries.into_iter().enumerate() {
            net::check_host(&u, &self.policy)?;
            if let Some(host) = u.host_str() {
                net::check_network(ctx, host, false).await?;
            }
            let req = client
                .get(u.clone())
                .header(
                    reqwest::header::ACCEPT,
                    "text/html,application/xhtml+xml,text/plain,application/json;q=0.9,*/*;q=0.1",
                )
                .send();
            let sent = tokio::select! {
                r = req => r,
                _ = ctx.cancel.cancelled() => return Err("cancelled".into()),
            };
            match sent {
                Ok(r) => return Ok((r, u)),
                Err(e) if e.is_connect() && i < last => continue,
                Err(e) => return Err(format!("request to {u} failed: {}", err_chain(&e))),
            }
        }
        unreachable!("candidates is never empty")
    }

    async fn run(&self, raw: &str, max_chars: usize, ctx: &ToolContext) -> ToolOutput {
        let mut current = match Url::parse(raw.trim()) {
            Ok(u) => u,
            Err(e) => return ToolOutput::error(format!("invalid URL `{raw}`: {e}")),
        };
        let client = match net::client(&self.policy, CONNECT_TIMEOUT, TOTAL_TIMEOUT) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(e),
        };
        let mut hops = 0;
        let (mut resp, used) = loop {
            let (resp, used) = match self.hop(&client, &current, ctx).await {
                Ok(r) => r,
                Err(e) => return ToolOutput::error(e),
            };
            if resp.status().is_redirection() {
                if let Some(loc) = resp.headers().get(reqwest::header::LOCATION) {
                    hops += 1;
                    if hops > MAX_REDIRECTS {
                        return ToolOutput::error(format!(
                            "too many redirects (more than {MAX_REDIRECTS}) fetching {raw}"
                        ));
                    }
                    let Some(next) = loc.to_str().ok().and_then(|l| used.join(l).ok()) else {
                        return ToolOutput::error(format!(
                            "{used} redirected to an invalid Location"
                        ));
                    };
                    current = next;
                    continue;
                }
            }
            break (resp, used);
        };
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut body: Vec<u8> = Vec::new();
        let mut capped = false;
        loop {
            let chunk = tokio::select! {
                c = resp.chunk() => c,
                _ = ctx.cancel.cancelled() => return ToolOutput::error("cancelled"),
            };
            match chunk {
                Ok(Some(c)) => {
                    let room = self.max_body - body.len();
                    if c.len() >= room {
                        body.extend_from_slice(&c[..room]);
                        capped = true;
                        break;
                    }
                    body.extend_from_slice(&c);
                }
                Ok(None) => break,
                Err(e) => {
                    return ToolOutput::error(format!(
                        "reading the response from {used} failed: {}",
                        err_chain(&e)
                    ))
                }
            }
        }
        let (kind, mime) = classify(content_type.as_deref(), &body);
        let header = format!(
            "URL: {used}\nStatus: {status}\nContent-Type: {}",
            content_type.as_deref().unwrap_or("(none)")
        );
        if kind == Kind::Binary {
            return ToolOutput::error(format!(
                "{header}\n\nrefused: binary content type `{}` ({}{} bytes) cannot be shown as text. WebFetch only reads HTML, plain text, JSON, XML and Markdown.",
                if mime.is_empty() { "unknown" } else { &mime },
                if capped { "at least " } else { "" },
                body.len()
            ));
        }
        let text = decode(content_type.as_deref(), &body, kind == Kind::Html);
        let text = if kind == Kind::Html {
            match html::to_markdown(&text) {
                Ok(t) => t,
                Err(e) => return ToolOutput::error(format!("{header}\n\n{e}")),
            }
        } else {
            text
        };
        let (shown, total) = truncate_chars(&text, max_chars);
        let mut out = format!("{header}\n\n{shown}");
        if let Some(total) = total {
            out.push_str(&format!(
                "\n\n[truncated: showing {max_chars} of {total} characters; pass a larger max_chars (up to {HARD_MAX_CHARS}) to read more]"
            ));
        }
        if capped {
            out.push_str(&format!(
                "\n\n[the response body was larger than {} MB; only the first {} MB were read]",
                self.max_body / (1024 * 1024),
                self.max_body / (1024 * 1024)
            ));
        }
        if status.is_client_error() || status.is_server_error() {
            ToolOutput::error(out)
        } else {
            ToolOutput::ok(out)
        }
    }
}

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        WEB_FETCH
    }

    fn description(&self) -> String {
        "Fetch a web page and return it as readable text. HTML is converted to Markdown \
         (headings, links, lists, code); text, JSON and Markdown are returned as-is; binary \
         content is refused. http URLs are upgraded to https. Private, loopback and metadata \
         addresses are refused, and the sandbox's network policy applies (unknown domains ask \
         the user). The result starts with the final URL, status and content type."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The http(s) URL to fetch."},
                "max_chars": {
                    "type": "integer",
                    "description": format!(
                        "Maximum characters of content to return (default {DEFAULT_MAX_CHARS}, max {HARD_MAX_CHARS})."
                    )
                }
            },
            "required": ["url"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Fetch
    }

    fn title(&self, input: &Value) -> String {
        match input.get("url").and_then(Value::as_str) {
            Some(u) => format!("Fetch {u}"),
            None => "WebFetch".into(),
        }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let url = str_arg(&input, "url")?;
        let max_chars = input
            .get("max_chars")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_CHARS, |n| (n as usize).clamp(1, HARD_MAX_CHARS));
        Ok(self.run(url, max_chars, ctx).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ctx;
    use crate::web::net::{BlockedFn, ResolveHook};
    use crate::SandboxHandle;
    use async_trait::async_trait;
    use ruckcode_sandbox::proxy::{DomainAnswer, DomainPrompter, DomainStore, Gate};
    use ruckcode_sandbox::{NetworkMode, SandboxConfig};
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One canned response.
    #[derive(Clone)]
    struct Resp {
        status: &'static str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    }

    fn ok(ct: &'static str, body: impl Into<Vec<u8>>) -> Resp {
        Resp {
            status: "200 OK",
            headers: vec![("Content-Type", ct.into())],
            body: body.into(),
        }
    }

    fn redirect(to: &str) -> Resp {
        Resp {
            status: "302 Found",
            headers: vec![("Location", to.into())],
            body: vec![],
        }
    }

    /// A mock HTTP server on 127.0.0.1; returns its port and a request counter.
    async fn serve(routes: Vec<(&'static str, Resp)>) -> (u16, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    break;
                };
                let routes = routes.clone();
                let h = h.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = c.read(&mut buf).await.unwrap_or(0);
                    h.fetch_add(1, Ordering::SeqCst);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let r = routes
                        .iter()
                        .find(|(p, _)| *p == path)
                        .map(|(_, r)| r.clone())
                        .unwrap_or(Resp {
                            status: "404 Not Found",
                            headers: vec![("Content-Type", "text/plain".into())],
                            body: b"nope".to_vec(),
                        });
                    let mut head = format!("HTTP/1.1 {}\r\n", r.status);
                    for (k, v) in &r.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n",
                        r.body.len()
                    ));
                    let _ = c.write_all(head.as_bytes()).await;
                    let _ = c.write_all(&r.body).await;
                    let _ = c.shutdown().await;
                });
            }
        });
        (port, hits)
    }

    fn open() -> WebFetch {
        WebFetch::with_policy(NetPolicy::new(true))
    }

    async fn fetch(t: &WebFetch, url: &str, ctx: &ToolContext) -> ToolOutput {
        t.call(json!({"url": url}), ctx).await.unwrap()
    }

    fn dir_ctx() -> (tempfile::TempDir, ToolContext) {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        (d, c)
    }

    #[tokio::test]
    async fn converts_html_and_prefixes_url_status_and_type() {
        let (port, _) = serve(vec![(
            "/p",
            ok(
                "text/html; charset=utf-8",
                "<html><head><title>T</title></head><body><nav>menu</nav><h1>Hello</h1><p>See <a href=\"/x\">this</a></p><script>bad()</script></body></html>",
            ),
        )])
        .await;
        let (_d, c) = dir_ctx();
        let out = fetch(&open(), &format!("http://127.0.0.1:{port}/p"), &c).await;
        assert!(!out.is_error, "{}", out.content);
        let s = &out.content;
        assert!(s.starts_with(&format!("URL: http://127.0.0.1:{port}/p\nStatus: 200 OK\nContent-Type: text/html; charset=utf-8\n\n")), "{s}");
        assert!(s.contains("# Hello") && s.contains("[this](/x)"), "{s}");
        assert!(!s.contains("menu") && !s.contains("bad()"), "{s}");
    }

    #[tokio::test]
    async fn text_and_json_pass_through() {
        let (port, _) = serve(vec![
            ("/j", ok("application/json", "{\"a\": [1, 2]}")),
            ("/m", ok("text/markdown", "# Title\n\n- item\n")),
        ])
        .await;
        let (_d, c) = dir_ctx();
        let j = fetch(&open(), &format!("http://127.0.0.1:{port}/j"), &c).await;
        assert!(j.content.ends_with("\n\n{\"a\": [1, 2]}"), "{}", j.content);
        let m = fetch(&open(), &format!("http://127.0.0.1:{port}/m"), &c).await;
        assert!(
            m.content.ends_with("\n\n# Title\n\n- item\n"),
            "{}",
            m.content
        );
    }

    #[tokio::test]
    async fn binary_is_refused() {
        let (port, _) = serve(vec![
            ("/i", ok("image/png", vec![0x89, b'P', b'N', b'G', 0, 1, 2])),
            ("/o", ok("application/octet-stream", vec![1u8, 0, 2, 3])),
        ])
        .await;
        let (_d, c) = dir_ctx();
        for p in ["/i", "/o"] {
            let out = fetch(&open(), &format!("http://127.0.0.1:{port}{p}"), &c).await;
            assert!(
                out.is_error && out.content.contains("refused: binary"),
                "{}",
                out.content
            );
        }
    }

    #[tokio::test]
    async fn truncates_to_max_chars_and_caps_the_body() {
        let big = "é".repeat(5_000);
        let (port, _) = serve(vec![("/b", ok("text/plain", big.clone()))]).await;
        let (_d, c) = dir_ctx();
        let out = open()
            .call(
                json!({"url": format!("http://127.0.0.1:{port}/b"), "max_chars": 100}),
                &c,
            )
            .await
            .unwrap();
        assert!(out.content.contains(&"é".repeat(100)) && !out.content.contains(&"é".repeat(101)));
        assert!(
            out.content
                .contains("truncated: showing 100 of 5000 characters"),
            "{}",
            out.content
        );
        // The body cap: only the first 1000 bytes are read.
        let out = open()
            .with_max_body(1000)
            .call(json!({"url": format!("http://127.0.0.1:{port}/b")}), &c)
            .await
            .unwrap();
        assert!(out.content.contains("only the first"), "{}", out.content);
        let shown = out.content.matches('é').count();
        assert_eq!(shown, 500);
    }

    #[tokio::test]
    async fn follows_redirects_up_to_the_limit() {
        let mut routes = vec![("/end", ok("text/plain", "arrived"))];
        // /r0 -> /r1 -> ... -> /r5 -> /end : exactly 6 redirects in a chain starting at r0.
        for (i, p) in ["/r0", "/r1", "/r2", "/r3", "/r4", "/r5"]
            .iter()
            .enumerate()
        {
            let next: &'static str = ["/r1", "/r2", "/r3", "/r4", "/r5", "/end"][i];
            routes.push((*p, redirect(next)));
        }
        let (port, _) = serve(routes).await;
        let (_d, c) = dir_ctx();
        // 5 redirects (r1 -> ... -> end) are fine.
        let out = fetch(&open(), &format!("http://127.0.0.1:{port}/r1"), &c).await;
        assert!(
            !out.is_error && out.content.contains("arrived"),
            "{}",
            out.content
        );
        assert!(
            out.content
                .starts_with(&format!("URL: http://127.0.0.1:{port}/end")),
            "{}",
            out.content
        );
        // 6 are not.
        let out = fetch(&open(), &format!("http://127.0.0.1:{port}/r0"), &c).await;
        assert!(
            out.is_error && out.content.contains("too many redirects"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn redirect_to_a_private_address_is_refused() {
        // Loopback is the mock server's own address, so only the metadata range is blocked here;
        // the policy function itself is covered exhaustively in net.rs.
        let (port, _) = serve(vec![(
            "/meta",
            redirect("http://169.254.169.254/latest/meta-data/"),
        )])
        .await;
        let blocked: BlockedFn =
            Arc::new(|ip: IpAddr| crate::web::net::is_blocked_ip(ip) && !ip.is_loopback());
        let mut p = NetPolicy::new(false);
        p.blocked = Some(blocked);
        let (_d, c) = dir_ctx();
        let out = fetch(
            &WebFetch::with_policy(p),
            &format!("http://127.0.0.1:{port}/meta"),
            &c,
        )
        .await;
        assert!(
            out.is_error && out.content.contains("169.254.169.254"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn private_literals_and_rebinding_dns_are_refused() {
        let (port, hits) = serve(vec![("/", ok("text/plain", "secret"))]).await;
        let strict = WebFetch::new(&WebConfig::default());
        let (_d, c) = dir_ctx();
        for u in [
            format!("http://127.0.0.1:{port}/"),
            format!("http://[::1]:{port}/"),
            format!("http://[::ffff:127.0.0.1]:{port}/"),
            "http://169.254.169.254/latest/meta-data/".to_string(),
            "http://10.1.2.3/".to_string(),
            "http://192.168.0.1/".to_string(),
            "http://172.16.5.5/".to_string(),
            "http://0.0.0.0/".to_string(),
            "http://[fc00::1]/".to_string(),
            "http://[fe80::1]/".to_string(),
            "file:///etc/passwd".to_string(),
            "ftp://example.com/".to_string(),
        ] {
            let out = fetch(&strict, &u, &c).await;
            assert!(out.is_error, "{u}: {}", out.content);
        }
        // A name that resolves (DNS rebinding style) to loopback is refused at connect time and
        // the server is never contacted.
        let hook: ResolveHook = Arc::new(|h: &str| {
            (h == "rebind.example").then(|| vec!["127.0.0.1".parse::<IpAddr>().unwrap()])
        });
        let mut p = NetPolicy::new(false);
        p.resolve_hook = Some(hook.clone());
        let out = fetch(
            &WebFetch::with_policy(p),
            &format!("http://rebind.example:{port}/"),
            &c,
        )
        .await;
        assert!(
            out.is_error && out.content.contains("resolves to a private"),
            "{}",
            out.content
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "the server must not be contacted"
        );
        // With allow_private the same name connects: the resolved (hooked) address is used.
        let mut p = NetPolicy::new(true);
        p.resolve_hook = Some(hook);
        let out = fetch(
            &WebFetch::with_policy(p),
            &format!("http://rebind.example:{port}/"),
            &c,
        )
        .await;
        assert!(
            !out.is_error && out.content.contains("secret"),
            "{}",
            out.content
        );
    }

    #[test]
    fn decodes_by_declared_charset() {
        // windows-1252: 0x80 is the euro sign, 0x93/0x94 are curly quotes (not C1 controls).
        let w = [0x80u8, b' ', 0x93, b'h', b'i', 0x94];
        assert_eq!(
            decode(Some("text/plain; charset=windows-1252"), &w, false),
            "\u{20ac} \u{201c}hi\u{201d}"
        );
        // ISO-8859-1 is windows-1252 per the WHATWG label table; 0xE9 is e-acute.
        assert_eq!(
            decode(
                Some("text/plain; charset=ISO-8859-1"),
                &[b'c', b'a', b'f', 0xe9],
                false
            ),
            "caf\u{e9}"
        );
        // An unknown label falls back to UTF-8.
        assert_eq!(
            decode(
                Some("text/plain; charset=bogus-9"),
                "caf\u{e9}".as_bytes(),
                false
            ),
            "caf\u{e9}"
        );
        assert_eq!(decode(None, b"plain", false), "plain");
        // Meta sniff for HTML without a header charset; a BOM beats the header.
        let html = b"<meta charset=\"windows-1252\"><p>\x93q\x94";
        assert_eq!(
            decode(Some("text/html"), html, true),
            "<meta charset=\"windows-1252\"><p>\u{201c}q\u{201d}"
        );
        let bom = [0xefu8, 0xbb, 0xbf, b'o', b'k'];
        assert_eq!(
            decode(Some("text/plain; charset=windows-1252"), &bom, false),
            "ok"
        );
    }

    #[test]
    fn http_is_upgraded_to_https_when_it_has_no_explicit_port() {
        let c = |u: &str| {
            candidates(&u.parse().unwrap())
                .iter()
                .map(Url::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            c("http://example.com/a?b=1"),
            ["https://example.com/a?b=1", "http://example.com/a?b=1"]
        );
        assert_eq!(
            c("http://example.com:8080/a"),
            ["http://example.com:8080/a"]
        );
        assert_eq!(c("https://example.com/a"), ["https://example.com/a"]);
    }

    struct Fixed(DomainAnswer, Arc<AtomicUsize>);
    #[async_trait]
    impl DomainPrompter for Fixed {
        async fn ask(&self, _: &str, _: &str) -> DomainAnswer {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0
        }
    }

    fn gated(
        net: NetworkMode,
        gate: Option<Arc<Gate>>,
        allowed: &[&str],
    ) -> (tempfile::TempDir, ToolContext) {
        let (d, mut c) = dir_ctx();
        let cfg = SandboxConfig {
            network: net,
            allowed_domains: allowed.iter().map(|s| s.to_string()).collect(),
            ..SandboxConfig::default()
        };
        c.sandbox = Some(SandboxHandle {
            sandbox: None,
            cfg,
            protected_write: vec![],
            deny_read: vec![],
            gate,
        });
        (d, c)
    }

    #[tokio::test]
    async fn unknown_domains_prompt_deny_refuses_and_always_is_remembered() {
        let (port, hits) = serve(vec![("/", ok("text/plain", "hello"))]).await;
        let url = format!("http://127.0.0.1:{port}/");
        // Deny: refused, never contacted, and not asked twice.
        let asks = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::new(
            None,
            Some(Arc::new(Fixed(DomainAnswer::Deny, asks.clone()))),
        ));
        let (_d, c) = gated(NetworkMode::Allowlist, Some(gate), &[]);
        let out = fetch(&open(), &url, &c).await;
        assert!(
            out.is_error && out.content.contains("not allowed by the sandbox policy"),
            "{}",
            out.content
        );
        fetch(&open(), &url, &c).await;
        assert_eq!(
            (asks.load(Ordering::SeqCst), hits.load(Ordering::SeqCst)),
            (1, 0)
        );
        // Allow always: fetched, stored for the project, and a fresh gate reads it back.
        let store_dir = tempfile::tempdir().unwrap();
        let store = DomainStore::at(store_dir.path().join("d.json"));
        let asks = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::new(
            Some(store.clone()),
            Some(Arc::new(Fixed(DomainAnswer::Always, asks.clone()))),
        ));
        let (_d, c) = gated(NetworkMode::Allowlist, Some(gate), &[]);
        let out = fetch(&open(), &url, &c).await;
        assert!(
            !out.is_error && out.content.contains("hello"),
            "{}",
            out.content
        );
        assert_eq!(store.load(), ["127.0.0.1"]);
        let gate2 = Arc::new(Gate::new(Some(store), None));
        let (_d, c2) = gated(NetworkMode::Allowlist, Some(gate2), &[]);
        let out = fetch(&open(), &url, &c2).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(asks.load(Ordering::SeqCst), 1);
        // An allowlisted domain needs no prompt at all.
        let gate = Arc::new(Gate::new(None, None));
        let (_d, c3) = gated(NetworkMode::Allowlist, Some(gate), &["127.0.0.1"]);
        assert!(!fetch(&open(), &url, &c3).await.is_error);
    }

    #[tokio::test]
    async fn network_off_refuses_and_full_skips_the_prompt() {
        let (port, hits) = serve(vec![("/", ok("text/plain", "hello"))]).await;
        let url = format!("http://127.0.0.1:{port}/");
        let asks = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::new(
            None,
            Some(Arc::new(Fixed(DomainAnswer::Once, asks.clone()))),
        ));
        let (_d, off) = gated(NetworkMode::Off, Some(gate.clone()), &["127.0.0.1"]);
        let out = fetch(&open(), &url, &off).await;
        assert!(
            out.is_error && out.content.contains("network access is disabled"),
            "{}",
            out.content
        );
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        let (_d, full) = gated(NetworkMode::Full, Some(gate), &[]);
        let out = fetch(&open(), &url, &full).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(asks.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn http_errors_are_reported_as_errors_with_the_body() {
        let (port, _) = serve(vec![]).await;
        let (_d, c) = dir_ctx();
        let out = fetch(&open(), &format!("http://127.0.0.1:{port}/missing"), &c).await;
        assert!(
            out.is_error && out.content.contains("Status: 404") && out.content.contains("nope")
        );
    }
}
