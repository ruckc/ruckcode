//! Keeping every model request inside the context window.
//!
//! Three mechanisms work together (see `[context]` in the README):
//!
//! * a per-result cap: every tool result (MCP included) is cut to head and tail before it enters
//!   the history;
//! * automatic compaction before **every** request of a turn, not only at its start; the latest
//!   tool calls and results of the turn in flight survive the compaction;
//! * a hard ceiling: prompt estimate plus the output reserve never exceeds the window; the
//!   largest tool results of the turn are truncated until the request fits.

use ruck_harness_llm::{ContentBlock, Message, Role, ToolSpec};
use serde::Deserialize;

/// Token estimates assume this many bytes per token (conservative for code and JSON).
pub const BYTES_PER_TOKEN: usize = 3;
/// A tool result is never shrunk below this many bytes by the ceiling.
const MIN_RESULT_BYTES: usize = 1000;

/// `[context]` in `config.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Compact when the prompt estimate exceeds this share of the context window. Unset: the
    /// top-level `compaction_threshold` (default 0.8).
    pub compact_threshold: Option<f64>,
    /// Largest tool result, in tokens (estimated). The effective cap is the smaller of this and
    /// `max_tool_result_fraction` of the window. Default 12000; 0 disables the cap.
    pub max_tool_result_tokens: u64,
    /// Largest tool result as a share of the context window. Default 0.10.
    pub max_tool_result_fraction: f64,
    /// Tokens kept free for the model's answer (and thinking). Unset: the smaller of the
    /// profile's `max_tokens` and a quarter of the window.
    pub output_reserve_tokens: Option<u64>,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            compact_threshold: None,
            max_tool_result_tokens: 12_000,
            max_tool_result_fraction: 0.10,
            output_reserve_tokens: None,
        }
    }
}

/// `[profiles.<name>.context]`: any key set here replaces the global value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ContextOverride {
    pub compact_threshold: Option<f64>,
    pub max_tool_result_tokens: Option<u64>,
    pub max_tool_result_fraction: Option<f64>,
    pub output_reserve_tokens: Option<u64>,
}

impl ContextConfig {
    /// The global settings with a profile's overrides applied.
    pub fn with_override(&self, o: Option<&ContextOverride>) -> Self {
        let mut c = self.clone();
        if let Some(o) = o {
            c.compact_threshold = o.compact_threshold.or(c.compact_threshold);
            c.max_tool_result_tokens = o.max_tool_result_tokens.unwrap_or(c.max_tool_result_tokens);
            c.max_tool_result_fraction = o
                .max_tool_result_fraction
                .unwrap_or(c.max_tool_result_fraction);
            c.output_reserve_tokens = o.output_reserve_tokens.or(c.output_reserve_tokens);
        }
        c
    }
}

/// The limits of one model's requests, in tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub window: u64,
    /// Compact when the prompt estimate exceeds this.
    pub compact_at: u64,
    /// Kept free for the answer; prompt estimate plus this must stay within the window.
    pub reserve: u64,
    /// Largest tool result in bytes; `None` when uncapped.
    pub result_cap_bytes: Option<usize>,
}

impl Limits {
    pub fn new(cfg: &ContextConfig, global_threshold: f64, window: u64, max_tokens: u32) -> Self {
        let reserve = cfg
            .output_reserve_tokens
            .unwrap_or_else(|| u64::from(max_tokens).min(window / 4))
            .min(window / 2);
        let threshold = cfg.compact_threshold.unwrap_or(global_threshold);
        let by_threshold = (threshold * window as f64) as u64;
        let compact_at = by_threshold.min(window.saturating_sub(reserve));
        let cap_tokens = (cfg.max_tool_result_fraction * window as f64) as u64;
        let cap_tokens = cap_tokens.min(cfg.max_tool_result_tokens);
        let result_cap_bytes = (cfg.max_tool_result_tokens > 0)
            .then(|| (cap_tokens as usize * BYTES_PER_TOKEN).max(MIN_RESULT_BYTES));
        Self {
            window,
            compact_at,
            reserve,
            result_cap_bytes,
        }
    }

    /// Whether a prompt of `estimate` tokens plus the reserve fits the window.
    pub fn fits(&self, estimate: u64) -> bool {
        estimate + self.reserve <= self.window
    }
}

/// Estimated prompt tokens of a request.
pub fn estimate_tokens(system: &str, tools: &[ToolSpec], messages: &[Message]) -> u64 {
    let msgs = serde_json::to_string(messages).map_or(0, |j| j.len());
    let tools = serde_json::to_string(tools).map_or(0, |j| j.len());
    ((msgs + tools + system.len()) / BYTES_PER_TOKEN) as u64
}

/// Keep the head and tail of `s` within `max` bytes, with a marker naming what was cut.
pub fn truncate_head_tail(s: &str, max: usize) -> String {
    ruck_harness_tools::truncate_middle(s, max, &crate::identity::RUCKCODE)
}

/// Index of the first message of the in-flight exchange: the last assistant tool calls and the
/// results that answer them (or the final user message). Messages from this index on survive a
/// compaction. `messages.len()` when there is nothing to keep.
pub fn in_flight_start(messages: &[Message]) -> usize {
    let n = messages.len();
    let Some(last) = messages.last() else {
        return 0;
    };
    if last.role != Role::User {
        return n;
    }
    let has_results = last
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
    if has_results && n >= 2 && messages[n - 2].role == Role::Assistant {
        n - 2
    } else {
        n - 1
    }
}

/// Index of the first message of the current turn: the last user message with text.
pub fn turn_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|m| {
            m.role == Role::User
                && m.content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { .. }))
        })
        .unwrap_or(0)
}

/// Truncate the largest tool result at or after `from` to half its size (never below
/// [`MIN_RESULT_BYTES`]). Returns false when nothing can shrink further.
pub fn shrink_largest_result(messages: &mut [Message], from: usize) -> bool {
    let mut best: Option<(usize, usize, usize)> = None;
    for (mi, m) in messages.iter().enumerate().skip(from) {
        for (bi, b) in m.content.iter().enumerate() {
            if let ContentBlock::ToolResult { content, .. } = b {
                if content.len() > MIN_RESULT_BYTES * 2
                    && best.is_none_or(|(_, _, len)| content.len() > len)
                {
                    best = Some((mi, bi, content.len()));
                }
            }
        }
    }
    let Some((mi, bi, len)) = best else {
        return false;
    };
    if let ContentBlock::ToolResult { content, .. } = &mut messages[mi].content[bi] {
        *content = truncate_head_tail(content, (len / 2).max(MIN_RESULT_BYTES));
    }
    true
}

/// Shrink tool results (the turn's first, then any) until `estimate()` plus the reserve fits.
/// Returns how many shrink steps ran and whether the request fits now.
pub fn fit_to_ceiling(
    messages: &mut [Message],
    limits: &Limits,
    turn_from: usize,
    estimate: impl Fn(&[Message]) -> u64,
) -> (usize, bool) {
    let mut steps = 0;
    let mut from = turn_from;
    loop {
        if limits.fits(estimate(messages)) {
            return (steps, true);
        }
        if shrink_largest_result(messages, from) {
            steps += 1;
        } else if from > 0 {
            from = 0;
        } else {
            return (steps, false);
        }
    }
}
