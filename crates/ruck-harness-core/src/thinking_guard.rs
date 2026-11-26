//! Thinking-loop guard: detect a model that loops in its reasoning without ever answering or
//! calling a tool, so the request can be aborted before it fills the context window.
//!
//! Two independent triggers watch the streamed thinking of one request:
//!
//! * **Repetition**: the same (normalised) sentence recurs `repeat_count` times within the last
//!   `window_chars` characters, or, once a full window of sentences has been seen, the share of
//!   characters belonging to sentences never seen before in the request falls below `min_ratio`.
//! * **Budget**: the thinking of one request exceeds a token budget (`budget_tokens`, else
//!   `budget_fraction` of the profile's context window), estimated as 4 characters per token.
//!
//! Both only apply while the model has produced no text and no tool call in the request.

use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

/// Sentences shorter than this (normalised) are too generic to count as repetition.
const MIN_SENTENCE_CHARS: usize = 24;
/// Bound on distinct sentences remembered per request.
const MAX_TRACKED: usize = 4096;

/// `[thinking_guard]` in `config.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ThinkingGuardConfig {
    /// Turn the guard off entirely. Default true.
    pub enabled: bool,
    /// Trip when one sentence recurs this many times within `window_chars`. Default 4.
    pub repeat_count: u32,
    /// Size of the sliding window, in characters of thinking. Default 4000.
    pub window_chars: usize,
    /// Trip when, over a full window, less than this share of the sentence text is new.
    /// Default 0.2; 0 disables the ratio check.
    pub min_ratio: f64,
    /// Per-request thinking budget as a fraction of the profile's `context_window`.
    /// Default 0.25; 0 disables the budget.
    pub budget_fraction: f64,
    /// Absolute per-request thinking budget in tokens; overrides `budget_fraction`.
    pub budget_tokens: Option<u64>,
    /// How many times per turn a looping request is re-prompted with a nudge before the turn
    /// ends with stop reason `thinking_loop`. Default 2.
    pub max_nudges: u32,
}

impl Default for ThinkingGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repeat_count: 4,
            window_chars: 4000,
            min_ratio: 0.2,
            budget_fraction: 0.25,
            budget_tokens: None,
            max_nudges: 2,
        }
    }
}

/// `[profiles.<name>.thinking_guard]`: any key set here replaces the global value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ThinkingGuardOverride {
    pub enabled: Option<bool>,
    pub repeat_count: Option<u32>,
    pub window_chars: Option<usize>,
    pub min_ratio: Option<f64>,
    pub budget_fraction: Option<f64>,
    pub budget_tokens: Option<u64>,
    pub max_nudges: Option<u32>,
}

impl ThinkingGuardConfig {
    /// The global settings with a profile's overrides applied.
    pub fn with_override(&self, o: Option<&ThinkingGuardOverride>) -> Self {
        let mut c = self.clone();
        if let Some(o) = o {
            c.enabled = o.enabled.unwrap_or(c.enabled);
            c.repeat_count = o.repeat_count.unwrap_or(c.repeat_count);
            c.window_chars = o.window_chars.unwrap_or(c.window_chars);
            c.min_ratio = o.min_ratio.unwrap_or(c.min_ratio);
            c.budget_fraction = o.budget_fraction.unwrap_or(c.budget_fraction);
            c.budget_tokens = o.budget_tokens.or(c.budget_tokens);
            c.max_nudges = o.max_nudges.unwrap_or(c.max_nudges);
        }
        c
    }

    /// The per-request thinking budget in tokens, if any.
    pub fn budget(&self, context_window: u64) -> Option<u64> {
        match self.budget_tokens {
            Some(t) => Some(t),
            None if self.budget_fraction > 0.0 => {
                Some((context_window as f64 * self.budget_fraction) as u64)
            }
            None => None,
        }
    }
}

/// Sampling options for Ollama, `[profiles.<name>.ollama]`. Only profiles that set this section
/// get them; other providers are never sent anything extra.
///
/// Ollama's OpenAI-compatible endpoint (`/v1/chat/completions`) maps only `frequency_penalty` and
/// `presence_penalty` onto its sampler; `repeat_penalty` and `repeat_last_n` belong to the native
/// `options` object, which that endpoint ignores on versions that do not read it. They are sent
/// in an `options` object anyway (harmless when ignored); `frequency_penalty` and
/// `presence_penalty` are the dependable knobs through `/v1`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct OllamaOptions {
    pub repeat_penalty: Option<f64>,
    pub repeat_last_n: Option<i64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
}

impl OllamaOptions {
    /// Extra request-body fields, or `None` when nothing is set.
    pub fn extra_body(&self) -> Option<serde_json::Value> {
        use serde_json::{json, Map, Value};
        let mut top = Map::new();
        let mut options = Map::new();
        if let Some(v) = self.frequency_penalty {
            top.insert("frequency_penalty".into(), json!(v));
        }
        if let Some(v) = self.presence_penalty {
            top.insert("presence_penalty".into(), json!(v));
        }
        if let Some(v) = self.repeat_penalty {
            options.insert("repeat_penalty".into(), json!(v));
        }
        if let Some(v) = self.repeat_last_n {
            options.insert("repeat_last_n".into(), json!(v));
        }
        if !options.is_empty() {
            top.insert("options".into(), Value::Object(options));
        }
        (!top.is_empty()).then_some(Value::Object(top))
    }
}

/// Why the guard fired.
#[derive(Debug, Clone, PartialEq)]
pub enum Trip {
    Repetition,
    Budget,
}

impl Trip {
    pub fn describe(&self) -> &'static str {
        match self {
            Trip::Repetition => "repeating itself",
            Trip::Budget => "thinking past its budget",
        }
    }
}

/// Streaming detector for one model request.
pub struct LoopDetector {
    cfg: ThinkingGuardConfig,
    budget_chars: Option<usize>,
    /// Text since the last sentence boundary.
    pending: String,
    /// Total characters of thinking seen.
    total: usize,
    /// Offsets (in `total`) of recent occurrences, per sentence hash.
    recent: HashMap<u64, VecDeque<usize>>,
    /// Every sentence hash seen in the request (bounded).
    seen: HashMap<u64, ()>,
    /// Sentences inside the window: (end offset, chars, was new).
    window: VecDeque<(usize, usize, bool)>,
    /// A text or tool-call delta arrived: the model is acting, so the guard stands down.
    acted: bool,
}

impl LoopDetector {
    /// `context_window` is the profile's, for the default budget.
    pub fn new(cfg: &ThinkingGuardConfig, context_window: u64) -> Self {
        Self {
            budget_chars: cfg
                .budget(context_window)
                .map(|t| (t as usize).saturating_mul(4)),
            cfg: cfg.clone(),
            pending: String::new(),
            total: 0,
            recent: HashMap::new(),
            seen: HashMap::new(),
            window: VecDeque::new(),
            acted: false,
        }
    }

    /// The model produced text or a tool call: stop watching this request.
    pub fn mark_acted(&mut self) {
        self.acted = true;
    }

    /// Characters of thinking seen so far.
    pub fn thinking_chars(&self) -> usize {
        self.total
    }

    /// Feed one thinking delta; `Some` when the request should be aborted.
    pub fn feed(&mut self, delta: &str) -> Option<Trip> {
        if self.acted || !self.cfg.enabled {
            return None;
        }
        self.total += delta.chars().count();
        if self.budget_chars.is_some_and(|b| self.total > b) {
            return Some(Trip::Budget);
        }
        for ch in delta.chars() {
            if matches!(ch, '.' | '!' | '?' | '\n') {
                let sentence = std::mem::take(&mut self.pending);
                if let Some(t) = self.sentence(&sentence) {
                    return Some(t);
                }
            } else {
                self.pending.push(ch);
            }
        }
        None
    }

    fn sentence(&mut self, raw: &str) -> Option<Trip> {
        let norm: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let len = norm.chars().count();
        if len < MIN_SENTENCE_CHARS {
            return None;
        }
        let mut h = DefaultHasher::new();
        norm.hash(&mut h);
        let key = h.finish();
        if self.seen.len() >= MAX_TRACKED {
            self.seen.clear();
            self.recent.clear();
        }
        let is_new = self.seen.insert(key, ()).is_none();
        let floor = self.total.saturating_sub(self.cfg.window_chars);
        let occurrences = self.recent.entry(key).or_default();
        while occurrences.front().is_some_and(|&o| o < floor) {
            occurrences.pop_front();
        }
        occurrences.push_back(self.total);
        if self.cfg.repeat_count > 0 && occurrences.len() >= self.cfg.repeat_count as usize {
            return Some(Trip::Repetition);
        }
        self.window.push_back((self.total, len, is_new));
        while self.window.front().is_some_and(|w| w.0 < floor) {
            self.window.pop_front();
        }
        if self.cfg.min_ratio > 0.0 {
            let all: usize = self.window.iter().map(|w| w.1).sum();
            // Only judge a window that is substantially full of sentence text.
            if all >= self.cfg.window_chars / 2 {
                let fresh: usize = self.window.iter().filter(|w| w.2).map(|w| w.1).sum();
                if (fresh as f64) / (all as f64) < self.cfg.min_ratio {
                    return Some(Trip::Repetition);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(d: &mut LoopDetector, text: &str) -> Option<Trip> {
        // Stream in small chunks like a provider would.
        let chars: Vec<char> = text.chars().collect();
        for c in chars.chunks(7) {
            let s: String = c.iter().collect();
            if let Some(t) = d.feed(&s) {
                return Some(t);
            }
        }
        None
    }

    #[test]
    fn repeated_sentence_trips() {
        let mut d = LoopDetector::new(&ThinkingGuardConfig::default(), 65536);
        let text = "Actually, I'll also fix the any in src/service-worker.ts. ".repeat(10);
        assert_eq!(feed_all(&mut d, &text), Some(Trip::Repetition));
    }

    #[test]
    fn cycling_through_a_few_sentences_trips() {
        let mut d = LoopDetector::new(&ThinkingGuardConfig::default(), 65536);
        let text = "Actually, I will also fix the any in wire.ts. Wait, I will also fix the any in service-worker.ts. Let us go ahead and do that now. "
            .repeat(10);
        assert_eq!(feed_all(&mut d, &text), Some(Trip::Repetition));
    }

    #[test]
    fn long_varied_thinking_does_not_trip() {
        let mut d = LoopDetector::new(&ThinkingGuardConfig::default(), 65536);
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!(
                "Step number {i}: examine module {} and consider how function {} interacts with it. ",
                i * 7,
                i * 13
            ));
        }
        assert!(text.len() > 30_000);
        assert_eq!(feed_all(&mut d, &text), None);
    }

    #[test]
    fn short_generic_sentences_are_ignored() {
        let mut d = LoopDetector::new(&ThinkingGuardConfig::default(), 65536);
        assert_eq!(feed_all(&mut d, &"Let's go. ".repeat(500)), None);
    }

    #[test]
    fn budget_trips_on_non_repeating_thinking() {
        let cfg = ThinkingGuardConfig {
            budget_tokens: Some(100),
            ..Default::default()
        };
        let mut d = LoopDetector::new(&cfg, 65536);
        let mut text = String::new();
        for i in 0..50 {
            text.push_str(&format!(
                "Unique reasoning sentence about item {i} in the codebase. "
            ));
        }
        assert_eq!(feed_all(&mut d, &text), Some(Trip::Budget));
    }

    #[test]
    fn default_budget_is_a_fraction_of_the_window() {
        let cfg = ThinkingGuardConfig::default();
        assert_eq!(cfg.budget(65536), Some(16384));
        let off = ThinkingGuardConfig {
            budget_fraction: 0.0,
            ..Default::default()
        };
        assert_eq!(off.budget(65536), None);
    }

    #[test]
    fn acting_or_disabling_stands_the_guard_down() {
        let mut d = LoopDetector::new(&ThinkingGuardConfig::default(), 65536);
        d.mark_acted();
        assert_eq!(
            feed_all(
                &mut d,
                &"Actually, I will fix the any in wire.ts. ".repeat(20)
            ),
            None
        );
        let off = ThinkingGuardConfig {
            enabled: false,
            ..Default::default()
        };
        let mut d = LoopDetector::new(&off, 65536);
        assert_eq!(
            feed_all(
                &mut d,
                &"Actually, I will fix the any in wire.ts. ".repeat(20)
            ),
            None
        );
    }

    #[test]
    fn profile_override_replaces_only_set_keys() {
        let o = ThinkingGuardOverride {
            max_nudges: Some(5),
            ..Default::default()
        };
        let c = ThinkingGuardConfig::default().with_override(Some(&o));
        assert_eq!((c.max_nudges, c.repeat_count), (5, 4));
    }

    #[test]
    fn ollama_options_body() {
        assert_eq!(OllamaOptions::default().extra_body(), None);
        let o = OllamaOptions {
            repeat_penalty: Some(1.1),
            repeat_last_n: Some(256),
            presence_penalty: Some(0.5),
            ..Default::default()
        };
        let b = o.extra_body().unwrap();
        assert_eq!(b["options"]["repeat_penalty"], 1.1);
        assert_eq!(b["options"]["repeat_last_n"], 256);
        assert_eq!(b["presence_penalty"], 0.5);
    }
}
