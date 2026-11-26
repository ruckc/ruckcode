//! HTML to readable Markdown-ish text, using `html2text` for the conversion and a small
//! pre-pass that drops boilerplate elements (scripts, styles, navigation, footers, ...).

use html2text::render::{RichAnnotation, TextDecorator};
use regex::Regex;
use std::sync::OnceLock;

/// Elements removed together with their contents.
const STRIPPED: [&str; 12] = [
    "script", "style", "noscript", "template", "svg", "iframe", "nav", "footer", "aside", "form",
    "dialog", "head",
];

fn tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let names = STRIPPED.join("|");
        Regex::new(&format!(r"(?is)<(/?)({names})\b[^>]*?(/?)>")).unwrap()
    })
}

fn comment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap())
}

/// Remove stripped elements (nesting-aware per tag name). An element that is never closed is
/// dropped from its start to the end of the document, except `head`/`nav`/`footer`/`aside`/
/// `form`, whose open tag alone is removed (HTML allows omitting those end tags in practice).
fn strip_boilerplate(html: &str) -> String {
    let html = comment_re().replace_all(html, "");
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    let mut depth_name: Option<String> = None;
    let mut depth = 0usize;
    let mut skip_from = 0usize;
    for m in tag_re().captures_iter(&html) {
        let whole = m.get(0).unwrap();
        let closing = !m[1].is_empty();
        let self_closing = !m[3].is_empty();
        let name = m[2].to_ascii_lowercase();
        match &depth_name {
            None => {
                if closing || self_closing {
                    continue;
                }
                out.push_str(&html[pos..whole.start()]);
                depth_name = Some(name);
                depth = 1;
                skip_from = whole.start();
            }
            Some(open) if *open == name => {
                if closing {
                    depth -= 1;
                    if depth == 0 {
                        pos = whole.end();
                        depth_name = None;
                    }
                } else if !self_closing {
                    depth += 1;
                }
            }
            Some(_) => {}
        }
    }
    match depth_name {
        None => out.push_str(&html[pos..]),
        Some(name) => {
            // Unclosed: drop just the open tag for the structural elements, the rest of the
            // document for raw-text ones.
            if matches!(name.as_str(), "script" | "style" | "noscript" | "template") {
                // dropped to the end
            } else {
                let rest = &html[skip_from..];
                let end = rest.find('>').map_or(rest.len(), |i| i + 1);
                out.push_str(&rest[end..]);
            }
        }
    }
    out
}

/// A Markdown-flavoured decorator: `[text](url)` links, `` `code` ``, `**strong**`, `*em*`.
#[derive(Clone, Debug, Default)]
struct MdDecorator {
    links: Vec<String>,
}

impl TextDecorator for MdDecorator {
    type Annotation = RichAnnotation;

    fn decorate_link_start(&mut self, url: &str) -> (String, Self::Annotation) {
        self.links.push(url.to_string());
        ("[".into(), RichAnnotation::Link(url.to_string()))
    }

    fn decorate_link_end(&mut self) -> String {
        match self.links.pop() {
            Some(u) => format!("]({u})"),
            None => "]".into(),
        }
    }

    fn decorate_em_start(&self) -> (String, Self::Annotation) {
        ("*".into(), RichAnnotation::Emphasis)
    }
    fn decorate_em_end(&self) -> String {
        "*".into()
    }
    fn decorate_strong_start(&self) -> (String, Self::Annotation) {
        ("**".into(), RichAnnotation::Strong)
    }
    fn decorate_strong_end(&self) -> String {
        "**".into()
    }
    fn decorate_strikeout_start(&self) -> (String, Self::Annotation) {
        ("~~".into(), RichAnnotation::Strikeout)
    }
    fn decorate_strikeout_end(&self) -> String {
        "~~".into()
    }
    fn decorate_code_start(&self) -> (String, Self::Annotation) {
        ("`".into(), RichAnnotation::Code)
    }
    fn decorate_code_end(&self) -> String {
        "`".into()
    }
    fn decorate_preformat_first(&self) -> Self::Annotation {
        RichAnnotation::Preformat(false)
    }
    fn decorate_preformat_cont(&self) -> Self::Annotation {
        RichAnnotation::Preformat(true)
    }
    fn decorate_image(&mut self, src: &str, title: &str) -> (String, Self::Annotation) {
        (
            format!("![{title}]({src})"),
            RichAnnotation::Image(src.to_string()),
        )
    }
    fn header_prefix(&self, level: usize) -> String {
        "#".repeat(level) + " "
    }
    fn quote_prefix(&self) -> String {
        "> ".into()
    }
    fn unordered_item_prefix(&self) -> String {
        "- ".into()
    }
    fn ordered_item_prefix(&self, i: i64) -> String {
        format!("{i}. ")
    }
    fn make_subblock_decorator(&self) -> Self {
        Self::default()
    }
}

/// The text of the document `<title>`, if any.
fn title_of(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    let raw = re.captures(html)?.get(1)?.as_str();
    let t = html2text::from_read(raw.as_bytes(), 10_000).ok()?;
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then_some(t)
}

/// Convert an HTML document to Markdown-ish text: headings, `[text](url)` links, lists and
/// code kept; scripts, styles and page chrome dropped; blank-line runs collapsed.
pub fn to_markdown(html: &str) -> Result<String, String> {
    let title = title_of(html);
    let cleaned = strip_boilerplate(html);
    let text = html2text::config::with_decorator(MdDecorator::default())
        .link_footnotes(false)
        .string_from_read(cleaned.as_bytes(), 100_000)
        .map_err(|e| format!("cannot convert the HTML: {e}"))?;
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let body = out.trim().to_string();
    Ok(match title {
        Some(t) if !body.contains(&t) => format!("Title: {t}\n\n{body}"),
        _ => body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html><head><title>Guide to Foo</title>
<style>body { color: red }</style><script>var secret = 1;</script></head>
<body>
<nav><ul><li><a href="/home">Home</a></li><li>NAVMARK</li></ul></nav>
<main><h1>Foo Guide</h1>
<p>Intro with a <a href="https://example.com/doc">link to docs</a> and <code>inline()</code>.</p>
<h2>Steps</h2>
<ul><li>First</li><li>Second</li></ul>
<ol><li>One</li><li>Two</li></ol>
<pre>fn main() {
    println!("hi");
}</pre>
</main>
<aside>ASIDEMARK</aside>
<footer>FOOTERMARK <nav>inner</nav></footer>
<script>alert('x')</script>
</body></html>"#;

    #[test]
    fn extracts_readable_markdown_and_drops_chrome() {
        let md = to_markdown(PAGE).unwrap();
        assert!(md.contains("# Foo Guide"), "{md}");
        assert!(md.contains("## Steps"), "{md}");
        assert!(
            md.contains("[link to docs](https://example.com/doc)"),
            "{md}"
        );
        assert!(md.contains("`inline()`"), "{md}");
        assert!(md.contains("- First") && md.contains("- Second"), "{md}");
        assert!(md.contains("1. One") && md.contains("2. Two"), "{md}");
        assert!(md.contains("println!(\"hi\");"), "{md}");
        for gone in [
            "secret",
            "alert",
            "color: red",
            "NAVMARK",
            "ASIDEMARK",
            "FOOTERMARK",
            "inner",
        ] {
            assert!(!md.contains(gone), "{gone} leaked: {md}");
        }
        assert!(md.starts_with("Title: Guide to Foo"), "{md}");
    }

    #[test]
    fn unclosed_chrome_and_comments_do_not_eat_the_page() {
        let md = to_markdown("<body><!-- <script> --><nav>menu<p>Real content</p></body>").unwrap();
        assert!(md.contains("Real content"), "{md}");
        let md = to_markdown("<body><p>Keep</p><script>drop me").unwrap();
        assert!(md.contains("Keep") && !md.contains("drop me"), "{md}");
    }
}
