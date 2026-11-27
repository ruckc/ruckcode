//! `Context`: the model manages its own context window (collapse a range of history into its
//! own summary, recall archived originals).
//!
//! This module holds what is generic and stateless: the tool's specification, the parsing and
//! validation of its flat input, and the tag syntax. The agent loop (`ruck-harness-core`) runs
//! the actions, because they need the session history.
//!
//! The schema is deliberately flat: plain optional strings and one enum. Array-of-object schemas
//! made some small models stop calling tools (see the lean-tools notes in the README).

use crate::{LeanClass, Tool, ToolContext, ToolKind, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

/// The tool's name.
pub const CONTEXT_TOOL: &str = "Context";

/// A reference to history events: one event (`e9`) or a range, also the tag of a summary
/// (`e3..e17`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagRef {
    Event(u64),
    Range(u64, u64),
}

impl TagRef {
    /// Parse `e9`, `E9`, `9`, `[e9]`, `e3..e17` or `3..17`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let t = s
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim();
        let bad = || {
            format!(
                "invalid tag `{}`: use an event tag like e9 or a range like e3..e17",
                s.trim()
            )
        };
        let num = |p: &str| -> Result<u64, String> {
            let p = p.trim();
            let p = p.strip_prefix(['e', 'E']).unwrap_or(p);
            match p.parse::<u64>() {
                Ok(n) if n > 0 => Ok(n),
                _ => Err(bad()),
            }
        };
        if let Some((a, b)) = t.split_once("..") {
            let (a, b) = (num(a)?, num(b)?);
            if a > b {
                return Err(format!("invalid range `{t}`: the start is after the end"));
            }
            return Ok(if a == b {
                Self::Event(a)
            } else {
                Self::Range(a, b)
            });
        }
        num(t).map(Self::Event)
    }

    /// First and last event of the reference.
    pub fn bounds(self) -> (u64, u64) {
        match self {
            Self::Event(n) => (n, n),
            Self::Range(a, b) => (a, b),
        }
    }

    /// The tag as shown to the model.
    pub fn label(self) -> String {
        match self {
            Self::Event(n) => format!("e{n}"),
            Self::Range(a, b) => format!("e{a}..e{b}"),
        }
    }
}

/// A validated call of the `Context` tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAction {
    /// Replace the events `from..=to` with `summary`.
    Collapse { from: u64, to: u64, summary: String },
    /// Read the archived originals of a tag.
    Recall { tag: TagRef },
}

/// An optional string field; numbers count as event numbers (small models send `3` for `e3`).
fn field(input: &Value, key: &str) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

impl ContextAction {
    /// Validate the input of a `Context` call. The error says what is wrong and what the chosen
    /// action needs.
    pub fn parse(input: &Value) -> Result<Self, String> {
        let action = field(input, "action")?
            .ok_or("missing required string parameter `action` (\"collapse\" or \"recall\")")?;
        match action.trim().to_ascii_lowercase().as_str() {
            "collapse" => {
                let from = field(input, "from")?;
                let to = field(input, "to")?;
                let summary = field(input, "summary")?;
                let missing: Vec<&str> = [
                    ("from", from.is_none()),
                    ("to", to.is_none()),
                    ("summary", summary.is_none()),
                ]
                .iter()
                .filter(|(_, m)| *m)
                .map(|(k, _)| *k)
                .collect();
                if !missing.is_empty() {
                    return Err(format!(
                        "action `collapse` needs `from`, `to` and `summary`; missing: {}",
                        missing.join(", ")
                    ));
                }
                let (from, to) = (
                    TagRef::parse(&from.unwrap_or_default()).map_err(|e| format!("`from`: {e}"))?,
                    TagRef::parse(&to.unwrap_or_default()).map_err(|e| format!("`to`: {e}"))?,
                );
                let (from, to) = (from.bounds().0, to.bounds().1);
                if from > to {
                    return Err(format!(
                        "`from` (e{from}) is after `to` (e{to}); give the range oldest first"
                    ));
                }
                Ok(Self::Collapse {
                    from,
                    to,
                    summary: summary.unwrap_or_default(),
                })
            }
            "recall" => {
                let tag = field(input, "tag")?.ok_or(
                    "action `recall` needs `tag`: an event tag like e9 or a summary tag like e3..e17",
                )?;
                TagRef::parse(&tag)
                    .map(|tag| Self::Recall { tag })
                    .map_err(|e| format!("`tag`: {e}"))
            }
            other => Err(format!(
                "unknown action `{other}`: use \"collapse\" or \"recall\""
            )),
        }
    }
}

/// The `Context` tool. The agent loop runs it; `call` only exists to satisfy the trait.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextTool;

#[async_trait]
impl Tool for ContextTool {
    fn name(&self) -> &str {
        CONTEXT_TOOL
    }

    fn description(&self) -> String {
        "Manage your own context window. Messages in the history carry tags like [e17], and each \
         request ends with a `context: N% of M tokens` line. Compact only once usage is above \
         the stated mark: compacting earlier loses detail for nothing. \
         action=collapse replaces the events `from`..`to` (inclusive) with your own `summary`, \
         which must keep what you still need (decisions, file paths, results, open questions); \
         the originals are archived, not deleted. A range is widened to whole exchanges (a tool \
         call is never separated from its result); the first message and the latest request \
         stay. It returns the summary tag, e.g. e3..e17. \
         action=recall reads the archived originals of `tag` (an event tag like e9, or a summary \
         tag like e3..e17); it does not put them back into the history, and the text it returns \
         counts against your context."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["collapse", "recall"],
                    "description": "collapse: replace a range of history with your summary. \
                                    recall: read archived originals."
                },
                "from": {
                    "type": "string",
                    "description": "collapse: first event of the range, e.g. e3"
                },
                "to": {
                    "type": "string",
                    "description": "collapse: last event of the range (inclusive), e.g. e17"
                },
                "summary": {
                    "type": "string",
                    "description": "collapse: your summary of the range"
                },
                "tag": {
                    "type": "string",
                    "description": "recall: an event tag (e9) or a summary tag (e3..e17)"
                }
            },
            "required": ["action"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Think
    }

    fn title(&self, input: &Value) -> String {
        match ContextAction::parse(input) {
            Ok(ContextAction::Collapse { from, to, .. }) => format!("Collapse e{from}..e{to}"),
            Ok(ContextAction::Recall { tag }) => format!("Recall {}", tag.label()),
            Err(_) => "Context".into(),
        }
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    fn auto_allow(&self) -> bool {
        true
    }

    fn lean_class(&self) -> LeanClass {
        LeanClass::OptInWhenLean
    }

    async fn call(&self, _: Value, _: &ToolContext) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::error(
            "the Context tool is run by the agent loop, which this session does not provide",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_parse_in_the_forms_models_write() {
        for (s, t) in [
            ("e9", TagRef::Event(9)),
            ("E9", TagRef::Event(9)),
            ("9", TagRef::Event(9)),
            ("[e9]", TagRef::Event(9)),
            ("e3..e17", TagRef::Range(3, 17)),
            ("3..17", TagRef::Range(3, 17)),
            ("e4..e4", TagRef::Event(4)),
        ] {
            assert_eq!(TagRef::parse(s), Ok(t), "{s}");
        }
        for s in ["", "e", "e0", "x9", "e3..", "e9..e3", "e1.5"] {
            assert!(TagRef::parse(s).is_err(), "{s}");
        }
        assert_eq!(TagRef::Range(3, 17).label(), "e3..e17");
    }

    #[test]
    fn the_schema_is_flat() {
        let s = ContextTool.input_schema();
        let text = s.to_string();
        for banned in ["oneOf", "anyOf", "allOf", "items"] {
            assert!(!text.contains(banned), "{banned}");
        }
        let props = s["properties"].as_object().unwrap();
        assert_eq!(props.len(), 5);
        assert!(props.values().all(|p| p["type"] == "string"));
        assert_eq!(s["required"], json!(["action"]));
        assert_eq!(props["action"]["enum"], json!(["collapse", "recall"]));
    }

    #[test]
    fn collapse_and_recall_validate_their_own_fields() {
        let ok = ContextAction::parse(
            &json!({"action": "collapse", "from": "e3", "to": 17, "summary": "did things"}),
        );
        assert_eq!(
            ok,
            Ok(ContextAction::Collapse {
                from: 3,
                to: 17,
                summary: "did things".into()
            })
        );
        assert_eq!(
            ContextAction::parse(&json!({"action": "recall", "tag": "e3..e17"})),
            Ok(ContextAction::Recall {
                tag: TagRef::Range(3, 17)
            })
        );
        let err = |v: Value| ContextAction::parse(&v).unwrap_err();
        assert!(err(json!({})).contains("`action`"));
        assert!(err(json!({"action": "shrink"})).contains("unknown action"));
        let e = err(json!({"action": "collapse", "from": "e1"}));
        assert!(e.contains("missing: to, summary"), "{e}");
        assert!(
            err(json!({"action": "collapse", "from": "e5", "to": "e2", "summary": "x"}))
                .contains("after")
        );
        assert!(err(json!({"action": "recall"})).contains("needs `tag`"));
        assert!(err(json!({"action": "recall", "tag": "zzz"})).contains("invalid tag"));
        assert!(err(json!({"action": "recall", "tag": ["e1"]})).contains("must be a string"));
    }

    #[test]
    fn lean_profiles_opt_in() {
        assert_eq!(ContextTool.lean_class(), LeanClass::OptInWhenLean);
        assert!(ContextTool.auto_allow());
    }
}
