//! The shared network layer of the web tools: address policy (SSRF protection), a resolver that
//! enforces it at connect time, and the sandbox network gate.

use crate::ToolContext;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use ruckcode_sandbox::NetworkMode;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// Whether `v4` is not a globally routable unicast address (private, loopback, link-local
/// including the 169.254.169.254 metadata address, carrier-grade NAT, reserved, multicast...).
fn v4_blocked(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_multicast()
        || o[0] == 0 // "this network" 0.0.0.0/8
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // CGNAT 100.64/10 (Alibaba metadata 100.100.100.200)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
        || (o[0] == 192 && o[1] == 0 && o[2] == 2) // TEST-NET-1
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // benchmarking 198.18/15
        || (o[0] == 198 && o[1] == 51 && o[2] == 100) // TEST-NET-2
        || (o[0] == 203 && o[1] == 0 && o[2] == 113) // TEST-NET-3
        || o[0] >= 240 // reserved
}

fn v6_blocked(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    // IPv4 embedded in an IPv6 address: judge by the IPv4 address.
    if let Some(v4) = v6.to_ipv4_mapped() {
        return v4_blocked(v4); // ::ffff:a.b.c.d
    }
    if s[..6] == [0, 0, 0, 0, 0, 0] && (s[6] != 0 || s[7] > 1) {
        // IPv4-compatible ::a.b.c.d (deprecated)
        return v4_blocked(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        // NAT64 64:ff9b::/96
        return v4_blocked(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }
    if s[0] == 0x2002 {
        // 6to4: the embedded IPv4 address is what the packet is routed to.
        return v4_blocked(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            s[1] as u8,
            (s[2] >> 8) as u8,
            s[2] as u8,
        ));
    }
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7 (includes fd00:ec2::254)
        || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        || (s[0] & 0xffc0) == 0xfec0 // deprecated site-local fec0::/10
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
}

/// Whether the SSRF policy forbids connecting to `ip`.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v4_blocked(v),
        IpAddr::V6(v) => v6_blocked(v),
    }
}

/// Test hook: replaces DNS. `None` falls through to the system resolver.
pub type ResolveHook = Arc<dyn Fn(&str) -> Option<Vec<IpAddr>> + Send + Sync>;

/// Test hook: replaces the address policy.
pub type BlockedFn = Arc<dyn Fn(IpAddr) -> bool + Send + Sync>;

/// Address policy plus (optional) test hooks.
#[derive(Clone)]
pub struct NetPolicy {
    pub allow_private: bool,
    pub resolve_hook: Option<ResolveHook>,
    pub blocked: Option<BlockedFn>,
}

impl NetPolicy {
    pub fn new(allow_private: bool) -> Self {
        Self {
            allow_private,
            resolve_hook: None,
            blocked: None,
        }
    }

    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        if self.allow_private {
            return false;
        }
        match &self.blocked {
            Some(f) => f(ip),
            None => is_blocked_ip(ip),
        }
    }
}

/// Resolves a name once and hands the HTTP client only addresses the policy allows, so the
/// address that was checked is the address that is connected to (no second lookup to rebind).
struct SafeResolver(NetPolicy);

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.0.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            let ips: Vec<IpAddr> = match policy.resolve_hook.as_ref().and_then(|h| h(&host)) {
                Some(v) => v,
                None => tokio::net::lookup_host((host.as_str(), 0))
                    .await?
                    .map(|a| a.ip())
                    .collect(),
            };
            let total = ips.len();
            let ok: Vec<SocketAddr> = ips
                .into_iter()
                .filter(|ip| !policy.is_blocked(*ip))
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();
            if ok.is_empty() {
                let why = if total == 0 {
                    format!("`{host}` did not resolve")
                } else {
                    format!(
                        "`{host}` resolves to a private, loopback, link-local or metadata address"
                    )
                };
                return Err(why.into());
            }
            Ok(Box::new(ok.into_iter()) as Addrs)
        })
    }
}

/// Build the HTTP client: no proxies (they would resolve names themselves), no automatic
/// redirects (every hop is re-validated by the caller), connect and total timeouts.
pub fn client(
    policy: &NetPolicy,
    connect: Duration,
    total: Duration,
) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(super::user_agent())
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect)
        .timeout(total)
        .dns_resolver(Arc::new(SafeResolver(policy.clone())))
        .build()
        .map_err(|e| format!("cannot build the HTTP client: {e}"))
}

/// Check a URL's host before sending: literal IP addresses never reach the resolver, so they
/// are judged here. Names are judged by the resolver at connect time.
pub fn check_host(url: &reqwest::Url, policy: &NetPolicy) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => {}
        s => return Err(format!("only http and https URLs are allowed (got `{s}`)")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with embedded credentials are not allowed".into());
    }
    let Some(host) = url.host() else {
        return Err("the URL has no host".into());
    };
    let ip = match host {
        url::Host::Ipv4(v) => Some(IpAddr::V4(v)),
        url::Host::Ipv6(v) => Some(IpAddr::V6(v)),
        url::Host::Domain(_) => None,
    };
    if let Some(ip) = ip {
        if policy.is_blocked(ip) {
            return Err(format!(
                "refused: {ip} is a private, loopback, link-local or metadata address (set `web.allow_private = true` in your config.toml to allow such hosts)"
            ));
        }
    }
    Ok(())
}

/// Apply the sandbox network policy to a request for `host`: `Off` refuses, `Full` allows, and
/// `Allowlist` asks the shared gate (which prompts for unknown domains). `preallow` marks hosts
/// the user configured themselves (search providers).
pub async fn check_network(ctx: &ToolContext, host: &str, preallow: bool) -> Result<(), String> {
    let Some(h) = ctx.sandbox.as_ref() else {
        return Ok(()); // sandbox disabled by the user: no network policy
    };
    match h.cfg.network {
        NetworkMode::Off => Err(
            "refused: network access is disabled (`sandbox.network = \"off\"`). The user can change this in config.toml or the mode's `[modes.<id>.sandbox]` in modes.toml."
                .into(),
        ),
        NetworkMode::Full => Ok(()),
        NetworkMode::Allowlist => {
            let Some(gate) = h.gate.as_ref() else {
                return Err("refused: the network gate is unavailable".into());
            };
            gate.set_allowed(h.cfg.allowed_domains.clone());
            gate.set_current_call(&ctx.tool_call_id);
            if preallow {
                gate.allow_session(host);
            }
            if gate.decide(host).await {
                Ok(())
            } else {
                gate.take_denials();
                Err(format!(
                    "refused: network access to {host} is not allowed by the sandbox policy (the domain is not in sandbox.allowed_domains and was not approved). The user can add it to `sandbox.allowed_domains` in config.toml or approve it when prompted; you cannot change the policy."
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_loopback_linklocal_metadata_and_mapped_forms() {
        for s in [
            "10.0.0.1",
            "10.255.255.255",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "127.0.0.1",
            "127.1.2.3",
            "169.254.169.254",
            "169.254.0.1",
            "0.0.0.0",
            "100.100.100.200",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::1",
        ] {
            assert!(is_blocked_ip(s.parse().unwrap()), "{s} should be blocked");
        }
        for s in [
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "172.15.255.255",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_blocked_ip(s.parse().unwrap()), "{s} should be allowed");
        }
    }

    #[test]
    fn check_host_rules() {
        let p = NetPolicy::new(false);
        let ok = |u: &str| check_host(&u.parse().unwrap(), &p);
        assert!(ok("https://example.com/x").is_ok());
        assert!(ok("ftp://example.com/").is_err());
        assert!(ok("file:///etc/passwd").is_err());
        assert!(ok("http://user:pw@example.com/").is_err());
        assert!(ok("http://127.0.0.1/").is_err());
        assert!(ok("http://[::1]/").is_err());
        assert!(ok("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(ok("http://2130706433/").is_err()); // decimal 127.0.0.1
        assert!(ok("http://0x7f.1/").is_err());
        assert!(ok("http://[::ffff:7f00:1]/").is_err());
        let open = NetPolicy::new(true);
        assert!(check_host(&"http://127.0.0.1/".parse().unwrap(), &open).is_ok());
    }
}
