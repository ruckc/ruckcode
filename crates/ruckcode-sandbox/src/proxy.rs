//! The in-process HTTP / HTTPS (CONNECT) proxy that enforces the domain allowlist. Sandboxed
//! commands can only reach the network through it (Landlock TCP rules, or a network namespace
//! whose only exit is a forwarder to this proxy's unix socket).

use crate::config::domain_matches;
use async_trait::async_trait;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainAnswer {
    Once,
    /// Remember for this project.
    Always,
    Deny,
}

/// Asked when a command reaches for a domain that is not allowlisted.
#[async_trait]
pub trait DomainPrompter: Send + Sync {
    async fn ask(&self, host: &str, tool_call_id: &str) -> DomainAnswer;
}

/// Per-project remembered domains, kept in the user's data dir (outside anything a sandboxed
/// command can write).
#[derive(Debug, Clone)]
pub struct DomainStore {
    path: PathBuf,
}

impl DomainStore {
    pub fn for_project(project: &Path) -> Option<Self> {
        let canon = project
            .canonicalize()
            .unwrap_or_else(|_| project.to_path_buf());
        let mut h: u64 = 0xcbf29ce484222325;
        for b in canon.to_string_lossy().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        let dir = dirs::data_dir()?.join("ruckcode").join("network");
        Some(Self {
            path: dir.join(format!("{h:016x}.json")),
        })
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Vec<String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default()
    }

    pub fn add(&self, host: &str) {
        let mut v = self.load();
        if !v.iter().any(|h| h == host) {
            v.push(host.to_string());
            if let Some(p) = self.path.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(
                &self.path,
                serde_json::to_vec_pretty(&v).unwrap_or_default(),
            );
        }
    }
}

/// The decision point: allowlist, remembered domains, then (maybe) the user.
pub struct Gate {
    allowed: RwLock<Vec<String>>,
    session_allowed: Mutex<HashSet<String>>,
    session_denied: Mutex<HashSet<String>>,
    store: Option<DomainStore>,
    prompter: Option<Arc<dyn DomainPrompter>>,
    prompt_lock: tokio::sync::Mutex<()>,
    current_call: Mutex<String>,
    denials: Mutex<Vec<String>>,
}

impl Gate {
    pub fn new(store: Option<DomainStore>, prompter: Option<Arc<dyn DomainPrompter>>) -> Self {
        Self {
            allowed: RwLock::new(vec![]),
            session_allowed: Default::default(),
            session_denied: Default::default(),
            store,
            prompter,
            prompt_lock: tokio::sync::Mutex::new(()),
            current_call: Mutex::new(String::new()),
            denials: Mutex::new(vec![]),
        }
    }

    pub fn set_allowed(&self, domains: Vec<String>) {
        *self.allowed.write().unwrap() = domains;
    }

    pub fn set_current_call(&self, id: &str) {
        *self.current_call.lock().unwrap() = id.to_string();
    }

    /// Hosts refused since the last call to this (for the tool result).
    pub fn take_denials(&self) -> Vec<String> {
        std::mem::take(&mut *self.denials.lock().unwrap())
    }

    fn static_allowed(&self, host: &str) -> bool {
        self.allowed
            .read()
            .unwrap()
            .iter()
            .any(|p| domain_matches(p, host))
            || self.session_allowed.lock().unwrap().contains(host)
            || self
                .store
                .as_ref()
                .is_some_and(|s| s.load().iter().any(|p| domain_matches(p, host)))
    }

    pub async fn decide(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if self.static_allowed(&host) {
            return true;
        }
        if self.session_denied.lock().unwrap().contains(&host) {
            self.note_denial(&host);
            return false;
        }
        if let Some(p) = &self.prompter {
            // One prompt at a time; a concurrent request for the same host reuses the answer.
            let _g = self.prompt_lock.lock().await;
            if self.static_allowed(&host) {
                return true;
            }
            if self.session_denied.lock().unwrap().contains(&host) {
                self.note_denial(&host);
                return false;
            }
            let call = self.current_call.lock().unwrap().clone();
            match p.ask(&host, &call).await {
                DomainAnswer::Once => {
                    self.session_allowed.lock().unwrap().insert(host);
                    return true;
                }
                DomainAnswer::Always => {
                    if let Some(s) = &self.store {
                        s.add(&host);
                    }
                    self.session_allowed.lock().unwrap().insert(host);
                    return true;
                }
                DomainAnswer::Deny => {
                    self.session_denied.lock().unwrap().insert(host.clone());
                }
            }
        }
        self.note_denial(&host);
        false
    }

    fn note_denial(&self, host: &str) {
        let mut d = self.denials.lock().unwrap();
        if !d.iter().any(|h| h == host) {
            d.push(host.to_string());
        }
    }
}

/// A running proxy.
pub struct Proxy {
    pub port: u16,
    pub socket: Option<PathBuf>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        if let Some(s) = &self.socket {
            let _ = std::fs::remove_file(s);
        }
    }
}

/// Listen on `127.0.0.1:<random>` and (optionally) a unix socket.
pub async fn start(gate: Arc<Gate>, socket: Option<PathBuf>) -> io::Result<Proxy> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let mut tasks = Vec::new();
    let g = gate.clone();
    tasks.push(tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else {
                break;
            };
            let g = g.clone();
            tokio::spawn(async move { handle(g, s).await });
        }
    }));
    #[cfg(unix)]
    let mut sock_path = None;
    #[cfg(unix)]
    if let Some(path) = socket {
        let _ = std::fs::remove_file(&path);
        let ul = tokio::net::UnixListener::bind(&path)?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let g = gate.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                let Ok((s, _)) = ul.accept().await else {
                    break;
                };
                let g = g.clone();
                tokio::spawn(async move { handle(g, s).await });
            }
        }));
        sock_path = Some(path);
    }
    #[cfg(not(unix))]
    let sock_path = {
        let _ = socket;
        None
    };
    Ok(Proxy {
        port,
        socket: sock_path,
        tasks,
    })
}

const MAX_HEAD: usize = 64 * 1024;

async fn deny<S: AsyncWrite + Unpin>(s: &mut S, status: &str, msg: &str) {
    let body = format!("ruckcode sandbox: {msg}\n");
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(resp.as_bytes()).await;
    let _ = s.shutdown().await;
}

fn split_host_port(t: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = t.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .map_or(Some(default_port), |p| p.parse().ok())?;
        return Some((h.to_string(), port));
    }
    match t.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Some((h.to_string(), p.parse().ok()?)),
        _ => Some((t.to_string(), default_port)),
    }
}

fn is_literal_or_localhost(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok() || host.eq_ignore_ascii_case("localhost")
}

/// Hostnames must not resolve into the local machine or link-local metadata services
/// (DNS-rebinding style bypasses of the allowlist). Literal addresses are what the user listed.
fn addr_ok(host: &str, ip: std::net::IpAddr) -> bool {
    if is_literal_or_localhost(host) {
        return true;
    }
    match ip {
        std::net::IpAddr::V4(v) => !(v.is_loopback() || v.is_link_local() || v.is_unspecified()),
        std::net::IpAddr::V6(v) => {
            !(v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

async fn connect_upstream(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last = io::Error::other("no addresses");
    for a in tokio::net::lookup_host((host, port)).await? {
        if !addr_ok(host, a.ip()) {
            last = io::Error::other("address not permitted");
            continue;
        }
        match TcpStream::connect(a).await {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn handle<S: AsyncRead + AsyncWrite + Unpin + Send>(gate: Arc<Gate>, mut s: S) {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > MAX_HEAD {
            return deny(
                &mut s,
                "431 Request Header Fields Too Large",
                "request too large",
            )
            .await;
        }
        match s.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let rest = buf[head_end..].to_vec();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return deny(&mut s, "400 Bad Request", "malformed request").await;
    };
    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = split_host_port(target, 443) else {
            return deny(&mut s, "400 Bad Request", "bad CONNECT target").await;
        };
        if !gate.decide(&host).await {
            return deny(&mut s, "403 Forbidden", &blocked_msg(&host)).await;
        }
        match connect_upstream(&host, port).await {
            Ok(mut up) => {
                if s.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                if !rest.is_empty() && up.write_all(&rest).await.is_err() {
                    return;
                }
                let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
            }
            Err(e) => {
                deny(
                    &mut s,
                    "502 Bad Gateway",
                    &format!("cannot reach {host}:{port}: {e}"),
                )
                .await
            }
        }
        return;
    }
    // Absolute-form request: METHOD http://host[:port]/path HTTP/1.1
    let Some(url) = target.strip_prefix("http://") else {
        return deny(
            &mut s,
            "400 Bad Request",
            "only http:// absolute URIs and CONNECT are proxied",
        )
        .await;
    };
    let (authority, path) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, "/"),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let Some((host, port)) = split_host_port(authority, 80) else {
        return deny(&mut s, "400 Bad Request", "bad URI").await;
    };
    if !gate.decide(&host).await {
        return deny(&mut s, "403 Forbidden", &blocked_msg(&host)).await;
    }
    let mut up = match connect_upstream(&host, port).await {
        Ok(u) => u,
        Err(e) => {
            return deny(
                &mut s,
                "502 Bad Gateway",
                &format!("cannot reach {host}:{port}: {e}"),
            )
            .await
        }
    };
    let mut out = format!("{method} {path} {version}\r\n");
    for l in lines {
        if l.is_empty() {
            continue;
        }
        let name = l.split(':').next().unwrap_or("").to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "proxy-connection" | "proxy-authorization" | "connection" | "keep-alive"
        ) {
            continue;
        }
        out.push_str(l);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    if up.write_all(out.as_bytes()).await.is_err()
        || (!rest.is_empty() && up.write_all(&rest).await.is_err())
    {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
}

fn blocked_msg(host: &str) -> String {
    format!(
        "network access to {host} is not allowed by the sandbox policy (add it to sandbox.allowed_domains in config.toml, or approve it when prompted)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(DomainAnswer);
    #[async_trait]
    impl DomainPrompter for Fixed {
        async fn ask(&self, _: &str, _: &str) -> DomainAnswer {
            self.0
        }
    }

    #[tokio::test]
    async fn gate_allowlist_prompt_and_memory() {
        let dir = tempfile::tempdir().unwrap();
        let store = DomainStore::at(dir.path().join("d.json"));
        let g = Gate::new(
            Some(store.clone()),
            Some(Arc::new(Fixed(DomainAnswer::Always))),
        );
        g.set_allowed(vec!["github.com".into()]);
        assert!(g.decide("GitHub.com").await);
        assert!(g.decide("example.com").await);
        assert_eq!(store.load(), ["example.com"]);
        let g2 = Gate::new(Some(store), None);
        assert!(g2.decide("example.com").await);
        assert!(!g2.decide("evil.test").await);
        assert_eq!(g2.take_denials(), ["evil.test"]);
        let g3 = Gate::new(None, Some(Arc::new(Fixed(DomainAnswer::Deny))));
        assert!(!g3.decide("x.test").await);
        assert!(!g3.decide("x.test").await);
    }

    #[tokio::test]
    async fn proxies_allowed_and_refuses_others() {
        // A tiny upstream HTTP server.
        let up = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_port = up.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = up.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut b = [0u8; 2048];
                    let _ = c.read(&mut b).await;
                    let _ = c
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi",
                        )
                        .await;
                });
            }
        });
        let gate = Arc::new(Gate::new(None, None));
        gate.set_allowed(vec!["127.0.0.1".into()]);
        let proxy = start(gate.clone(), None).await.unwrap();
        let fetch = |req: String| async move {
            let mut c = TcpStream::connect(("127.0.0.1", proxy.port)).await.unwrap();
            c.write_all(req.as_bytes()).await.unwrap();
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out).await;
            String::from_utf8_lossy(&out).into_owned()
        };
        let ok = fetch(format!(
            "GET http://127.0.0.1:{up_port}/x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        ))
        .await;
        assert!(ok.starts_with("HTTP/1.1 200") && ok.ends_with("hi"), "{ok}");
        let bad =
            fetch("GET http://blocked.invalid/ HTTP/1.1\r\nHost: blocked.invalid\r\n\r\n".into())
                .await;
        assert!(
            bad.starts_with("HTTP/1.1 403") && bad.contains("blocked.invalid"),
            "{bad}"
        );
        let bad = fetch("CONNECT blocked.invalid:443 HTTP/1.1\r\n\r\n".into()).await;
        assert!(bad.starts_with("HTTP/1.1 403"), "{bad}");
        let tunnel = fetch(format!(
            "CONNECT 127.0.0.1:{up_port} HTTP/1.1\r\n\r\nGET / HTTP/1.0\r\n\r\n"
        ))
        .await;
        assert!(
            tunnel.contains("200 Connection Established") && tunnel.ends_with("hi"),
            "{tunnel}"
        );
        assert_eq!(gate.take_denials(), ["blocked.invalid"]);
    }
}
