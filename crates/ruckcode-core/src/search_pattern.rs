//! Extract the search pattern from a search-like tool call, for the tool-loop guard's
//! repeated-search detector.
//!
//! The `Grep` and `Glob` tools carry the pattern in their `pattern` argument. For `Bash`, the
//! command is split with a small shell-word tokenizer (quotes, backslashes, `|`, `||`, `&&`, `;`,
//! `&` and newlines separate commands) and the first command that is a search (`grep`, `egrep`,
//! `fgrep`, `rg`, `ag`, `ack`, `git grep`, `find -name/-iname/-path/-regex`) is the primary one.
//! Later pipeline stages (`| grep -v internal`) are ignored. The pattern is normalised: trimmed,
//! whitespace collapsed, surrounding quotes stripped.

use serde_json::Value;

/// The normalised search pattern of a tool call, or `None` if it is not a search.
pub fn search_pattern(tool: &str, input: &Value) -> Option<String> {
    match tool {
        "Grep" | "Glob" => normalise(input["pattern"].as_str()?),
        "Bash" => bash_search_pattern(input["command"].as_str()?),
        _ => None,
    }
}

/// Trim, collapse whitespace and strip surrounding quotes; `None` if nothing is left.
pub fn normalise(p: &str) -> Option<String> {
    let mut s = p.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let inner = ['"', '\'']
            .iter()
            .find_map(|q| s.strip_prefix(*q)?.strip_suffix(*q))
            .filter(|_| s.len() >= 2)
            .map(|i| i.trim().to_string());
        match inner {
            Some(i) => s = i,
            None => break,
        }
    }
    (!s.is_empty()).then_some(s)
}

/// The pattern of the first search command in a shell command line.
pub fn bash_search_pattern(command: &str) -> Option<String> {
    split_commands(&tokenize(command))
        .iter()
        .find_map(|argv| command_pattern(argv))
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Sep,
}

/// Split a command line into words and command separators.
fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut chars = s.chars().peekable();
    fn flush(cur: &mut String, has: &mut bool, out: &mut Vec<Tok>) {
        if *has {
            out.push(Tok::Word(std::mem::take(cur)));
            *has = false;
        }
    }
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                has = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                cur.push(n);
                                chars.next();
                            }
                            _ => cur.push('\\'),
                        },
                        _ => cur.push(q),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(n) => {
                    has = true;
                    cur.push(n);
                }
                None => {
                    has = true;
                    cur.push('\\');
                }
            },
            '|' | '&' | ';' | '\n' | '(' | ')' => {
                // `2>&1` and `&>` are redirections, not separators.
                if c == '&' && (cur.ends_with('>') || chars.peek() == Some(&'>')) {
                    has = true;
                    cur.push(c);
                    continue;
                }
                flush(&mut cur, &mut has, &mut out);
                if out.last() != Some(&Tok::Sep) {
                    out.push(Tok::Sep);
                }
            }
            c if c.is_whitespace() => flush(&mut cur, &mut has, &mut out),
            c => {
                has = true;
                cur.push(c);
            }
        }
    }
    flush(&mut cur, &mut has, &mut out);
    out
}

fn split_commands(toks: &[Tok]) -> Vec<Vec<String>> {
    let mut cmds = vec![vec![]];
    for t in toks {
        match t {
            Tok::Word(w) => cmds.last_mut().unwrap().push(w.clone()),
            Tok::Sep => cmds.push(vec![]),
        }
    }
    cmds.retain(|c| !c.is_empty());
    cmds
}

/// Whether a word is a redirection (`>f`, `2>/dev/null`, `<in`).
fn is_redirect(w: &str) -> bool {
    let t = w.trim_start_matches(|c: char| c.is_ascii_digit());
    t.starts_with('>') || t.starts_with('<') || t.starts_with("&>")
}

fn command_pattern(argv: &[String]) -> Option<String> {
    // Skip env assignments and bare wrappers (`xargs`, `sudo`, `time`, ...) with their flags.
    let mut i = 0;
    while i < argv.len() {
        let w = argv[i].as_str();
        let assignment = w.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        if assignment {
            i += 1;
        } else if matches!(
            w,
            "env" | "sudo" | "time" | "command" | "nice" | "xargs" | "exec"
        ) {
            i += 1;
            while i < argv.len() && argv[i].starts_with('-') && argv[i].len() > 1 {
                i += 1;
            }
        } else {
            break;
        }
    }
    let rest: Vec<&str> = argv[i..]
        .iter()
        .map(String::as_str)
        .filter(|w| !is_redirect(w))
        .collect();
    let (prog, args) = rest.split_first()?;
    let prog = prog.rsplit('/').next().unwrap_or(prog);
    match prog {
        "grep" | "egrep" | "fgrep" | "zgrep" => grep_like(args, GREP_VALUE_FLAGS, GREP_LONG_VALUE),
        "rg" => grep_like(args, RG_VALUE_FLAGS, RG_LONG_VALUE),
        "ag" | "ack" => grep_like(args, AG_VALUE_FLAGS, AG_LONG_VALUE),
        "git" => {
            let mut j = 0;
            while j < args.len() {
                match args[j] {
                    "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => j += 2,
                    a if a.starts_with('-') => j += 1,
                    _ => break,
                }
            }
            if args.get(j) == Some(&"grep") {
                grep_like(&args[j + 1..], GREP_VALUE_FLAGS, GREP_LONG_VALUE)
            } else {
                None
            }
        }
        "find" => find_pattern(args),
        _ => None,
    }
}

// Short flags that take a value (`e`, the pattern, is handled separately).
const GREP_VALUE_FLAGS: &str = "ABCmdD";
const RG_VALUE_FLAGS: &str = "ABCmgtTjMdrE";
const AG_VALUE_FLAGS: &str = "ABCmGgpsW";
const GREP_LONG_VALUE: &[&str] = &[
    "include",
    "exclude",
    "exclude-dir",
    "exclude-from",
    "max-count",
    "after-context",
    "before-context",
    "context",
    "devices",
    "directories",
    "binary-files",
    "label",
    "group-separator",
];
const RG_LONG_VALUE: &[&str] = &[
    "glob",
    "iglob",
    "type",
    "type-not",
    "type-add",
    "max-count",
    "max-depth",
    "maxdepth",
    "after-context",
    "before-context",
    "context",
    "replace",
    "threads",
    "max-columns",
    "ignore-file",
    "sort",
    "sortr",
    "encoding",
    "engine",
    "path-separator",
    "colors",
    "max-filesize",
    "pre",
    "pre-glob",
];
const AG_LONG_VALUE: &[&str] = &[
    "ignore",
    "depth",
    "max-count",
    "after",
    "before",
    "context",
    "file-search-regex",
    "pager",
    "path-to-ignore",
    "workers",
    "type",
];

/// Parse grep/rg/ag-style arguments: the first `-e`/`--regexp` value, else the first positional,
/// is the pattern. grep's `-f FILE` has no usable pattern.
fn grep_like(args: &[&str], short_value: &str, long_value: &[&str]) -> Option<String> {
    let is_grep = short_value == GREP_VALUE_FLAGS;
    let mut explicit: Option<String> = None;
    let mut positional: Option<String> = None;
    let mut i = 0;
    let mut no_more_flags = false;
    while i < args.len() {
        let a = args[i];
        i += 1;
        if no_more_flags || !a.starts_with('-') || a == "-" {
            positional.get_or_insert_with(|| a.to_string());
            continue;
        }
        if a == "--" {
            no_more_flags = true;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            if name == "regexp" {
                let v = match inline {
                    Some(v) => Some(v.to_string()),
                    None => {
                        i += 1;
                        args.get(i - 1).map(|s| s.to_string())
                    }
                };
                explicit = explicit.or(v);
            } else if name == "file" && is_grep {
                return None;
            } else if inline.is_none() && long_value.contains(&name) {
                i += 1;
            }
            continue;
        }
        let cluster: Vec<char> = a[1..].chars().collect();
        for (k, &c) in cluster.iter().enumerate() {
            if c == 'e' {
                let attached: String = cluster[k + 1..].iter().collect();
                let v = if attached.is_empty() {
                    i += 1;
                    args.get(i - 1).map(|s| s.to_string())
                } else {
                    Some(attached)
                };
                explicit = explicit.or(v);
                break;
            }
            if c == 'f' && is_grep {
                return None;
            }
            if short_value.contains(c) {
                if k + 1 == cluster.len() {
                    i += 1;
                }
                break;
            }
        }
    }
    normalise(&explicit.or(positional)?)
}

fn find_pattern(args: &[&str]) -> Option<String> {
    let at = args.iter().position(|a| {
        matches!(
            *a,
            "-name" | "-iname" | "-path" | "-ipath" | "-regex" | "-iregex" | "-wholename"
        )
    })?;
    normalise(args.get(at + 1)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(cmd: &str) -> Option<String> {
        bash_search_pattern(cmd)
    }

    #[test]
    fn grep_variants_share_the_pattern() {
        let want = Some("export const base".to_string());
        for c in [
            r#"grep -r "export const base" node_modules/@sveltejs/kit/src/runtime/app/"#,
            r#"grep -rn 'export const base' node_modules/@sveltejs/kit"#,
            r#"grep -r "export const base" node_modules/@sveltejs/kit/src/runtime/app/ | grep -v internal"#,
            r#"cd app && grep -r  "export   const base" . 2>/dev/null | head -20"#,
            r#"grep -rE --include=*.js "export const base" ."#,
            r#"grep --include "*.js" -r "export const base" ."#,
            r#"rg -e "export const base" src"#,
            r#"rg "export const base" -g '*.ts'"#,
            r#"git grep -n "export const base" -- src"#,
            r#"git -C sub grep "export const base""#,
            r#"grep -A3 -B 2 "export const base" f"#,
            r#"grep -e "export const base" -r ."#,
            r#"grep --regexp="export const base" -r ."#,
            r#"FOO=1 grep "export const base" f"#,
        ] {
            assert_eq!(p(c), want, "{c}");
        }
    }

    #[test]
    fn pipelines_use_the_primary_search() {
        assert_eq!(p("cat f | grep -v x foo"), Some("x".into()));
        assert_eq!(p("ls | grep -E 'a|b' | wc -l"), Some("a|b".into()));
        assert_eq!(p("grep -r foo . | grep -v bar"), Some("foo".into()));
        assert_eq!(p("echo hi; rg 'a b' ."), Some("a b".into()));
        assert_eq!(p("make && grep -rn TODO src"), Some("TODO".into()));
    }

    #[test]
    fn find_and_non_searches() {
        assert_eq!(p(r#"find . -name "*.rs" -type f"#), Some("*.rs".into()));
        assert_eq!(p("find . -type f -iname '*.TS'"), Some("*.TS".into()));
        assert_eq!(p("find . -type f"), None);
        assert_eq!(p("ls -la"), None);
        assert_eq!(p("sed -i 's/a/b/' f"), None);
        assert_eq!(p("grep -f patterns.txt file"), None);
        assert_eq!(p("grep"), None);
        assert_eq!(p("git status"), None);
        assert_eq!(p("find . -name x | xargs grep -l foo"), Some("x".into()));
        assert_eq!(p("xargs -0 grep -l foo"), Some("foo".into()));
    }

    #[test]
    fn quotes_and_whitespace_are_normalised() {
        assert_eq!(p(r#"grep "  a   b  " f"#), Some("a b".into()));
        assert_eq!(p(r#"grep -E '^\s*import' f"#), Some(r"^\s*import".into()));
        assert_eq!(p(r#"grep -c "a\"b" f"#), Some(r#"a"b"#.into()));
        assert_eq!(normalise("  'x  y' "), Some("x y".into()));
        assert_eq!(normalise("  "), None);
    }

    #[test]
    fn tool_arguments() {
        assert_eq!(
            search_pattern("Grep", &json!({"pattern": "  foo  bar", "path": "x"})),
            Some("foo bar".into())
        );
        assert_eq!(
            search_pattern("Glob", &json!({"pattern": "**/*.rs"})),
            Some("**/*.rs".into())
        );
        assert_eq!(search_pattern("Read", &json!({"pattern": "x"})), None);
        assert_eq!(
            search_pattern("Bash", &json!({"command": "rg -e foo"})),
            Some("foo".into())
        );
    }
}
