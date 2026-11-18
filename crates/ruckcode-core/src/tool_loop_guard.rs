//! Tool-loop guard: detect a model that keeps repeating tool calls without making progress, so
//! the repeats can be dropped from the history before they keep inflating the context.
//!
//! Per turn, two independent triggers watch the executed tool calls:
//!
//! * **Repeat**: `repeat_count` calls in a row of the same tool with identical or near-identical
//!   arguments (canonical JSON; long string arguments may differ by under 5%) that all returned
//!   the same result. Another tool call or assistant text in between ends the run. A repeated
//!   command whose output changes (a test run after an edit) is progress and never counts.
//! * **No progress**: over a sliding window of the last `window` calls, at least
//!   `max_noprogress_ratio` of the calls were redundant, i.e. same tool, same normalised
//!   arguments and same result as an earlier call in the turn.
//!
//! Plan updates that change nothing (`plan_noop_count` in a row) trip it as well; the plan tool
//! answers those locally (see [`plan_items`]).

use ruckcode_llm::{ContentBlock, Message};
use serde::Deserialize;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// Result of a plan update that changed nothing; the tool is not executed.
pub const PLAN_UNCHANGED: &str = "Plan unchanged (no items or statuses changed).";
/// Strings shorter than this must match exactly to count as near-identical.
const MIN_FUZZY_CHARS: usize = 20;
/// Share of the longer string that must be shared prefix plus suffix to be near-identical.
const FUZZY_SHARE: f64 = 0.95;

/// `[tool_loop_guard]` in `config.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToolLoopGuardConfig {
    /// Turn the guard (and the plan no-op short-circuit) off entirely. Default true.
    pub enabled: bool,
    /// Trip after this many consecutive same-tool, same-arguments, same-result calls. Default 4.
    pub repeat_count: u32,
    /// Trip after this many consecutive plan updates that changed nothing. Default 3.
    pub plan_noop_count: u32,
    /// Size of the sliding window, in tool calls. Default 10; 0 disables the ratio check.
    pub window: usize,
    /// Trip when at least this share of a full window is redundant. Default 0.6.
    pub max_noprogress_ratio: f64,
    /// How many times per turn a loop is dropped and nudged before the turn ends with stop
    /// reason `tool_loop`. Default 2.
    pub max_nudges: u32,
}

impl Default for ToolLoopGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repeat_count: 4,
            plan_noop_count: 3,
            window: 10,
            max_noprogress_ratio: 0.6,
            max_nudges: 2,
        }
    }
}

/// `[profiles.<name>.tool_loop_guard]`: any key set here replaces the global value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToolLoopGuardOverride {
    pub enabled: Option<bool>,
    pub repeat_count: Option<u32>,
    pub plan_noop_count: Option<u32>,
    pub window: Option<usize>,
    pub max_noprogress_ratio: Option<f64>,
    pub max_nudges: Option<u32>,
}

impl ToolLoopGuardConfig {
    /// The global settings with a profile's overrides applied.
    pub fn with_override(&self, o: Option<&ToolLoopGuardOverride>) -> Self {
        let mut c = self.clone();
        if let Some(o) = o {
            c.enabled = o.enabled.unwrap_or(c.enabled);
            c.repeat_count = o.repeat_count.unwrap_or(c.repeat_count);
            c.plan_noop_count = o.plan_noop_count.unwrap_or(c.plan_noop_count);
            c.window = o.window.unwrap_or(c.window);
            c.max_noprogress_ratio = o.max_noprogress_ratio.unwrap_or(c.max_noprogress_ratio);
            c.max_nudges = o.max_nudges.unwrap_or(c.max_nudges);
        }
        c
    }
}

/// Why the guard tripped.
#[derive(Debug, Clone, PartialEq)]
pub enum TripKind {
    Repeat { tool: String, count: usize },
    PlanNoop { count: usize },
    NoProgress { redundant: usize, window: usize },
}

/// A tripped guard: what was seen and which tool calls to drop from the history.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolTrip {
    pub kind: TripKind,
    pub drop_ids: HashSet<String>,
}

impl ToolTrip {
    pub fn describe(&self) -> String {
        match &self.kind {
            TripKind::Repeat { tool, count } => {
                format!("called `{tool}` {count} times in a row with the same arguments and result")
            }
            TripKind::PlanNoop { count } => {
                format!("sent {count} plan updates in a row that changed nothing")
            }
            TripKind::NoProgress { redundant, window } => {
                format!("made {redundant} of its last {window} tool calls with no new result")
            }
        }
    }
}

struct Call {
    tool: String,
    args: Value,
    result: u64,
}

/// Per-turn state of the guard.
#[derive(Default)]
pub struct ToolLoopDetector {
    /// The current run of repeats (the first call of the run is kept in history).
    run: Vec<(String, Call)>,
    noop_ids: Vec<String>,
    /// (tool, args, result) fingerprints of every call so far in the turn.
    seen: HashSet<u64>,
    window: VecDeque<bool>,
    redundant_ids: Vec<String>,
    trip: Option<ToolTrip>,
}

impl ToolLoopDetector {
    /// Assistant text, or any non-call content, ends a run of consecutive calls.
    pub fn note_text(&mut self) {
        self.run.clear();
        self.noop_ids.clear();
    }

    /// Record one executed tool call with its `result`. `noop` marks a plan update that changed
    /// nothing. The first trip is kept for [`take_trip`](Self::take_trip).
    #[allow(clippy::too_many_arguments)]
    pub fn observe(
        &mut self,
        cfg: &ToolLoopGuardConfig,
        id: &str,
        tool: &str,
        input: &Value,
        result: &str,
        is_error: bool,
        noop: bool,
    ) {
        if !cfg.enabled {
            return;
        }
        let args = canonical(input);
        let result = hash_of(&(result, is_error));
        let key = hash_of(&(tool, args.to_string(), result));
        let redundant = !self.seen.insert(key);
        if redundant {
            self.redundant_ids.push(id.to_string());
        }
        self.window.push_back(redundant);
        while self.window.len() > cfg.window.max(1) {
            self.window.pop_front();
        }

        if noop {
            self.noop_ids.push(id.to_string());
        } else {
            self.noop_ids.clear();
        }
        let call = Call {
            tool: tool.to_string(),
            args,
            result,
        };
        let continues = self
            .run
            .last()
            .is_some_and(|(_, last)| last.tool == call.tool && last.result == call.result)
            && self
                .run
                .first()
                .is_some_and(|(_, first)| similar(&first.args, &call.args));
        if !continues {
            self.run.clear();
        }
        self.run.push((id.to_string(), call));

        if self.trip.is_some() {
            return;
        }
        let kind = if cfg.plan_noop_count > 0 && self.noop_ids.len() >= cfg.plan_noop_count as usize
        {
            Some(TripKind::PlanNoop {
                count: self.noop_ids.len(),
            })
        } else if cfg.repeat_count > 0 && self.run.len() >= cfg.repeat_count as usize {
            Some(TripKind::Repeat {
                tool: tool.to_string(),
                count: self.run.len(),
            })
        } else if cfg.window > 0 && self.window.len() >= cfg.window {
            let n = self.window.iter().filter(|r| **r).count();
            (n > 0 && n as f64 >= cfg.max_noprogress_ratio * cfg.window as f64).then_some(
                TripKind::NoProgress {
                    redundant: n,
                    window: self.window.len(),
                },
            )
        } else {
            None
        };
        if let Some(kind) = kind {
            let mut drop_ids: HashSet<String> = self.redundant_ids.iter().cloned().collect();
            drop_ids.extend(self.noop_ids.iter().cloned());
            drop_ids.extend(self.run.iter().skip(1).map(|(id, _)| id.clone()));
            self.trip = Some(ToolTrip { kind, drop_ids });
        }
    }

    /// The trip recorded since the last call, if any; the run state restarts afterwards.
    pub fn take_trip(&mut self) -> Option<ToolTrip> {
        let trip = self.trip.take()?;
        self.run.clear();
        self.noop_ids.clear();
        self.window.clear();
        self.redundant_ids.clear();
        Some(trip)
    }
}

fn hash_of<T: Hash>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Canonical form of tool arguments: object keys sorted, strings trimmed. (Formatting
/// whitespace never reaches a parsed `Value`.)
pub fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonical(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        Value::String(s) => Value::String(s.trim().to_string()),
        other => other.clone(),
    }
}

/// Identical, or equal except that long strings may differ in a few characters in the middle.
fn similar(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => {
            if x == y {
                return true;
            }
            let (x, y): (Vec<char>, Vec<char>) = (x.chars().collect(), y.chars().collect());
            let shortest = x.len().min(y.len());
            if shortest < MIN_FUZZY_CHARS {
                return false;
            }
            let prefix = x.iter().zip(&y).take_while(|(p, q)| p == q).count();
            let suffix = x
                .iter()
                .rev()
                .zip(y.iter().rev())
                .take_while(|(p, q)| p == q)
                .count()
                .min(shortest - prefix);
            (prefix + suffix) as f64 >= FUZZY_SHARE * x.len().max(y.len()) as f64
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| similar(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| similar(p, q))
        }
        _ => a == b,
    }
}

/// The plan's (content, status) pairs from `TodoWrite` input, normalised for comparison
/// (whitespace collapsed, case ignored). `None` if the input is not a valid plan.
pub fn plan_items(input: &Value) -> Option<Vec<(String, String)>> {
    input["todos"]
        .as_array()?
        .iter()
        .map(|t| {
            let content = t["content"]
                .as_str()?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            Some((content, t["status"].as_str()?.to_string()))
        })
        .collect()
}

/// Remove the tool calls `ids` (and their results) from a message history, dropping messages
/// left empty. Roles that end up adjacent are merged by `normalize_messages` when sent.
pub fn drop_tool_calls(messages: &mut Vec<Message>, ids: &HashSet<String>) {
    for m in messages.iter_mut() {
        m.content.retain(|b| match b {
            ContentBlock::ToolUse { id, .. } => !ids.contains(id),
            ContentBlock::ToolResult { tool_use_id, .. } => !ids.contains(tool_use_id),
            _ => true,
        });
    }
    messages.retain(|m| !m.content.is_empty());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obs(d: &mut ToolLoopDetector, cfg: &ToolLoopGuardConfig, i: usize, input: Value, r: &str) {
        d.observe(cfg, &format!("c{i}"), "Read", &input, r, false, false);
    }

    #[test]
    fn identical_calls_and_results_trip_and_keep_the_first() {
        let cfg = ToolLoopGuardConfig::default();
        let mut d = ToolLoopDetector::default();
        for i in 0..4 {
            obs(&mut d, &cfg, i, json!({"path": "a", "x": 1}), "same");
        }
        let t = d.take_trip().expect("trips");
        assert!(matches!(t.kind, TripKind::Repeat { count: 4, .. }));
        assert!(!t.drop_ids.contains("c0") && t.drop_ids.contains("c3"));
    }

    #[test]
    fn key_order_is_ignored_and_changing_results_or_args_are_progress() {
        let cfg = ToolLoopGuardConfig::default();
        let mut d = ToolLoopDetector::default();
        for i in 0..3 {
            obs(&mut d, &cfg, i, json!({"a": 1, "b": 2}), "r");
            obs(&mut d, &cfg, i + 10, json!({"b": 2, "a": 1}), "r");
        }
        assert!(d.take_trip().is_some());
        let mut d = ToolLoopDetector::default();
        for i in 0..20 {
            obs(&mut d, &cfg, i, json!({"path": "a"}), &format!("out {i}"));
            obs(&mut d, &cfg, i + 100, json!({"path": format!("f{i}")}), "x");
        }
        assert!(d.take_trip().is_none());
    }

    #[test]
    fn near_identical_long_strings_count() {
        let long = "x".repeat(200);
        assert!(similar(
            &json!({"q": format!("{long}a")}),
            &json!({"q": format!("{long}b")})
        ));
        assert!(!similar(&json!({"q": "abc"}), &json!({"q": "abd"})));
    }

    #[test]
    fn drop_removes_calls_and_empty_messages() {
        let mut msgs = vec![
            Message::user_text("hi"),
            Message {
                role: ruckcode_llm::Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "Read".into(),
                    input: json!({}),
                }],
            },
        ];
        drop_tool_calls(&mut msgs, &HashSet::from(["c1".to_string()]));
        assert_eq!(msgs.len(), 1);
    }
}
