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
//! Two further detectors catch loops whose arguments keep changing:
//!
//! * **Repeated search**: the search pattern of `Grep`/`Glob` calls and of `Bash` commands running
//!   grep, rg, ag, ack, `git grep` or `find -name` (see [`crate::search_pattern`]) is counted per
//!   turn, whatever the paths, flags or trailing filters. It trips when the pattern has been
//!   searched `search_repeat_count` times and the latest `search_repeat_count / 2` searches added
//!   no new information. "No new information" means: every non-empty (trimmed) line of the result
//!   (at most [`MAX_RESULT_LINES`]) was already returned by an earlier search for that pattern
//!   (errors and empty results add nothing). A refinement that finds any new line resets it.
//! * **Edit oscillation**: after each file-changing call the content hash of the affected file is
//!   recorded (see [`file_hash`], [`git_changed_files`]). A file returning to a content state it had
//!   earlier in the turn counts as one return; `oscillation_count` returns trip the detector.
//!
//! Plan updates that change nothing (`plan_noop_count` in a row) trip it as well; the plan tool
//! answers those locally (see [`plan_items`]).

use ruckcode_llm::{ContentBlock, Message};
use serde::Deserialize;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// At most this many result lines per search are compared.
pub const MAX_RESULT_LINES: usize = 2000;
/// Files larger than this are not hashed by the oscillation detector.
pub const MAX_HASH_FILE_BYTES: u64 = 1024 * 1024;
/// `git status` output larger than this (a huge or very dirty tree) disables the Bash snapshot.
const MAX_STATUS_BYTES: usize = 256 * 1024;
/// At most this many changed files are hashed per Bash call; more disables the snapshot.
const MAX_SNAPSHOT_FILES: usize = 200;
/// Total bytes hashed per Bash snapshot; more disables the snapshot.
const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Hash of a file that does not exist.
pub const ABSENT: u64 = 0;

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
    /// Detect searches repeating the same pattern without new results. Default true.
    pub search_repeat_enabled: bool,
    /// Trip when a pattern was searched this many times in a turn (and the latest searches found
    /// nothing new). Default 6; 0 disables the detector.
    pub search_repeat_count: u32,
    /// Detect files being changed back to an earlier content. Default true.
    pub oscillation_enabled: bool,
    /// Trip after a file returned to an earlier content this many times in a turn. Default 2;
    /// 0 disables the detector.
    pub oscillation_count: u32,
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
            search_repeat_enabled: true,
            search_repeat_count: 6,
            oscillation_enabled: true,
            oscillation_count: 2,
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
    pub search_repeat_enabled: Option<bool>,
    pub search_repeat_count: Option<u32>,
    pub oscillation_enabled: Option<bool>,
    pub oscillation_count: Option<u32>,
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
            c.search_repeat_enabled = o.search_repeat_enabled.unwrap_or(c.search_repeat_enabled);
            c.search_repeat_count = o.search_repeat_count.unwrap_or(c.search_repeat_count);
            c.oscillation_enabled = o.oscillation_enabled.unwrap_or(c.oscillation_enabled);
            c.oscillation_count = o.oscillation_count.unwrap_or(c.oscillation_count);
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
    Search { pattern: String, count: usize },
    Oscillation { path: String, returns: usize },
}

impl TripKind {
    /// `_meta.ruckcode.loopKind`: `repeat`, `search` or `oscillation`.
    pub fn loop_kind(&self) -> &'static str {
        match self {
            TripKind::Repeat { .. } | TripKind::PlanNoop { .. } | TripKind::NoProgress { .. } => {
                "repeat"
            }
            TripKind::Search { .. } => "search",
            TripKind::Oscillation { .. } => "oscillation",
        }
    }
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
            TripKind::Search { pattern, count } => {
                format!("searched for `{pattern}` {count} times with no new results")
            }
            TripKind::Oscillation { path, .. } => {
                format!("changed `{path}` back and forth between the same versions")
            }
        }
    }

    /// The message sent to the model for this trip.
    pub fn nudge(&self) -> String {
        match &self.kind {
            TripKind::Search { pattern, count } => format!(
                "You have searched for `{pattern}` {count} times with no new results. Use what \
                 you found, or take a different approach."
            ),
            TripKind::Oscillation { path, .. } => format!(
                "You are changing {path} back and forth between the same versions. Decide on one \
                 change, or answer."
            ),
            _ => "You are repeating the same tool call without progress. Take a different next \
                  step, or answer."
                .to_string(),
        }
    }
}

/// Per-pattern state of the repeated-search detector.
#[derive(Default)]
struct SearchState {
    count: usize,
    /// Consecutive latest searches that added nothing new.
    stale_streak: usize,
    seen_lines: HashSet<u64>,
    stale_ids: Vec<String>,
}

/// Per-file state of the oscillation detector.
struct FileHistory {
    current: u64,
    seen: HashSet<u64>,
    returns: usize,
}

impl FileHistory {
    fn new(hash: u64) -> Self {
        Self {
            current: hash,
            seen: HashSet::from([hash]),
            returns: 0,
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
    searches: HashMap<String, SearchState>,
    files: HashMap<String, FileHistory>,
    /// Whether the working-tree baseline for Bash was taken this turn.
    pub bash_baseline_done: bool,
    /// Set when the git snapshot is unavailable or too costly; Bash is not tracked afterwards.
    pub bash_snapshots_off: bool,
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
        let result_text = result;
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

        self.observe_search(cfg, id, tool, input, result_text, is_error);
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

    fn observe_search(
        &mut self,
        cfg: &ToolLoopGuardConfig,
        id: &str,
        tool: &str,
        input: &Value,
        result: &str,
        is_error: bool,
    ) {
        if !cfg.search_repeat_enabled || cfg.search_repeat_count == 0 || self.trip.is_some() {
            return;
        }
        let Some(pattern) = crate::search_pattern::search_pattern(tool, input) else {
            return;
        };
        let lines: Vec<u64> = if is_error {
            vec![]
        } else {
            result
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(MAX_RESULT_LINES)
                .map(|l| hash_of(&l))
                .collect()
        };
        let st = self.searches.entry(pattern.clone()).or_default();
        st.count += 1;
        let before = st.seen_lines.len();
        st.seen_lines.extend(lines);
        if st.count > 1 && st.seen_lines.len() == before {
            st.stale_streak += 1;
            st.stale_ids.push(id.to_string());
        } else {
            st.stale_streak = 0;
        }
        let need = (cfg.search_repeat_count as usize / 2).max(1);
        if st.count >= cfg.search_repeat_count as usize && st.stale_streak >= need {
            let drop_ids = st.stale_ids.iter().cloned().collect();
            let count = st.count;
            self.searches.remove(&pattern);
            self.trip = Some(ToolTrip {
                kind: TripKind::Search { pattern, count },
                drop_ids,
            });
        }
    }

    /// Paths with a recorded content history this turn.
    pub fn tracked_paths(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    /// Record the content hash of a file that is about to change (the baseline), unless the
    /// file already has a history this turn.
    pub fn baseline_file(&mut self, path: &str, hash: u64) {
        self.files
            .entry(path.to_string())
            .or_insert_with(|| FileHistory::new(hash));
    }

    /// Record the content hash of a file after a changing call. A return to an earlier state
    /// counts; the history is kept for the rest of the turn.
    pub fn observe_file(&mut self, cfg: &ToolLoopGuardConfig, path: &str, hash: u64) {
        if !cfg.enabled || !cfg.oscillation_enabled || cfg.oscillation_count == 0 {
            return;
        }
        let h = self
            .files
            .entry(path.to_string())
            .or_insert_with(|| FileHistory::new(hash));
        if h.current == hash {
            return;
        }
        h.current = hash;
        if !h.seen.insert(hash) {
            h.returns += 1;
        }
        if h.returns >= cfg.oscillation_count as usize && self.trip.is_none() {
            self.trip = Some(ToolTrip {
                kind: TripKind::Oscillation {
                    path: path.to_string(),
                    returns: h.returns,
                },
                drop_ids: HashSet::new(),
            });
        }
    }

    /// The trip recorded since the last call, if any; the run state restarts afterwards.
    pub fn take_trip(&mut self) -> Option<ToolTrip> {
        let trip = self.trip.take()?;
        self.run.clear();
        self.noop_ids.clear();
        self.window.clear();
        self.redundant_ids.clear();
        for h in self.files.values_mut() {
            h.returns = 0;
        }
        Some(trip)
    }
}

/// Tools that change one file, and the argument holding its path.
pub fn edit_target(tool: &str, input: &Value) -> Option<String> {
    if !matches!(tool, "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        return None;
    }
    ["file_path", "path", "notebook_path"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .map(str::to_string)
}

/// Content hash of a file: [`ABSENT`] if it does not exist, `None` if unreadable or larger than
/// [`MAX_HASH_FILE_BYTES`].
pub fn file_hash(path: &Path) -> Option<u64> {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(ABSENT),
        Err(_) => None,
        Ok(m) if !m.is_file() || m.len() > MAX_HASH_FILE_BYTES => None,
        Ok(_) => Some(hash_of(&std::fs::read(path).ok()?).max(1)),
    }
}

async fn git(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = tokio::time::timeout(
        GIT_TIMEOUT,
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    (out.status.success() && out.stdout.len() <= MAX_STATUS_BYTES).then_some(out.stdout)
}

/// Absolute paths that git reports as modified or untracked in the repository containing `cwd`,
/// or `None` if `cwd` is not in a git repository, git is slow or missing, or the tree is too
/// dirty to look at cheaply (status output over 256 KiB, or more than 200 files).
pub async fn git_changed_files(cwd: &Path) -> Option<Vec<PathBuf>> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).await?;
    let root = PathBuf::from(String::from_utf8_lossy(&root).trim_end());
    let status = git(
        cwd,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
    )
    .await?;
    let files: Vec<PathBuf> = status
        .split(|b| *b == 0)
        .filter(|e| e.len() > 3)
        .map(|e| root.join(String::from_utf8_lossy(&e[3..]).as_ref()))
        .collect();
    (files.len() <= MAX_SNAPSHOT_FILES).then_some(files)
}

/// Content hashes of `paths` that can be hashed cheaply; `None` if together they exceed 8 MiB.
pub fn hash_files(paths: &[PathBuf]) -> Option<Vec<(String, u64)>> {
    let mut total = 0u64;
    let mut out = Vec::new();
    for p in paths {
        if let Ok(m) = std::fs::metadata(p) {
            total += m.len().min(MAX_HASH_FILE_BYTES);
            if total > MAX_SNAPSHOT_BYTES {
                return None;
            }
        }
        if let Some(h) = file_hash(p) {
            out.push((p.to_string_lossy().into_owned(), h));
        }
    }
    Some(out)
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
    fn search_trips_only_when_recent_results_add_nothing() {
        let cfg = ToolLoopGuardConfig::default();
        let grep =
            |i: usize| json!({"command": format!("grep -r \"foo bar\" dir{i}/ | head -{i}")});
        let mut d = ToolLoopDetector::default();
        // Every search finds something new: never trips.
        for i in 0..10 {
            d.observe(
                &cfg,
                &format!("n{i}"),
                "Bash",
                &grep(i),
                &format!("hit {i}"),
                false,
                false,
            );
        }
        assert!(d.take_trip().is_none());
        // Same pattern, results a subset of what was seen: trips at 6 and drops the stale ones.
        let mut d = ToolLoopDetector::default();
        d.observe(&cfg, "s0", "Bash", &grep(0), "a\nb\nc", false, false);
        for i in 1..6 {
            assert!(d.take_trip().is_none());
            d.observe(
                &cfg,
                &format!("s{i}"),
                "Bash",
                &grep(i),
                "b\nc",
                false,
                false,
            );
        }
        let t = d.take_trip().expect("trips");
        assert_eq!(
            t.kind,
            TripKind::Search {
                pattern: "foo bar".into(),
                count: 6
            }
        );
        assert_eq!(t.kind.loop_kind(), "search");
        assert!(!t.drop_ids.contains("s0") && t.drop_ids.contains("s5"));
        assert!(t.nudge().contains("`foo bar` 6 times with no new results"));
    }

    #[test]
    fn oscillation_counts_returns_to_earlier_states() {
        let cfg = ToolLoopGuardConfig::default();
        let mut d = ToolLoopDetector::default();
        d.baseline_file("f", 1);
        for h in [2, 3, 1] {
            d.observe_file(&cfg, "f", h);
        }
        assert!(d.take_trip().is_none(), "one revert is fine");
        d.observe_file(&cfg, "f", 2);
        let t = d.take_trip().expect("second return trips");
        assert_eq!(
            t.kind,
            TripKind::Oscillation {
                path: "f".into(),
                returns: 2
            }
        );
        assert!(t.drop_ids.is_empty() && t.kind.loop_kind() == "oscillation");
        // Same content again is not a change; a fresh state is progress.
        d.observe_file(&cfg, "f", 2);
        d.observe_file(&cfg, "f", 9);
        assert!(d.take_trip().is_none());
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
