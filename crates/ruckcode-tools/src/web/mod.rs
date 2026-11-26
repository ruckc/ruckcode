//! The `WebFetch` and `WebSearch` tools and their shared network layer.
//!
//! Both run in-process (not inside the command sandbox), so they apply the sandbox's network
//! policy themselves: `sandbox.network = "off"` refuses them, `"full"` skips the domain prompt,
//! and `"allowlist"` uses the same `Gate` as the sandbox proxy (allowlist, remembered
//! per-project decisions, "allow once / always / deny" prompt). On top of that they refuse
//! private, loopback, link-local and metadata addresses (see [`net`]).

mod config;
mod fetch;
mod html;
pub mod net;
mod search;

pub use config::{ProviderConf, SearchConfig, Secret, WebConfig};
pub use fetch::WebFetch;
pub use search::WebSearch;

/// Tool names, shared with the profile allowlist logic.
pub const WEB_FETCH: &str = "WebFetch";
pub const WEB_SEARCH: &str = "WebSearch";

/// The `User-Agent` of every request: identifies ruckcode, carries nothing personal.
pub fn user_agent() -> String {
    format!(
        "ruckcode/{} (+https://github.com/ruckc/ruckcode)",
        env!("CARGO_PKG_VERSION")
    )
}

/// Truncate to at most `max` characters on a char boundary. Returns the text and the total
/// number of characters when it was cut.
pub(crate) fn truncate_chars(s: &str, max: usize) -> (String, Option<usize>) {
    match s.char_indices().nth(max) {
        None => (s.to_string(), None),
        Some((i, _)) => (s[..i].to_string(), Some(s.chars().count())),
    }
}
