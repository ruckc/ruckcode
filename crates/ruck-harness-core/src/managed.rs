//! Model-managed context: tagged history, collapse and recall.
//!
//! Adapted from "Letting Models Manage Their Own Context: Where We Stand" (arXiv:2611.34147).
//! With `[context] model_managed = true` the model sees a usage line and a tag on every history
//! event, and may replace a contiguous range of events with its own summary (`Context`
//! `action=collapse`); the originals are archived and can be read back (`action=recall`). The
//! harness's automatic compaction and the hard ceiling remain the safety net (see
//! [`crate::context`]).
//!
//! # Tags
//!
//! An *event* is one history message. Its tag `eN` is the 1-based position of the message's
//! record among the transcript's `user_message` and `assistant_message` records, so tags are a
//! function of the transcript: they never change when history is collapsed, compacted or
//! trimmed, and resuming a session reproduces them exactly. A collapse leaves one summary
//! message tagged with the range it replaced (`e3..e17`); the compaction summary of automatic
//! compaction carries no tag.
//!
//! Tags are shown to the model by labelling a copy of the history for each request; the stored
//! history is never edited, so labels cost nothing in the transcript and stay identical from
//! request to request (stable for prompt caching).
//!
//! # Collapse rules
//!
//! The range must hold whole exchanges: every tool call in it has its result in it and vice
//! versa (a range that splits a pair is rejected, with the nearest valid range suggested). The
//! first message of the history (the task), the current request (the last user message with
//! text) and the exchange in flight (the tool calls being run now, or the final message) can
//! never be collapsed.

use ruck_harness_llm::{ContentBlock, Message, Role};
use ruck_harness_tools::TagRef;
use std::collections::{BTreeMap, HashSet};

use crate::context::{in_flight_start, turn_start, Limits, BYTES_PER_TOKEN};

/// What a history message is, for tagging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// Untagged: the summary of automatic compaction.
    None,
    /// One event, `eN`.
    Event(u64),
    /// A collapse summary standing for the events `a..b`.
    Summary(u64, u64),
}

/// The original messages of collapsed events, by event number. Never edited, only added to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Archive {
    events: BTreeMap<u64, Message>,
}

impl Archive {
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// What a collapse did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collapse {
    /// The summary's tag, e.g. `e3..e17`.
    pub tag: String,
    pub from: u64,
    pub to: u64,
    /// History messages replaced by the one summary.
    pub replaced: usize,
    /// Estimated tokens the history shrank by.
    pub freed_tokens: u64,
    /// Why the range differs from the one asked for, if it does.
    pub note: Option<String>,
}

/// The history message standing for a collapsed range.
pub fn collapsed_message(label: &str, summary: &str) -> Message {
    Message::user_text(format!(
        "[{label}] You collapsed these events into the summary below (the originals are \
         archived; read them with the Context tool, action=recall, tag={label}).\n{}",
        summary.trim()
    ))
}

fn tool_ids(m: &Message) -> (Vec<&str>, Vec<&str>) {
    let mut uses = vec![];
    let mut results = vec![];
    for b in &m.content {
        match b {
            ContentBlock::ToolUse { id, .. } => uses.push(id.as_str()),
            ContentBlock::ToolResult { tool_use_id, .. } => results.push(tool_use_id.as_str()),
            _ => {}
        }
    }
    (uses, results)
}

/// Whether `messages[i..=j]` holds every tool call together with its result and nothing else's.
fn is_closed(messages: &[Message], i: usize, j: usize) -> bool {
    let mut uses = HashSet::new();
    let mut results = HashSet::new();
    for m in &messages[i..=j] {
        let (u, r) = tool_ids(m);
        uses.extend(u);
        results.extend(r);
    }
    uses == results
}

/// The smallest closed range containing `i..=j`, if one exists in the history.
fn expand(messages: &[Message], mut i: usize, mut j: usize) -> Option<(usize, usize)> {
    loop {
        if is_closed(messages, i, j) {
            return Some((i, j));
        }
        let mut uses = HashSet::new();
        let mut results = HashSet::new();
        for m in &messages[i..=j] {
            let (u, r) = tool_ids(m);
            uses.extend(u);
            results.extend(r);
        }
        if uses.iter().any(|u| !results.contains(u)) {
            j += 1;
            if j >= messages.len() {
                return None;
            }
        } else {
            if i == 0 {
                return None;
            }
            i -= 1;
        }
    }
}

fn find_start(tags: &[Tag], n: u64) -> Option<usize> {
    tags.iter()
        .position(|t| matches!(t, Tag::Event(e) | Tag::Summary(e, _) if *e == n))
}

fn find_end(tags: &[Tag], n: u64) -> Option<usize> {
    tags.iter().position(|t| match t {
        Tag::Event(e) => *e == n,
        Tag::Summary(_, e) => *e == n,
        Tag::None => false,
    })
}

fn live_span(tags: &[Tag]) -> String {
    let (mut lo, mut hi) = (u64::MAX, 0);
    for t in tags {
        let (a, b) = match t {
            Tag::Event(e) => (*e, *e),
            Tag::Summary(a, b) => (*a, *b),
            Tag::None => continue,
        };
        lo = lo.min(a);
        hi = hi.max(b);
    }
    if hi == 0 {
        "the history holds no tagged events".into()
    } else {
        format!("the history holds e{lo}..e{hi}")
    }
}

fn locate_end(tags: &[Tag], n: u64, what: &str) -> Result<usize, String> {
    let found = if what == "from" {
        find_start(tags, n)
    } else {
        find_end(tags, n)
    };
    if let Some(i) = found {
        return Ok(i);
    }
    for t in tags {
        if let Tag::Summary(a, b) = t {
            if (*a..=*b).contains(&n) {
                return Err(format!(
                    "`{what}` e{n} lies inside the collapsed range e{a}..e{b}; use e{a}..e{b} as \
                     the {} of your range (or recall it to read it)",
                    if what == "from" { "start" } else { "end" }
                ));
            }
        }
    }
    Err(format!(
        "`{what}` e{n} is not in the history now (it was collapsed, compacted or dropped, or \
         does not exist); {}",
        live_span(tags)
    ))
}

/// Resolve the endpoints `from` and `to` to message indices.
fn resolve(tags: &[Tag], from: u64, to: u64) -> Result<(usize, usize), String> {
    let i = locate_end(tags, from, "from")?;
    let j = locate_end(tags, to, "to")?;
    if i > j {
        return Err(format!(
            "`from` e{from} comes after `to` e{to} in the history"
        ));
    }
    Ok((i, j))
}

/// Resolve `from..=to` of a recorded collapse, which holds whole exchanges.
fn locate(
    messages: &[Message],
    tags: &[Tag],
    from: u64,
    to: u64,
) -> Result<(usize, usize), String> {
    let (i, j) = resolve(tags, from, to)?;
    if !is_closed(messages, i, j) {
        return Err(format!(
            "e{from}..e{to} splits a tool call from its result; cannot replay"
        ));
    }
    Ok((i, j))
}

/// Refuse a range that touches what must stay: the first message, the current request and the
/// exchange in flight.
pub fn check_protected(messages: &[Message], i: usize, j: usize) -> Result<(), String> {
    let n = messages.len();
    let in_flight = match messages.last() {
        Some(m) if m.role == Role::Assistant => n - 1,
        _ => in_flight_start(messages),
    };
    if i == 0 {
        return Err(
            "the first message of the history (the task) cannot be collapsed; start the range \
             after it"
                .into(),
        );
    }
    if j >= in_flight {
        return Err(
            "the exchange in flight (the latest tool calls and request) cannot be collapsed; \
             end the range before it"
                .into(),
        );
    }
    let turn = turn_start(messages);
    if (i..=j).contains(&turn) {
        return Err(
            "the current request cannot be collapsed; collapse the exchanges after it".into(),
        );
    }
    Ok(())
}

/// Estimated tokens of messages.
fn tokens(messages: &[Message]) -> u64 {
    (serde_json::to_string(messages).map_or(0, |j| j.len()) / BYTES_PER_TOKEN) as u64
}

/// Replace `messages[i..=j]` by the summary message, archiving the original events.
pub fn apply(
    messages: &mut Vec<Message>,
    tags: &mut Vec<Tag>,
    archive: &mut Archive,
    i: usize,
    j: usize,
    summary: &str,
) -> Collapse {
    let a = match tags[i] {
        Tag::Event(e) | Tag::Summary(e, _) => e,
        Tag::None => 0,
    };
    let b = match tags[j] {
        Tag::Event(e) | Tag::Summary(_, e) => e,
        Tag::None => 0,
    };
    let label = TagRef::Range(a, b).label();
    let before = tokens(&messages[i..=j]);
    let removed: Vec<Message> = messages.drain(i..=j).collect();
    let removed_tags: Vec<Tag> = tags.drain(i..=j).collect();
    let replaced = removed.len();
    for (m, t) in removed.into_iter().zip(removed_tags) {
        if let Tag::Event(n) = t {
            archive.events.insert(n, m);
        }
    }
    let msg = collapsed_message(&label, summary);
    let after = tokens(std::slice::from_ref(&msg));
    messages.insert(i, msg);
    tags.insert(i, Tag::Summary(a, b));
    Collapse {
        tag: label,
        from: a,
        to: b,
        replaced,
        freed_tokens: before.saturating_sub(after),
        note: None,
    }
}

/// Collapse `from..=to` of a live history, enforcing every rule.
pub fn collapse(
    messages: &mut Vec<Message>,
    tags: &mut Vec<Tag>,
    archive: &mut Archive,
    from: u64,
    to: u64,
    summary: &str,
) -> Result<Collapse, String> {
    if summary.trim().is_empty() {
        return Err("`summary` is empty; write what you still need from those events".into());
    }
    if tags.len() != messages.len() {
        return Err("the history is not tagged; collapse is unavailable".into());
    }
    let (mut i, j) = resolve(tags, from, to)?;
    let mut notes = vec![];
    // The task (the first message) always stays: a range that starts at it starts after it.
    if i == 0 {
        i = 1;
        if i > j {
            return Err(
                "the first message of the history (the task) cannot be collapsed and nothing \
                 else is in that range"
                    .into(),
            );
        }
        notes.push("the first message (the task) always stays, so the range starts after it");
    }
    // Whole exchanges only: a range that splits a tool call from its result is widened.
    let (i2, j2) = expand(messages, i, j).ok_or_else(|| {
        format!("e{from}..e{to} cannot be made of whole exchanges: a tool call has no result")
    })?;
    if (i2, j2) != (i, j) {
        notes.push(
            "it was widened to whole exchanges (a tool call is never separated from its result)",
        );
    }
    check_protected(messages, i2, j2)?;
    let asked = if from == to {
        TagRef::Event(from)
    } else {
        TagRef::Range(from, to)
    }
    .label();
    let mut c = apply(messages, tags, archive, i2, j2, summary);
    if c.tag != asked {
        c.note = Some(format!(
            "You asked for {asked} but {}; the range collapsed is {}.",
            notes.join(" and "),
            c.tag
        ));
    }
    Ok(c)
}

/// Re-apply a recorded collapse when a transcript is loaded (the rules held when it was made).
pub fn replay_collapse(
    messages: &mut Vec<Message>,
    tags: &mut Vec<Tag>,
    archive: &mut Archive,
    from: u64,
    to: u64,
    summary: &str,
) -> Result<Collapse, String> {
    let (i, j) = locate(messages, tags, from, to)?;
    Ok(apply(messages, tags, archive, i, j, summary))
}

/// [`crate::tool_loop_guard::drop_tool_calls`] keeping `tags` in step with `messages`.
pub fn drop_tool_calls_tagged(
    messages: &mut Vec<Message>,
    tags: &mut Vec<Tag>,
    ids: &HashSet<String>,
) {
    for m in messages.iter_mut() {
        m.content.retain(|b| match b {
            ContentBlock::ToolUse { id, .. } => !ids.contains(id),
            ContentBlock::ToolResult { tool_use_id, .. } => !ids.contains(tool_use_id),
            _ => true,
        });
    }
    if tags.len() != messages.len() {
        messages.retain(|m| !m.content.is_empty());
        tags.clear();
        return;
    }
    let keep: Vec<bool> = messages.iter().map(|m| !m.content.is_empty()).collect();
    let mut k = keep.iter();
    messages.retain(|_| *k.next().unwrap_or(&true));
    let mut k = keep.iter();
    tags.retain(|_| *k.next().unwrap_or(&true));
}

fn render(m: &Message) -> String {
    let mut out = String::new();
    for b in &m.content {
        match b {
            ContentBlock::Text { text } => out.push_str(text),
            ContentBlock::ToolUse { name, input, .. } => {
                out.push_str(&format!("-> {name} {input}"));
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                out.push_str(if *is_error { "<- error: " } else { "<- " });
                out.push_str(content);
            }
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => continue,
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// The original text of the events behind `tag`: archived (collapsed) ones, and live ones, which
/// are marked as still in the context.
pub fn recall(
    messages: &[Message],
    tags: &[Tag],
    archive: &Archive,
    tag: TagRef,
    event_count: u64,
) -> Result<String, String> {
    let (a, b) = tag.bounds();
    if a > event_count {
        return Err(format!(
            "no event {}: the session has events e1..e{event_count}",
            tag.label()
        ));
    }
    let b = b.min(event_count);
    let mut parts = vec![];
    let mut live = 0;
    let mut missing = vec![];
    for n in a..=b {
        if let Some(m) = archive.events.get(&n) {
            parts.push(format!("[e{n} {}]\n{}", role_name(m), render(m)));
        } else if let Some(m) = tags
            .iter()
            .position(|t| *t == Tag::Event(n))
            .and_then(|k| messages.get(k))
        {
            live += 1;
            parts.push(format!(
                "[e{n} {}, still in your context]\n{}",
                role_name(m),
                render(m)
            ));
        } else {
            missing.push(n);
        }
    }
    if parts.is_empty() {
        return Err(format!(
            "{} is not available: it was never collapsed and automatic compaction, /clear or \
             the loop guard removed it from the history",
            tag.label()
        ));
    }
    let mut out = parts.join("\n\n");
    if live == parts.len() {
        out.insert_str(0, "(all of these events are still in your context)\n\n");
    }
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(|n| format!("e{n}")).collect();
        out.push_str(&format!(
            "\n\n(not available: {}; removed by automatic compaction, /clear or the loop guard)",
            list.join(", ")
        ));
    }
    Ok(out)
}

fn role_name(m: &Message) -> &'static str {
    match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Number of events a recall of `tag` returned (for the notice).
pub fn event_span(tag: TagRef) -> u64 {
    let (a, b) = tag.bounds();
    b - a + 1
}

/// A copy of the history with each event's tag in front of its first block, as the model sees
/// it. Summaries carry their tag in their text and compaction summaries stay untagged.
pub fn label_messages(messages: &[Message], tags: &[Tag]) -> Vec<Message> {
    if messages.len() != tags.len() {
        return messages.to_vec();
    }
    messages
        .iter()
        .zip(tags)
        .map(|(m, t)| match t {
            Tag::Event(n) => label(m, *n),
            _ => m.clone(),
        })
        .collect()
}

fn label(m: &Message, n: u64) -> Message {
    let mut m = m.clone();
    let prefix = format!("[e{n}] ");
    match m.role {
        Role::User => match m.content.first_mut() {
            Some(ContentBlock::Text { text }) => text.insert_str(0, &prefix),
            Some(ContentBlock::ToolResult { content, .. }) => content.insert_str(0, &prefix),
            _ => {}
        },
        Role::Assistant => {
            if let Some(ContentBlock::Text { text }) = m
                .content
                .iter_mut()
                .find(|b| matches!(b, ContentBlock::Text { text } if !text.is_empty()))
            {
                text.insert_str(0, &prefix);
            } else if let Some(k) = m
                .content
                .iter()
                .position(|b| matches!(b, ContentBlock::ToolUse { .. }))
            {
                m.content.insert(
                    k,
                    ContentBlock::Text {
                        text: prefix.trim_end().to_string(),
                    },
                );
            }
        }
    }
    m
}

/// Remove a leading tag the model copied into its own reply (`[e17] `), so labels do not
/// compound.
pub fn strip_echoed_tag(m: &mut Message) {
    for b in m.content.iter_mut() {
        if let ContentBlock::Text { text } = b {
            if !text.is_empty() {
                if let Some(rest) = strip_tag(text) {
                    *text = rest.to_string();
                }
                return;
            }
        }
    }
}

fn strip_tag(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("[e")?;
    let end = rest.find(']')?;
    let inner = &rest[..end];
    if inner.is_empty()
        || !inner
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == 'e')
    {
        return None;
    }
    Some(rest[end + 1..].trim_start_matches(' '))
}

/// Add the usage note after the last message of a request (a copy is sent, the history is not
/// touched). At the tail so the cached prefix of the request stays intact.
pub fn append_note(messages: &mut Vec<Message>, note: &str) {
    match messages.last_mut() {
        Some(m) if m.role == Role::User => match m.content.last_mut() {
            Some(ContentBlock::Text { text }) => {
                text.push_str("\n\n");
                text.push_str(note);
            }
            _ => m.content.push(ContentBlock::Text {
                text: note.to_string(),
            }),
        },
        _ => messages.push(Message::user_text(note)),
    }
}

/// The usage line, plus a context-pressure warning from the high mark on.
pub fn usage_note(estimate: u64, limits: &Limits, hint_at: f64, warn_at: f64) -> String {
    let window = limits.window.max(1);
    let auto = limits.compact_at as f64 / window as f64;
    // The model must have room to act before the harness does.
    let hint = hint_at.min((auto - 0.05).max(0.2));
    let warn = warn_at.min((auto - 0.02).max(hint)).max(hint);
    let used = estimate as f64 / window as f64;
    let pct = (used * 100.0).floor() as u64;
    let mut note = format!(
        "context: {pct}% of {window} tokens; compact with the Context tool only after {}%",
        (hint * 100.0).round() as u64
    );
    if used >= warn {
        note.push_str(&format!(
            "\nWARNING: context pressure, {pct}% full. Collapse the oldest finished exchanges \
             with the Context tool now; at {}% the harness compacts the whole conversation \
             automatically.",
            (auto * 100.0).round() as u64
        ));
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(t: &str) -> Message {
        Message::user_text(t)
    }

    fn call(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "Read".into(),
                input: json!({"path": id}),
            }],
        }
    }

    fn result(id: &str, text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: text.into(),
                is_error: false,
            }],
        }
    }

    /// e1 task, e2/e3 an exchange (a), e4/e5 an exchange (b), e6 answer, e7 request, e8 call
    /// (in flight).
    fn history() -> (Vec<Message>, Vec<Tag>) {
        let messages = vec![
            user("task"),
            call("a"),
            result("a", "AAAA"),
            call("b"),
            result("b", "BBBB"),
            Message::assistant_text("done with b"),
            user("next request"),
            call("c"),
        ];
        let tags = (1..=8).map(Tag::Event).collect();
        (messages, tags)
    }

    #[test]
    fn collapse_replaces_whole_exchanges_and_archives_them() {
        let (mut m, mut t) = history();
        let mut a = Archive::default();
        let c = collapse(&mut m, &mut t, &mut a, 2, 5, "read a and b").unwrap();
        assert_eq!((c.tag.as_str(), c.replaced), ("e2..e5", 4));
        assert_eq!(m.len(), 5);
        assert_eq!(t[1], Tag::Summary(2, 5));
        assert_eq!(t[2], Tag::Event(6));
        assert!(m[1].text().contains("read a and b") && m[1].text().starts_with("[e2..e5]"));
        assert_eq!(a.len(), 4);
        let out = recall(&m, &t, &a, TagRef::Range(2, 5), 8).unwrap();
        assert!(out.contains("AAAA") && out.contains("BBBB") && out.contains("-> Read"));
        assert!(!out.contains("still in your context"));
    }

    #[test]
    fn a_range_splitting_a_tool_pair_is_widened_to_whole_exchanges() {
        let (mut m, mut t) = history();
        let mut a = Archive::default();
        // e3 is a result whose call (e2) lies outside; e4 is a call whose result (e5) does too.
        let c = collapse(&mut m, &mut t, &mut a, 3, 4, "x").unwrap();
        assert_eq!(c.tag, "e2..e5");
        let note = c.note.unwrap();
        assert!(
            note.contains("asked for e3..e4") && note.contains("widened"),
            "{note}"
        );
        assert_eq!(m.len(), 5);
        // No tool call is left without its result, or the other way round.
        assert!(is_closed(&m, 0, m.len() - 2));
        assert_eq!(a.len(), 4);
        // A single call widens to its exchange.
        let (mut m, mut t) = history();
        let c = collapse(&mut m, &mut t, &mut Archive::default(), 4, 4, "x").unwrap();
        assert_eq!(c.tag, "e4..e5");
    }

    #[test]
    fn a_range_starting_at_the_task_starts_after_it() {
        let (mut m, mut t) = history();
        let c = collapse(&mut m, &mut t, &mut Archive::default(), 1, 3, "x").unwrap();
        assert_eq!(c.tag, "e2..e3");
        assert!(c.note.unwrap().contains("task"));
        assert_eq!(m[0].text(), "task");
        let (mut m, mut t) = history();
        let e = collapse(&mut m, &mut t, &mut Archive::default(), 1, 1, "x").unwrap_err();
        assert!(e.contains("first message"), "{e}");
    }

    #[test]
    fn the_request_and_the_exchange_in_flight_are_protected() {
        let (mut m, mut t) = history();
        let mut a = Archive::default();
        let e = collapse(&mut m, &mut t, &mut a, 6, 7, "x").unwrap_err();
        assert!(e.contains("current request"), "{e}");
        // A call still running has no result yet: widening reaches into the exchange in flight.
        let e = collapse(&mut m, &mut t, &mut a, 4, 8, "x").unwrap_err();
        assert!(e.contains("cannot be made of whole exchanges"), "{e}");
        assert_eq!(m.len(), 8);
        // The final exchange (a call and its result) is in flight.
        let mut m = vec![user("task"), user("req"), call("c"), result("c", "CC")];
        let mut t: Vec<Tag> = (1..=4).map(Tag::Event).collect();
        let e = collapse(&mut m, &mut t, &mut a, 3, 4, "x").unwrap_err();
        assert!(e.contains("in flight"), "{e}");
        assert_eq!(m.len(), 4);
        assert!(a.is_empty());
    }

    #[test]
    fn unknown_and_collapsed_endpoints_are_explained() {
        let (mut m, mut t) = history();
        let mut a = Archive::default();
        collapse(&mut m, &mut t, &mut a, 2, 5, "s").unwrap();
        let e = collapse(&mut m, &mut t, &mut a, 3, 6, "x").unwrap_err();
        assert!(e.contains("inside the collapsed range e2..e5"), "{e}");
        let e = collapse(&mut m, &mut t, &mut a, 2, 99, "x").unwrap_err();
        assert!(e.contains("not in the history"), "{e}");
        assert!(collapse(&mut m, &mut t, &mut a, 2, 5, "  ").is_err());
    }

    #[test]
    fn collapsing_a_summary_keeps_the_originals_recallable() {
        let (mut m, mut t) = history();
        let mut a = Archive::default();
        collapse(&mut m, &mut t, &mut a, 2, 3, "a").unwrap();
        collapse(&mut m, &mut t, &mut a, 2, 5, "a and b").unwrap();
        assert_eq!(t[1], Tag::Summary(2, 5));
        let out = recall(&m, &t, &a, TagRef::Event(3), 8).unwrap();
        assert!(out.contains("AAAA"));
    }

    #[test]
    fn recall_reports_live_unknown_and_missing_events() {
        let (m, t) = history();
        let a = Archive::default();
        let out = recall(&m, &t, &a, TagRef::Event(3), 8).unwrap();
        assert!(out.contains("still in your context") && out.contains("AAAA"));
        assert!(recall(&m, &t, &a, TagRef::Event(40), 8)
            .unwrap_err()
            .contains("e1..e8"));
        let mut t2 = t.clone();
        t2[2] = Tag::None;
        let out = recall(&m, &t2, &a, TagRef::Range(2, 4), 8).unwrap();
        assert!(out.contains("not available: e3"), "{out}");
        assert!(recall(&m, &t2, &a, TagRef::Event(3), 8)
            .unwrap_err()
            .contains("not available"));
    }

    #[test]
    fn tagged_drop_keeps_tags_in_step() {
        let (mut m, mut t) = history();
        let ids: HashSet<String> = ["a".to_string()].into();
        drop_tool_calls_tagged(&mut m, &mut t, &ids);
        assert_eq!(m.len(), 6);
        assert_eq!(t[1], Tag::Event(1 + 3));
        assert_eq!(t.len(), m.len());
    }

    #[test]
    fn labels_are_added_to_a_copy_only() {
        let (m, t) = history();
        let l = label_messages(&m, &t);
        assert_eq!(l[0].text(), "[e1] task");
        assert_eq!(m[0].text(), "task");
        match &l[2].content[0] {
            ContentBlock::ToolResult { content, .. } => assert_eq!(content, "[e3] AAAA"),
            b => panic!("{b:?}"),
        }
        // A tool-call-only assistant message gets a leading text block.
        assert!(matches!(&l[1].content[0], ContentBlock::Text { text } if text == "[e2]"));
        assert!(matches!(l[1].content[1], ContentBlock::ToolUse { .. }));
        assert_eq!(l[5].text(), "[e6] done with b");
    }

    #[test]
    fn echoed_tags_are_stripped_from_replies() {
        let mut m = Message::assistant_text("[e17] hello");
        strip_echoed_tag(&mut m);
        assert_eq!(m.text(), "hello");
        let mut m = Message::assistant_text("[e3..e9] hi");
        strip_echoed_tag(&mut m);
        assert_eq!(m.text(), "hi");
        let mut m = Message::assistant_text("[x] keep");
        strip_echoed_tag(&mut m);
        assert_eq!(m.text(), "[x] keep");
    }

    #[test]
    fn the_usage_note_warns_at_the_high_mark() {
        use crate::context::ContextConfig;
        let cfg = ContextConfig {
            model_managed: true,
            ..Default::default()
        };
        let limits = Limits::new(&cfg, 0.8, 65536, 2048);
        assert_eq!(limits.compact_at, (0.95 * 65536.0) as u64);
        let n = usage_note(26900, &limits, 0.85, 0.90);
        assert_eq!(
            n,
            "context: 41% of 65536 tokens; compact with the Context tool only after 85%"
        );
        let n = usage_note(61000, &limits, 0.85, 0.90);
        assert!(n.contains("93% of") || n.contains("context: 93%"), "{n}");
        assert!(n.contains("WARNING") && n.contains("95%"), "{n}");
    }

    #[test]
    fn the_hint_stays_below_the_harness_threshold() {
        use crate::context::ContextConfig;
        let cfg = ContextConfig {
            model_managed: true,
            compact_threshold: Some(0.6),
            ..Default::default()
        };
        let limits = Limits::new(&cfg, 0.8, 10000, 1000);
        let n = usage_note(100, &limits, 0.85, 0.90);
        assert!(n.ends_with("only after 55%"), "{n}");
    }
}
