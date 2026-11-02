//! Retry policy for model requests: what to do when the API answers 429 / 5xx or the
//! connection drops, plus proactive pacing from the rate-limit headers of earlier responses.
//!
//! The policy is pure (see [`decide`]) so it can be unit-tested without a network or a clock;
//! the agent loop (`agent.rs`) owns waiting, cancellation and user-visible status.

use chrono::{DateTime, Local};
use ruckcode_llm::{is_transport_error, ApiError, LimitKind, RateLimitInfo};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn default_max_auto_wait_secs() -> u64 {
    60
}

fn default_max_retries() -> u32 {
    6
}

fn default_true() -> bool {
    true
}

/// `[rate_limit]` in `config.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RateLimitConfig {
    /// Longest wait (seconds) the agent sits out by itself before retrying a rate-limited
    /// request. A longer reset (for example an exhausted daily budget) ends the turn as
    /// `rate_limited` instead; the session can be continued after the reset. Default 60.
    #[serde(default = "default_max_auto_wait_secs")]
    pub max_auto_wait_secs: u64,
    /// Retries per model request for 429, overloaded (529/503), other transient 5xx and dropped
    /// connections. Default 6 (backoff 1s, 2s, 4s, ... capped at 60s, with jitter, when the
    /// server gives no timing).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Delay a request until the reset time when the previous response reported an exhausted
    /// per-minute budget, instead of sending it into a guaranteed 429. Default true.
    #[serde(default = "default_true")]
    pub pacing: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_auto_wait_secs: default_max_auto_wait_secs(),
            max_retries: default_max_retries(),
            pacing: true,
        }
    }
}

impl RateLimitConfig {
    pub fn max_auto_wait(&self) -> Duration {
        Duration::from_secs(self.max_auto_wait_secs)
    }
}

/// Why a turn gave up on rate limits instead of waiting.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitStop {
    pub kind: LimitKind,
    /// Time until the limit resets, when the server said.
    pub wait: Option<Duration>,
    pub info: RateLimitInfo,
    /// The wait was longer than `max_auto_wait_secs` (as opposed to retries running out).
    pub too_long: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Wait `delay`, then send the request again.
    Retry {
        delay: Duration,
        /// "Rate limited (tokens/minute)", "Server overloaded (HTTP 529)", ...
        reason: String,
    },
    /// End the turn as rate limited (resumable).
    Stop(RateLimitStop),
    /// Not retryable (or retries exhausted for a non-rate-limit failure): surface the error.
    Fail,
}

/// Exponential backoff 1s, 2s, 4s, ... capped at 60s. `jitter` in `[0, 1]` scales the delay into
/// `[50%, 100%]` so parallel clients do not retry in lockstep.
pub fn backoff(attempt: u32, jitter: f64) -> Duration {
    let base = 1u64 << attempt.min(6);
    let secs = base.min(60) as f64;
    Duration::from_secs_f64(secs * (0.5 + 0.5 * jitter.clamp(0.0, 1.0)))
}

/// Cheap jitter source in `[0, 1)` without a random-number dependency.
pub fn jitter() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    (h.finish() % 10_000) as f64 / 10_000.0
}

/// Decide what to do about a failed model request. `attempt` is how many retries were already
/// made for this request.
pub fn decide(err: &anyhow::Error, attempt: u32, cfg: &RateLimitConfig, jitter: f64) -> Decision {
    let api = err.chain().find_map(|e| e.downcast_ref::<ApiError>());
    if let Some(api) = api {
        if api.is_rate_limited() {
            return decide_rate_limited(&api.rate_limit, attempt, cfg, jitter);
        }
        if api.is_transient() {
            let reason = match api.status {
                529 | 503 => format!("Server overloaded (HTTP {})", api.status),
                s => format!("Server error (HTTP {s})"),
            };
            return decide_transient(api.rate_limit.retry_after, reason, attempt, cfg, jitter);
        }
        return Decision::Fail;
    }
    if is_transport_error(err) {
        return decide_transient(None, "Connection problem".into(), attempt, cfg, jitter);
    }
    Decision::Fail
}

fn decide_transient(
    hint: Option<Duration>,
    reason: String,
    attempt: u32,
    cfg: &RateLimitConfig,
    jitter: f64,
) -> Decision {
    if attempt >= cfg.max_retries {
        return Decision::Fail;
    }
    let delay = hint.unwrap_or_else(|| backoff(attempt, jitter));
    if delay > cfg.max_auto_wait() {
        return Decision::Fail;
    }
    Decision::Retry { delay, reason }
}

fn decide_rate_limited(
    info: &RateLimitInfo,
    attempt: u32,
    cfg: &RateLimitConfig,
    jitter: f64,
) -> Decision {
    let stop = |kind, wait, too_long| {
        Decision::Stop(RateLimitStop {
            kind,
            wait,
            info: info.clone(),
            too_long,
        })
    };
    let (delay, kind) = match info.wait() {
        Some((wait, kind)) => {
            if wait > cfg.max_auto_wait() {
                return stop(kind, Some(wait), true);
            }
            (wait, kind)
        }
        None => {
            let kind = info.hit_kind();
            // A budget (daily credits, ...) that says nothing about when it refills cannot be
            // waited out with a few quick retries.
            if kind.is_budget() {
                return stop(kind, None, false);
            }
            (backoff(attempt, jitter), kind)
        }
    };
    if attempt >= cfg.max_retries {
        return stop(kind, info.wait().map(|(w, _)| w), false);
    }
    Decision::Retry {
        delay,
        reason: format!("Rate limited ({})", kind.label()),
    }
}

/// Latest rate-limit snapshot per provider profile, used to delay requests that would be
/// rejected anyway.
#[derive(Default)]
pub struct Pacer {
    snapshots: Mutex<HashMap<String, (Instant, RateLimitInfo)>>,
}

impl Pacer {
    /// Remember what a response (successful or 429) reported.
    pub fn observe(&self, key: &str, info: &RateLimitInfo) {
        if info.buckets.is_empty() {
            return;
        }
        self.snapshots
            .lock()
            .unwrap()
            .insert(key.to_string(), (Instant::now(), info.clone()));
    }

    /// Forget the snapshot (after waiting for its reset).
    pub fn clear(&self, key: &str) {
        self.snapshots.lock().unwrap().remove(key);
    }

    /// When the last response left a per-minute bucket at (or nearly at) zero with a known reset
    /// that has not passed yet: the remaining time and which bucket. `None` means go ahead.
    pub fn gate(&self, key: &str) -> Option<(Duration, LimitKind, RateLimitInfo)> {
        let map = self.snapshots.lock().unwrap();
        let (seen, info) = map.get(key)?;
        let elapsed = seen.elapsed();
        // An exhausted daily budget is left to the server (a stale header must not lock a
        // session out), and waiting out a minute bucket would not help while it is empty.
        if info.exhausted().any(|b| b.kind.is_budget()) {
            return None;
        }
        let (reset, kind) = info
            .exhausted()
            .filter_map(|b| b.reset.map(|r| (r, b.kind)))
            .max_by_key(|(r, _)| *r)?;
        let left = reset.checked_sub(elapsed).filter(|d| !d.is_zero())?;
        Some((left, kind, info.clone()))
    }
}

/// "51s", "5m 10s", "1h 0m", "2d 3h".
pub fn format_wait(d: Duration) -> String {
    let s = d.as_secs() + u64::from(d.subsec_millis() > 0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d {}h", s / 86_400, (s % 86_400) / 3600)
    }
}

fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The message shown when a turn ends because of rate limits.
pub fn stop_message(stop: &RateLimitStop, now: DateTime<Local>) -> String {
    let what = match stop.kind {
        LimitKind::DailyCredits => "the daily credit budget is used up",
        LimitKind::TokensPerDay => "the daily token budget is used up",
        LimitKind::RequestsPerDay => "the daily request budget is used up",
        LimitKind::TokensPerMinute => "the tokens/minute limit was hit",
        LimitKind::RequestsPerMinute => "the requests/minute limit was hit",
        LimitKind::Other => "the API is rate limiting requests",
    };
    let mut msg = format!("Rate limited: {what}");
    if let Some(b) = stop.info.bucket(stop.kind) {
        if let (Some(rem), Some(lim)) = (b.remaining, b.limit) {
            msg.push_str(&format!(
                " ({} of {} remaining)",
                format_count(rem),
                format_count(lim)
            ));
        }
    }
    msg.push('.');
    match stop.wait {
        Some(w) => {
            let at = now + chrono::Duration::from_std(w).unwrap_or_default();
            let when = if w >= Duration::from_secs(86_400) {
                at.format("%Y-%m-%d %H:%M").to_string()
            } else {
                at.format("%H:%M").to_string()
            };
            msg.push_str(&format!(
                " It resets in {} (at {when} local time).",
                format_wait(w)
            ));
        }
        None => msg.push_str(" The server did not say when it resets."),
    }
    if stop.too_long {
        msg.push_str(
            " That is longer than the automatic wait limit (`rate_limit.max_auto_wait_secs`).",
        );
    } else if stop.wait.is_some() || stop.kind.is_budget() {
        // Retries ran out, or the budget has no known reset.
    } else {
        msg.push_str(" Retries ran out.");
    }
    msg.push_str(" The session is saved; send a message after the reset to continue.");
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn err429(headers: &[(&str, &str)], body: &str) -> anyhow::Error {
        let info = RateLimitInfo::parse(&h(headers), body, SystemTime::now());
        ApiError::new("test", 429, body.into(), info).into()
    }

    fn err_status(status: u16) -> anyhow::Error {
        ApiError::new("test", status, "x".into(), RateLimitInfo::default()).into()
    }

    const OWNER_BODY: &str = r#"{"error":{"type":"rate_limit_exceeded","message":"m","param":null,"code":"tokens_per_minute_exceeded","retry_after_seconds":51}}"#;

    #[test]
    fn short_wait_is_retried_after_the_hint() {
        let cfg = RateLimitConfig::default();
        let e = err429(
            &[
                ("RetryAfter", "51"),
                ("X-RateLimit-Remainint-Tokens-Minute", "0"),
                ("X-RateLimit-Reset-Tokens-Minute", "51"),
            ],
            OWNER_BODY,
        );
        assert_eq!(
            decide(&e, 0, &cfg, 0.0),
            Decision::Retry {
                delay: Duration::from_secs(51),
                reason: "Rate limited (tokens/minute)".into()
            }
        );
    }

    #[test]
    fn daily_budget_ends_the_turn() {
        let cfg = RateLimitConfig::default();
        let e = err429(
            &[
                ("RetryAfter", "51"),
                ("X-RateLimit-Limit-Credits-Day", "2000000"),
                ("X-RateLimit-Remaining-Credits-Day", "0"),
                ("X-RateLimit-Reset-Credits-Day", "3600"),
            ],
            OWNER_BODY,
        );
        let Decision::Stop(s) = decide(&e, 0, &cfg, 0.0) else {
            panic!("expected stop");
        };
        assert_eq!(s.kind, LimitKind::DailyCredits);
        assert_eq!(s.wait, Some(Duration::from_secs(3600)));
        assert!(s.too_long);
        let msg = stop_message(&s, Local::now());
        assert!(msg.contains("daily credit budget"), "{msg}");
        assert!(msg.contains("0 of 2,000,000 remaining"), "{msg}");
        assert!(msg.contains("resets in 1h 0m"), "{msg}");
        assert!(msg.contains("local time"), "{msg}");
    }

    #[test]
    fn max_auto_wait_is_configurable() {
        let e = err429(&[("retry-after", "120")], "");
        let short = RateLimitConfig {
            max_auto_wait_secs: 60,
            ..Default::default()
        };
        assert!(matches!(decide(&e, 0, &short, 0.0), Decision::Stop(s) if s.too_long));
        let long = RateLimitConfig {
            max_auto_wait_secs: 600,
            ..Default::default()
        };
        assert!(matches!(decide(&e, 0, &long, 0.0), Decision::Retry { .. }));
    }

    #[test]
    fn default_limit_is_sixty_seconds() {
        let cfg = RateLimitConfig::default();
        assert!(matches!(
            decide(&err429(&[("retry-after", "60")], ""), 0, &cfg, 0.0),
            Decision::Retry { .. }
        ));
        assert!(matches!(
            decide(&err429(&[("retry-after", "61")], ""), 0, &cfg, 0.0),
            Decision::Stop(s) if s.too_long
        ));
    }

    #[test]
    fn missing_headers_back_off_exponentially_with_jitter() {
        let cfg = RateLimitConfig::default();
        let e = err429(&[], "slow down");
        for (attempt, base) in [(0u32, 1.0), (1, 2.0), (2, 4.0), (5, 32.0)] {
            let Decision::Retry { delay, .. } = decide(&e, attempt, &cfg, 1.0) else {
                panic!()
            };
            assert_eq!(delay, Duration::from_secs_f64(base));
            let Decision::Retry { delay, .. } = decide(&e, attempt, &cfg, 0.0) else {
                panic!()
            };
            assert_eq!(delay, Duration::from_secs_f64(base / 2.0));
        }
        assert_eq!(backoff(20, 1.0), Duration::from_secs(60));
    }

    #[test]
    fn retries_run_out_as_rate_limited() {
        let cfg = RateLimitConfig {
            max_retries: 2,
            ..Default::default()
        };
        let e = err429(&[], "");
        assert!(matches!(decide(&e, 1, &cfg, 0.0), Decision::Retry { .. }));
        assert!(matches!(decide(&e, 2, &cfg, 0.0), Decision::Stop(s) if !s.too_long));
    }

    #[test]
    fn budget_without_a_reset_does_not_spin() {
        let cfg = RateLimitConfig::default();
        let e = err429(&[], r#"{"error":{"code":"daily_credits_exceeded"}}"#);
        assert!(matches!(
            decide(&e, 0, &cfg, 0.0),
            Decision::Stop(s) if s.kind == LimitKind::DailyCredits && s.wait.is_none()
        ));
    }

    #[test]
    fn overload_and_server_errors_retry_others_fail() {
        let cfg = RateLimitConfig::default();
        for status in [529, 503, 500, 502, 504] {
            assert!(
                matches!(
                    decide(&err_status(status), 0, &cfg, 0.0),
                    Decision::Retry { .. }
                ),
                "{status}"
            );
        }
        for status in [400, 401, 403, 404] {
            assert_eq!(decide(&err_status(status), 0, &cfg, 0.0), Decision::Fail);
        }
        assert_eq!(
            decide(&err_status(529), cfg.max_retries, &cfg, 0.0),
            Decision::Fail
        );
        assert_eq!(
            decide(&anyhow::anyhow!("boom"), 0, &cfg, 0.0),
            Decision::Fail
        );
        let io: anyhow::Error = std::io::Error::from(std::io::ErrorKind::ConnectionReset).into();
        assert!(matches!(decide(&io, 0, &cfg, 0.0), Decision::Retry { .. }));
    }

    #[test]
    fn overloaded_honors_retry_after() {
        let cfg = RateLimitConfig::default();
        let info = RateLimitInfo::parse(&h(&[("retry-after", "7")]), "", SystemTime::now());
        let e: anyhow::Error = ApiError::new("t", 529, "overloaded".into(), info).into();
        assert_eq!(
            decide(&e, 3, &cfg, 0.0),
            Decision::Retry {
                delay: Duration::from_secs(7),
                reason: "Server overloaded (HTTP 529)".into()
            }
        );
    }

    #[test]
    fn pacer_gates_on_exhausted_buckets_only() {
        let p = Pacer::default();
        let ok = RateLimitInfo::parse(
            &h(&[
                ("x-ratelimit-remaining-tokens-minute", "4000"),
                ("x-ratelimit-limit-tokens-minute", "5000"),
                ("x-ratelimit-reset-tokens-minute", "30"),
            ]),
            "",
            SystemTime::now(),
        );
        p.observe("k", &ok);
        assert!(p.gate("k").is_none());
        let empty = RateLimitInfo::parse(
            &h(&[
                ("x-ratelimit-remaining-tokens-minute", "0"),
                ("x-ratelimit-reset-tokens-minute", "30"),
            ]),
            "",
            SystemTime::now(),
        );
        p.observe("k", &empty);
        let (left, kind, _) = p.gate("k").unwrap();
        assert_eq!(kind, LimitKind::TokensPerMinute);
        assert!(left > Duration::from_secs(29) && left <= Duration::from_secs(30));
        assert!(p.gate("other").is_none());
        p.clear("k");
        assert!(p.gate("k").is_none());
    }

    #[test]
    fn pacer_leaves_exhausted_daily_budgets_to_the_server() {
        let p = Pacer::default();
        let info = RateLimitInfo::parse(
            &h(&[
                ("x-ratelimit-remaining-credits-day", "0"),
                ("x-ratelimit-reset-credits-day", "3600"),
                ("x-ratelimit-remaining-tokens-minute", "0"),
                ("x-ratelimit-reset-tokens-minute", "30"),
            ]),
            "",
            SystemTime::now(),
        );
        p.observe("k", &info);
        assert!(p.gate("k").is_none());
    }

    #[test]
    fn config_defaults_and_overrides() {
        let d = RateLimitConfig::default();
        assert_eq!(
            (d.max_auto_wait_secs, d.max_retries, d.pacing),
            (60, 6, true)
        );
        let c: RateLimitConfig =
            toml::from_str("max_auto_wait_secs = 30\nmax_retries = 2\npacing = false").unwrap();
        assert_eq!(c.max_auto_wait(), Duration::from_secs(30));
        assert_eq!((c.max_retries, c.pacing), (2, false));
        let cfg = crate::Config::from_toml_str("[rate_limit]\nmax_retries = 9").unwrap();
        assert_eq!(cfg.rate_limit.max_retries, 9);
        assert_eq!(cfg.rate_limit.max_auto_wait_secs, 60);
    }

    #[test]
    fn wait_formatting() {
        assert_eq!(format_wait(Duration::from_secs(51)), "51s");
        assert_eq!(format_wait(Duration::from_millis(50_200)), "51s");
        assert_eq!(format_wait(Duration::from_secs(310)), "5m 10s");
        assert_eq!(format_wait(Duration::from_secs(3600)), "1h 0m");
        assert_eq!(format_wait(Duration::from_secs(90_000)), "1d 1h");
    }
}
