//! Rate-limit and transient-failure information parsed from API responses.
//!
//! Providers disagree on header names and formats, so parsing is deliberately lenient:
//!
//! * header names are compared case-insensitively with `-` and `_` removed, so `RetryAfter`,
//!   `Retry-After` and `retry_after` are the same header;
//! * `X-RateLimit-{Limit,Remaining,Reset}-<resource>[-<period>]` (OpenAI and the "credits / tokens
//!   per minute" flavour), including the documented typo `Remainint`;
//! * `anthropic-ratelimit-<resource>-{limit,remaining,reset}`;
//! * reset values may be seconds, durations (`6m0s`, `20ms`, `1.5s`), RFC 3339 timestamps or
//!   HTTP dates; `Retry-After` may be seconds or an HTTP date, `retry-after-ms` is milliseconds;
//! * the JSON body may carry `error.retry_after_seconds` and an `error.code` naming the limit.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::time::{Duration, SystemTime};

/// Which limit was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    TokensPerMinute,
    RequestsPerMinute,
    TokensPerDay,
    RequestsPerDay,
    DailyCredits,
    Other,
}

impl LimitKind {
    /// Short human label, e.g. for "Rate limited (tokens/minute)".
    pub fn label(self) -> &'static str {
        match self {
            LimitKind::TokensPerMinute => "tokens/minute",
            LimitKind::RequestsPerMinute => "requests/minute",
            LimitKind::TokensPerDay => "tokens/day",
            LimitKind::RequestsPerDay => "requests/day",
            LimitKind::DailyCredits => "daily credits",
            LimitKind::Other => "rate limit",
        }
    }

    /// Budgets that refill once a day (or similar): waiting is never a quick retry.
    pub fn is_budget(self) -> bool {
        matches!(
            self,
            LimitKind::TokensPerDay | LimitKind::RequestsPerDay | LimitKind::DailyCredits
        )
    }

    fn from_resource(resource: &str, period: &str) -> LimitKind {
        let day = period.contains("day") || period.contains("daily");
        if resource.contains("credit") {
            LimitKind::DailyCredits
        } else if resource.contains("token") {
            if day {
                LimitKind::TokensPerDay
            } else {
                LimitKind::TokensPerMinute
            }
        } else if resource.contains("request") {
            if day {
                LimitKind::RequestsPerDay
            } else {
                LimitKind::RequestsPerMinute
            }
        } else {
            LimitKind::Other
        }
    }

    /// Classify a body `error.code` / `error.type` such as `tokens_per_minute_exceeded`.
    fn from_code(code: &str) -> Option<LimitKind> {
        let c = code.to_ascii_lowercase();
        let day = c.contains("day") || c.contains("daily");
        if c.contains("credit") || c.contains("budget") || c.contains("quota") {
            Some(LimitKind::DailyCredits)
        } else if c.contains("token") {
            Some(if day {
                LimitKind::TokensPerDay
            } else {
                LimitKind::TokensPerMinute
            })
        } else if c.contains("request") {
            Some(if day {
                LimitKind::RequestsPerDay
            } else {
                LimitKind::RequestsPerMinute
            })
        } else if day {
            Some(LimitKind::DailyCredits)
        } else {
            None
        }
    }
}

/// One rate-limit bucket (for example "tokens per minute") as last reported by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bucket {
    pub kind: LimitKind,
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    /// Time until the bucket refills, relative to when the response was received.
    pub reset: Option<Duration>,
}

impl Bucket {
    /// Nothing (or almost nothing: under 1% of a known limit) is left.
    pub fn is_exhausted(&self) -> bool {
        match (self.remaining, self.limit) {
            (Some(0), _) => true,
            (Some(r), Some(l)) if l > 0 => r.saturating_mul(100) < l,
            _ => false,
        }
    }
}

/// Everything the server told us about its limits in one response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RateLimitInfo {
    /// Explicit "try again in" hint: body `retry_after_seconds`, else `retry-after-ms` /
    /// `RetryAfter` / `Retry-After`.
    pub retry_after: Option<Duration>,
    /// The limit named by the body's `code`, if any.
    pub code_kind: Option<LimitKind>,
    /// Body `error.code` verbatim.
    pub code: Option<String>,
    pub buckets: Vec<Bucket>,
}

/// Normalised header name: lowercase, without `-` and `_`.
fn norm(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '-' && *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Parse a duration such as `6m0s`, `1.5s`, `20ms`, `1h2m3s`.
fn parse_duration_str(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0f64;
    let mut rest = s;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        let n: f64 = rest[..end].parse().ok()?;
        rest = &rest[end..];
        let (mult, len) = if rest.starts_with("ms") {
            (0.001, 2)
        } else if rest.starts_with('h') {
            (3600.0, 1)
        } else if rest.starts_with('m') {
            (60.0, 1)
        } else if rest.starts_with('s') {
            (1.0, 1)
        } else {
            return None;
        };
        total += n * mult;
        rest = &rest[len..];
    }
    Some(Duration::from_secs_f64(total))
}

fn until(t: DateTime<Utc>, now: SystemTime) -> Duration {
    let now: DateTime<Utc> = now.into();
    (t - now).to_std().unwrap_or(Duration::ZERO)
}

fn parse_timestamp(s: &str, now: SystemTime) -> Option<Duration> {
    let t = DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_rfc2822(s))
        .ok()?;
    Some(until(t.with_timezone(&Utc), now))
}

/// A "time until reset" value: seconds, a duration string, an epoch timestamp, RFC 3339 or an
/// HTTP date.
pub fn parse_reset(value: &str, now: SystemTime) -> Option<Duration> {
    let v = value.trim();
    if let Ok(n) = v.parse::<f64>() {
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        // Absolute epoch seconds / milliseconds (OpenRouter style) rather than a delay.
        let epoch = if n >= 1e12 {
            Some(n / 1000.0)
        } else if n >= 1e9 {
            Some(n)
        } else {
            None
        };
        return Some(match epoch {
            Some(secs) => {
                let at = SystemTime::UNIX_EPOCH + Duration::from_secs_f64(secs);
                at.duration_since(now).unwrap_or(Duration::ZERO)
            }
            None => Duration::from_secs_f64(n),
        });
    }
    parse_duration_str(v).or_else(|| parse_timestamp(v, now))
}

/// `Retry-After`: delay seconds (possibly fractional) or an HTTP date.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let v = value.trim();
    if let Ok(n) = v.parse::<f64>() {
        return (n.is_finite() && n >= 0.0).then(|| Duration::from_secs_f64(n));
    }
    parse_timestamp(v, now).or_else(|| parse_duration_str(v))
}

impl RateLimitInfo {
    /// Parse headers (any casing/spelling, see the module docs) and an optional JSON body.
    pub fn parse(headers: &[(String, String)], body: &str, now: SystemTime) -> Self {
        let mut info = RateLimitInfo::default();
        let mut hint_ms: Option<Duration> = None;
        let mut hint_s: Option<Duration> = None;
        // (resource key, bucket)
        let mut buckets: Vec<(String, Bucket)> = Vec::new();
        for (name, value) in headers {
            let n = norm(name);
            let value = value.trim();
            match n.as_str() {
                "retryafterms" => {
                    hint_ms = value
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite() && *v >= 0.0)
                        .map(|ms| Duration::from_secs_f64(ms / 1000.0))
                }
                "retryafter" => hint_s = parse_retry_after(value, now),
                _ => {
                    // Split into (field, resource+period).
                    let (field, rest): (&str, String) =
                        if let Some(r) = n.strip_prefix("xratelimit") {
                            let fields = ["limit", "remaining", "remainint", "reset"];
                            match fields.iter().find(|f| r.starts_with(**f)) {
                                Some(f) => (f, r[f.len()..].to_string()),
                                None => continue,
                            }
                        } else if let Some(r) = n.strip_prefix("anthropicratelimit") {
                            let fields = ["limit", "remaining", "reset"];
                            match fields.iter().find(|f| r.ends_with(**f)) {
                                Some(f) => (f, r[..r.len() - f.len()].to_string()),
                                None => continue,
                            }
                        } else {
                            continue;
                        };
                    let (resource, period) = if let Some(p) = rest.strip_suffix("minute") {
                        (p, "minute")
                    } else if let Some(p) = rest.strip_suffix("day") {
                        (p, "day")
                    } else {
                        (rest.as_str(), "")
                    };
                    let kind = LimitKind::from_resource(resource, period);
                    if kind == LimitKind::Other {
                        continue;
                    }
                    let key = format!("{resource}/{period}");
                    let i = match buckets.iter().position(|(k, _)| *k == key) {
                        Some(i) => i,
                        None => {
                            buckets.push((
                                key,
                                Bucket {
                                    kind,
                                    limit: None,
                                    remaining: None,
                                    reset: None,
                                },
                            ));
                            buckets.len() - 1
                        }
                    };
                    let b = &mut buckets[i].1;
                    match field {
                        "limit" => b.limit = value.parse().ok(),
                        "remaining" | "remainint" => b.remaining = value.parse().ok(),
                        _ => b.reset = parse_reset(value, now),
                    }
                }
            }
        }
        info.retry_after = hint_ms.or(hint_s);
        info.buckets = buckets.into_iter().map(|(_, b)| b).collect();

        if let Ok(v) = serde_json::from_str::<Value>(body) {
            let e = v.get("error").unwrap_or(&v);
            let secs = ["retry_after_seconds", "retry_after"]
                .iter()
                .find_map(|k| e.get(*k).and_then(Value::as_f64))
                .filter(|s| s.is_finite() && *s >= 0.0);
            if let Some(s) = secs {
                info.retry_after = Some(Duration::from_secs_f64(s));
            }
            if let Some(code) = e.get("code").and_then(Value::as_str) {
                info.code = Some(code.to_string());
                info.code_kind = LimitKind::from_code(code);
            }
            if info.code_kind.is_none() {
                info.code_kind = e
                    .get("type")
                    .and_then(Value::as_str)
                    .and_then(LimitKind::from_code);
            }
        }
        info
    }

    /// Whether there is anything worth keeping (used to skip empty snapshots).
    pub fn is_empty(&self) -> bool {
        self.retry_after.is_none() && self.buckets.is_empty() && self.code_kind.is_none()
    }

    /// Buckets that are at (or nearly at) zero.
    pub fn exhausted(&self) -> impl Iterator<Item = &Bucket> {
        self.buckets.iter().filter(|b| b.is_exhausted())
    }

    /// Which limit was hit and how long to wait for it, or `None` when the server gave no
    /// timing (the caller then backs off exponentially).
    ///
    /// Priority: the explicit hint (body `retry_after_seconds`, then `retry-after-ms` /
    /// `RetryAfter` / `Retry-After`), then the reset of the exhausted bucket (the one that
    /// refills last when several are at zero). An exhausted bucket that refills *after* the hint
    /// wins over it, because retrying earlier would be rejected again.
    pub fn wait(&self) -> Option<(Duration, LimitKind)> {
        let gating = self
            .exhausted()
            .filter_map(|b| b.reset.map(|r| (r, b.kind)))
            .max_by_key(|(r, _)| *r);
        match (self.retry_after, gating) {
            (Some(h), Some((r, k))) if r > h => Some((r, k)),
            (Some(h), _) => Some((h, self.hit_kind())),
            (None, Some((r, k))) => Some((r, k)),
            (None, None) => None,
        }
    }

    /// The limit that was hit: the body's `code`, else the first exhausted bucket.
    pub fn hit_kind(&self) -> LimitKind {
        self.code_kind
            .or_else(|| self.exhausted().map(|b| b.kind).next())
            .unwrap_or(LimitKind::Other)
    }

    /// The bucket for `kind`, if reported.
    pub fn bucket(&self, kind: LimitKind) -> Option<&Bucket> {
        self.buckets.iter().find(|b| b.kind == kind)
    }
}

/// An HTTP error from a provider API. Providers return it inside `anyhow::Error`; the agent
/// downcasts to decide whether to retry.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub provider: String,
    pub status: u16,
    pub body: String,
    pub rate_limit: RateLimitInfo,
}

impl ApiError {
    pub fn new(provider: &str, status: u16, body: String, rate_limit: RateLimitInfo) -> Self {
        Self {
            provider: provider.to_string(),
            status,
            body,
            rate_limit,
        }
    }

    pub fn is_rate_limited(&self) -> bool {
        self.status == 429
    }

    /// Server overloaded or temporarily unavailable (Anthropic 529, 503) or another transient
    /// server-side failure worth retrying.
    pub fn is_transient(&self) -> bool {
        matches!(self.status, 408 | 500 | 502 | 503 | 504 | 529)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} API error {}: {}",
            self.provider, self.status, self.body
        )
    }
}

impl std::error::Error for ApiError {}

fn header_pairs(resp: &reqwest::Response) -> Vec<(String, String)> {
    resp.headers()
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
        .collect()
}

/// Build an [`ApiError`] from a failed HTTP response.
pub(crate) async fn api_error(provider: &str, resp: reqwest::Response) -> ApiError {
    let status = resp.status().as_u16();
    let headers = header_pairs(&resp);
    let body = resp.text().await.unwrap_or_default();
    let info = RateLimitInfo::parse(&headers, &body, SystemTime::now());
    ApiError::new(provider, status, body, info)
}

/// Rate-limit headers of a successful response, if the server sent any.
pub(crate) fn from_success(resp: &reqwest::Response) -> Option<RateLimitInfo> {
    let info = RateLimitInfo::parse(&header_pairs(resp), "", SystemTime::now());
    (!info.buckets.is_empty()).then_some(info)
}

/// Map a streamed `error` event (`overloaded_error`, `rate_limit_error`, ...) to an HTTP-like
/// status so it takes the same retry path.
pub(crate) fn stream_error_status(error_type: &str) -> u16 {
    match error_type {
        "overloaded_error" => 529,
        "rate_limit_error" => 429,
        "api_error" => 500,
        _ => 0,
    }
}

/// Whether `err` looks like a transient network failure (connection reset, timeout, ...).
pub fn is_transport_error(err: &anyhow::Error) -> bool {
    err.chain().any(|e| {
        if let Some(r) = e.downcast_ref::<reqwest::Error>() {
            return r.is_connect() || r.is_timeout() || r.is_request() || r.is_body();
        }
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            return matches!(
                io.kind(),
                ConnectionReset
                    | ConnectionAborted
                    | ConnectionRefused
                    | BrokenPipe
                    | UnexpectedEof
                    | TimedOut
            );
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    const BODY: &str = r#"{"error": {"type": "rate_limit_exceeded","message":"slow down","param":null,"code":"tokens_per_minute_exceeded","retry_after_seconds": 51}}"#;

    #[test]
    fn owners_sample_headers_and_body() {
        let headers = h(&[
            ("RetryAfter", "51"),
            ("X-RateLimit-Limit-Credits-Day", "2000000"),
            ("X-RateLimit-Remaining-Credits-Day", "0"),
            ("X-RateLimit-Reset-Credits-Day", "3600"),
            ("X-RateLimit-Limit-Tokens-Minute", "500000"),
            ("X-RateLimit-Remainint-Tokens-Minute", "0"),
            ("X-RateLimit-Reset-Tokens-Minute", "51"),
        ]);
        let i = RateLimitInfo::parse(&headers, BODY, SystemTime::now());
        assert_eq!(i.retry_after, Some(secs(51)));
        assert_eq!(i.code.as_deref(), Some("tokens_per_minute_exceeded"));
        assert_eq!(i.code_kind, Some(LimitKind::TokensPerMinute));
        let credits = i.bucket(LimitKind::DailyCredits).unwrap();
        assert_eq!(credits.limit, Some(2_000_000));
        assert_eq!(credits.remaining, Some(0));
        assert_eq!(credits.reset, Some(secs(3600)));
        let tokens = i.bucket(LimitKind::TokensPerMinute).unwrap();
        assert_eq!(tokens.limit, Some(500_000));
        assert_eq!(
            tokens.remaining,
            Some(0),
            "the `Remainint` typo is accepted"
        );
        assert_eq!(tokens.reset, Some(secs(51)));
        // The daily budget refills after the 51s hint, so it gates the wait.
        assert_eq!(i.wait(), Some((secs(3600), LimitKind::DailyCredits)));
    }

    #[test]
    fn minute_bucket_only_waits_for_the_hint() {
        let headers = h(&[
            ("RetryAfter", "51"),
            ("X-RateLimit-Remaining-Credits-Day", "1500"),
            ("x-ratelimit-remaining-tokens-minute", "0"),
            ("x-ratelimit-reset-tokens-minute", "51"),
        ]);
        let i = RateLimitInfo::parse(&headers, BODY, SystemTime::now());
        assert_eq!(i.wait(), Some((secs(51), LimitKind::TokensPerMinute)));
    }

    #[test]
    fn correct_spelling_and_case_insensitive() {
        let headers = h(&[
            ("x-ratelimit-remaining-tokens-minute", "0"),
            ("X-RATELIMIT-RESET-TOKENS-MINUTE", "12"),
            ("x-ratelimit-limit-tokens-minute", "100"),
        ]);
        let i = RateLimitInfo::parse(&headers, "", SystemTime::now());
        assert_eq!(i.retry_after, None);
        assert_eq!(i.wait(), Some((secs(12), LimitKind::TokensPerMinute)));
    }

    #[test]
    fn retry_after_variants() {
        let now = SystemTime::now();
        let p = |k: &str, v: &str| RateLimitInfo::parse(&h(&[(k, v)]), "", now).retry_after;
        assert_eq!(p("Retry-After", "7"), Some(secs(7)));
        assert_eq!(p("retry-after", "0.5"), Some(Duration::from_millis(500)));
        assert_eq!(p("retry-after-ms", "250"), Some(Duration::from_millis(250)));
        assert_eq!(p("RetryAfter", "51"), Some(secs(51)));
        assert_eq!(p("Retry-After", "garbage"), None);
        // ms wins over seconds when both are present.
        let both = RateLimitInfo::parse(
            &h(&[("retry-after", "10"), ("retry-after-ms", "1500")]),
            "",
            now,
        );
        assert_eq!(both.retry_after, Some(Duration::from_millis(1500)));
    }

    #[test]
    fn retry_after_http_date() {
        let now = SystemTime::UNIX_EPOCH + secs(1_445_412_000); // Wed, 21 Oct 2015 07:20:00 GMT
        let i = RateLimitInfo::parse(
            &h(&[("Retry-After", "Wed, 21 Oct 2015 07:28:00 GMT")]),
            "",
            now,
        );
        assert_eq!(i.retry_after, Some(secs(480)));
        // A date in the past means "now".
        let past = RateLimitInfo::parse(
            &h(&[("Retry-After", "Wed, 21 Oct 2015 07:00:00 GMT")]),
            "",
            now,
        );
        assert_eq!(past.retry_after, Some(Duration::ZERO));
    }

    #[test]
    fn body_wins_over_headers() {
        let i = RateLimitInfo::parse(
            &h(&[("Retry-After", "10")]),
            r#"{"error":{"code":"x","retry_after_seconds": 3}}"#,
            SystemTime::now(),
        );
        assert_eq!(i.retry_after, Some(secs(3)));
    }

    #[test]
    fn missing_everything_means_no_wait() {
        let i = RateLimitInfo::parse(&[], "not json", SystemTime::now());
        assert!(i.is_empty());
        assert_eq!(i.wait(), None);
        assert_eq!(i.hit_kind(), LimitKind::Other);
    }

    #[test]
    fn body_code_classification() {
        let k = LimitKind::from_code;
        assert_eq!(
            k("tokens_per_minute_exceeded"),
            Some(LimitKind::TokensPerMinute)
        );
        assert_eq!(
            k("requests_per_minute_exceeded"),
            Some(LimitKind::RequestsPerMinute)
        );
        assert_eq!(k("daily_credits_exceeded"), Some(LimitKind::DailyCredits));
        assert_eq!(k("insufficient_quota"), Some(LimitKind::DailyCredits));
        assert_eq!(k("rate_limit_exceeded"), None);
    }

    #[test]
    fn anthropic_headers() {
        let now: SystemTime = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .into();
        let i = RateLimitInfo::parse(
            &h(&[
                ("anthropic-ratelimit-requests-limit", "50"),
                ("anthropic-ratelimit-requests-remaining", "0"),
                ("anthropic-ratelimit-requests-reset", "2030-01-01T00:00:30Z"),
                ("anthropic-ratelimit-tokens-remaining", "100"),
                ("retry-after", "30"),
            ]),
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"x"}}"#,
            now,
        );
        let r = i.bucket(LimitKind::RequestsPerMinute).unwrap();
        assert_eq!(r.limit, Some(50));
        assert_eq!(r.reset, Some(secs(30)));
        assert_eq!(i.wait(), Some((secs(30), LimitKind::RequestsPerMinute)));
    }

    #[test]
    fn openai_duration_resets() {
        let i = RateLimitInfo::parse(
            &h(&[
                ("x-ratelimit-limit-requests", "60"),
                ("x-ratelimit-remaining-requests", "0"),
                ("x-ratelimit-reset-requests", "1m30.5s"),
                ("x-ratelimit-remaining-tokens", "9000"),
                ("x-ratelimit-reset-tokens", "20ms"),
            ]),
            "",
            SystemTime::now(),
        );
        assert_eq!(
            i.bucket(LimitKind::RequestsPerMinute).unwrap().reset,
            Some(Duration::from_millis(90_500))
        );
        assert_eq!(
            i.bucket(LimitKind::TokensPerMinute).unwrap().reset,
            Some(Duration::from_millis(20))
        );
        assert_eq!(
            i.wait(),
            Some((Duration::from_millis(90_500), LimitKind::RequestsPerMinute))
        );
    }

    #[test]
    fn near_zero_counts_as_exhausted() {
        let b = |r, l| Bucket {
            kind: LimitKind::TokensPerMinute,
            limit: Some(l),
            remaining: Some(r),
            reset: None,
        };
        assert!(b(0, 100).is_exhausted());
        assert!(!b(5, 100).is_exhausted());
        assert!(b(4, 1000).is_exhausted());
    }
}
