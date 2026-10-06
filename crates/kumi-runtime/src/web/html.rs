//! A page as readable text, preserving its structure and code while leaving out site furniture.

use kumi_common::js::string::{trim, utf16_len};
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::LazyLock,
};
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageText {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub text: String,
    pub scripted: bool,
}
static NAMED: LazyLock<HashMap<String, String>> = LazyLock::new(|| serde_json::from_str(include_str!("entities.json")).unwrap());
static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#\d{1,7}|#[xX][0-9a-fA-F]{1,6}|[A-Za-z][A-Za-z0-9]{1,31});").unwrap());
pub fn decode_entities(text: &str) -> String {
    decoded_entities(text).into_owned()
}
fn decoded_entities(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    ENTITY.replace_all(text, |c: &Captures| {
        let name = &c[1];
        if let Some(code) = name.strip_prefix('#') {
            let parsed = if let Some(hex) = code.strip_prefix('x').or_else(|| code.strip_prefix('X')) {
                u32::from_str_radix(hex, 16)
            } else {
                code.parse()
            };
            return parsed.ok().filter(|v| *v > 0).and_then(char::from_u32).unwrap_or('\u{fffd}').to_string();
        }
        NAMED.get(name).cloned().unwrap_or_else(|| c[0].into())
    })
}
type Attributes = HashMap<String, String>;
static ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"([^\s"'>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?"#).unwrap());
fn attributes(source: &str) -> Attributes {
    let mut found = Attributes::new();
    if source.is_empty() {
        return found;
    }
    for c in ATTR.captures_iter(source) {
        found
            .entry(c[1].to_lowercase())
            .or_insert_with(|| decode_entities(c.get(2).or_else(|| c.get(3)).or_else(|| c.get(4)).map_or("", |m| m.as_str())));
    }
    found
}
fn contains(set: &str, name: &str) -> bool {
    set.split(' ').any(|v| v == name)
}
fn raw(name: &str) -> bool {
    matches!(name, "script" | "style" | "textarea" | "title" | "xmp" | "noscript" | "iframe" | "noembed" | "noframes" | "plaintext")
}
fn skip(name: &str) -> bool {
    matches!(
        name,
        "head"
            | "template"
            | "svg"
            | "math"
            | "canvas"
            | "object"
            | "embed"
            | "nav"
            | "footer"
            | "aside"
            | "select"
            | "button"
            | "dialog"
            | "audio"
            | "video"
            | "map"
            | "datalist"
    )
}
fn void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
            | "keygen"
    )
}
fn block(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "blockquote"
            | "body"
            | "center"
            | "details"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "form"
            | "header"
            | "hgroup"
            | "html"
            | "legend"
            | "main"
            | "menu"
            | "section"
            | "summary"
            | "caption"
            | "tbody"
            | "thead"
            | "tfoot"
            | "dir"
    )
}
fn attr<'a>(attrs: &'a Attributes, name: &str) -> &'a str {
    attrs.get(name).map_or("", String::as_str)
}
static HIDDEN_STYLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)display\s*:\s*none|visibility\s*:\s*hidden").unwrap());
fn hidden(a: &Attributes) -> bool {
    if a.is_empty() {
        return false;
    }
    a.contains_key("hidden") || attr(a,"aria-hidden")=="true" || HIDDEN_STYLE.is_match(attr(a,"style")) || contains("navigation banner contentinfo search complementary menu menubar toolbar",&attr(a,"role").to_lowercase()) || format!("{} {}",attr(a,"class"),attr(a,"id")).to_lowercase().split_whitespace().any(|v|contains("navbox navbar breadcrumb breadcrumbs sidebar ambox noprint mw-editsection mw-jump-link vector-dropdown vector-page-toolbar vector-toc mw-indicators cookie-banner skip-link skip-to-content sr-only visually-hidden screen-reader-text",v))
}
fn absolute(href: &str, base: &str) -> Option<String> {
    let href = trim(href);
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let lower = href.to_lowercase();
    if ["javascript:", "data:", "mailto:", "tel:", "blob:"].iter().any(|s| lower.starts_with(s)) {
        return None;
    }
    let url = Url::parse(href).or_else(|_| Url::parse(base)?.join(href)).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}
static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^<([a-zA-Z][^\s/>]*)((?:\s+[^\s"'>/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+))?)*)\s*(/?)>"#).unwrap()
});
static CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^</([a-zA-Z][^\s/>]*)\s*>").unwrap());
// Bare lowercase tags need neither regex capture buffers nor a lowercase copy.
// Everything outside this exact subset keeps the full source-compatible grammar.
fn bare_tag(tail: &str, prefix: usize) -> Option<&str> {
    let bytes = tail.as_bytes().get(prefix..)?;
    if !bytes.first()?.is_ascii_lowercase() {
        return None;
    }
    let end = bytes.iter().position(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit())?;
    (bytes[end] == b'>').then(|| &tail[prefix..prefix + end])
}
fn close_tag(tail: &str) -> Option<(Cow<'_, str>, usize)> {
    if !tail.starts_with("</") {
        return None;
    }
    if let Some(name) = bare_tag(tail, 2) {
        return Some((Cow::Borrowed(name), name.len() + 3));
    }
    let capture = CLOSE.captures(tail)?;
    Some((Cow::Owned(capture[1].to_lowercase()), capture[0].len()))
}
fn open_tag(tail: &str) -> Option<(Cow<'_, str>, &str, bool, usize)> {
    if let Some(name) = bare_tag(tail, 1) {
        return Some((Cow::Borrowed(name), "", false, name.len() + 2));
    }
    let capture = TAG.captures(tail)?;
    Some((Cow::Owned(capture[1].to_lowercase()), capture.get(2)?.as_str(), &capture[3] == "/", capture[0].len()))
}
static SPACES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\t-\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]+").unwrap());
static OPENING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\n) *(?:#{1,6} |- |[0-9]+\. |> )$").unwrap());
static LANGUAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)(?:lang|language|highlight-source)-([a-zA-Z0-9_+#-]+)").unwrap());
fn language(a: &Attributes) -> String {
    LANGUAGE
        .captures(attr(a, "class"))
        .map(|c| c[1].into())
        .unwrap_or_else(|| attr(a, "data-lang").chars().filter(|c| c.is_ascii_alphanumeric() || "_+#-".contains(*c)).collect())
}
fn words(text: &str) -> Cow<'_, str> {
    // Ordinary ASCII prose already has the exact whitespace this pass produces.
    // Unicode, other whitespace and adjacent spaces retain the full regex behavior.
    let mut space = false;
    for byte in text.bytes() {
        if !byte.is_ascii() || matches!(byte, b'\t' | b'\n' | 0x0b | 0x0c | b'\r') || (byte == b' ' && space) {
            return SPACES.replace_all(text, " ");
        }
        space = byte == b' ';
    }
    Cow::Borrowed(text)
}
fn level(name: &str) -> Option<usize> {
    let b = name.as_bytes();
    (b.len() == 2 && b[0] == b'h' && (b'1'..=b'6').contains(&b[1])).then(|| (b[1] - b'0') as usize)
}
#[derive(Default)]
struct Builder {
    out: String,
    pre: usize,
    fresh: bool,
    lists: Vec<(bool, i64)>,
    links: Vec<(Option<String>, usize)>,
    quotes: Vec<usize>,
    cells: Vec<usize>,
    scripts: Vec<(char, usize)>,
    codes: Vec<usize>,
    heading: bool,
    linked: HashSet<String>,
}
impl Builder {
    fn opening(&self) -> bool {
        if !self.out.ends_with(' ') {
            return false;
        }
        let start = self.out.char_indices().rev().nth(23).map_or(0, |(index, _)| index);
        let tail = &self.out[start..];
        OPENING.find(tail).is_some_and(|m| m.as_str().starts_with('\n') || start == 0)
    }
    fn trim_end(&mut self) {
        self.out.truncate(self.out.trim_end_matches([' ', '\t']).len());
    }
    /// Where `out` can be cut at `at`, an offset kept when a tag opened: a tag closed out of order may have rewritten
    /// what came before since, so `at` can be past the end or inside a character.
    fn cut(&self, at: usize) -> usize {
        let mut at = at.min(self.out.len());
        while !self.out.is_char_boundary(at) {
            at -= 1;
        }
        at
    }
    fn newline(&mut self) {
        if self.opening() {
            return;
        }
        self.trim_end();
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }
    fn blank(&mut self) {
        if self.opening() {
            return;
        }
        self.trim_end();
        if self.out.is_empty() {
            return;
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }
    fn write(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.pre > 0 {
            let text = if self.fresh { text.strip_prefix("\r\n").or_else(|| text.strip_prefix('\n')).unwrap_or(text) } else { text };
            self.out.push_str(text);
            self.fresh = false;
            return;
        }
        let words = words(text);
        self.out.push_str(if self.out.is_empty() || self.out.ends_with(['\n', ' ']) {
            words.strip_prefix(' ').unwrap_or(&words)
        } else {
            &words
        });
    }
    fn open(&mut self, name: &str, a: &Attributes, self_closing: bool, base: &str) {
        if let Some(level) = level(name) {
            self.blank();
            self.out.push_str(&format!("{} ", "#".repeat(level)));
            self.heading = true;
            return;
        }
        match name {
            "br" => {
                if self.pre > 0 {
                    self.out.push('\n');
                } else if self.heading {
                    self.write(" ");
                } else if !self.opening() {
                    self.trim_end();
                    self.out.push('\n');
                }
            }
            "hr" => {
                self.blank();
                self.out.push_str("---");
                self.blank();
            }
            "p" => {
                if !self.lists.is_empty() {
                    self.newline();
                } else {
                    self.blank();
                }
            }
            "pre" => {
                self.blank();
                self.out.push_str(&format!("```{}\n", language(a)));
                self.pre += 1;
                self.fresh = true;
            }
            "code" => {
                if self.pre == 0 {
                    self.codes.push(self.out.len());
                    self.out.push('`');
                } else {
                    let named = language(a);
                    if !named.is_empty() && self.out.ends_with("```\n") {
                        self.out.pop();
                        self.out.push_str(&named);
                        self.out.push('\n');
                    }
                }
            }
            "sup" | "sub" => self.scripts.push((if name == "sup" { '^' } else { '_' }, self.out.len())),
            "ul" | "ol" => {
                if self.lists.is_empty() {
                    self.blank();
                } else {
                    self.newline();
                }
                let named = trim(attr(a, "start"));
                let digits: String = named
                    .chars()
                    .enumerate()
                    .take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && matches!(c, '+' | '-')))
                    .map(|(_, c)| c)
                    .collect();
                let start = digits.parse::<i64>().ok().filter(|n| *n != 0).unwrap_or(1);
                self.lists.push((name == "ol", start - 1));
            }
            "li" => {
                self.newline();
                let indent = "  ".repeat(self.lists.len().saturating_sub(1));
                let marker = match self.lists.last_mut() {
                    Some((true, index)) => {
                        *index += 1;
                        format!("{index}. ")
                    }
                    _ => "- ".into(),
                };
                self.out.push_str(&indent);
                self.out.push_str(&marker);
            }
            "dt" => self.newline(),
            "dd" => {
                self.newline();
                self.out.push_str("  ");
            }
            "blockquote" => {
                self.blank();
                self.quotes.push(self.out.len());
            }
            "table" => self.blank(),
            "tr" => {
                self.newline();
                self.cells.push(0);
            }
            "td" | "th" => {
                let count = self.cells.last().copied().unwrap_or(0);
                if let Some(last) = self.cells.last_mut() {
                    *last = count + 1;
                }
                if count > 0 {
                    self.trim_end();
                    self.out.push_str(" | ");
                }
            }
            "a" => {
                if !self_closing {
                    self.links.push((absolute(attr(a, "href"), base), self.out.len()));
                }
            }
            "img" => {
                let alt = words(attr(a, "alt"));
                let alt = trim(&alt);
                if utf16_len(&alt.chars().filter(|c| c.is_alphanumeric()).collect::<String>()) < 3 || attr(a, "width") == "1" {
                    return;
                }
                static MATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[a-zA-Z]+|^\$|displaystyle").unwrap());
                static CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)(math|tex|latex)").unwrap());
                if MATH.is_match(alt) || CLASS.is_match(attr(a, "class")) {
                    self.write(&format!(" {} ", if alt.starts_with('$') { alt.into() } else { format!("${alt}$") }));
                    return;
                }
                if let Some(src) = absolute(a.get("src").or_else(|| a.get("data-src")).map_or("", String::as_str), base) {
                    self.write(&format!(" ![{}]({src}) ", alt.replace(['[', ']'], "")));
                }
            }
            _ => {
                if block(name) {
                    self.newline();
                }
            }
        }
    }
    fn close(&mut self, name: &str) {
        if level(name).is_some() {
            self.heading = false;
            self.blank();
            return;
        }
        match name {
            "p" => {
                if self.lists.is_empty() {
                    self.blank();
                } else {
                    self.newline();
                }
            }
            "pre" => {
                if self.pre == 0 {
                    return;
                }
                self.pre -= 1;
                self.fresh = false;
                if !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
                self.out.push_str("```");
                self.blank();
            }
            "code" => {
                if self.pre > 0 {
                    return;
                }
                if let Some(start) = self.codes.pop() {
                    let start = self.cut(start);
                    if self.out.len() == start + 1 {
                        self.out.truncate(start);
                    } else {
                        self.out.push('`');
                    }
                }
            }
            "sup" | "sub" => {
                if let Some((mark, start)) = self.scripts.pop() {
                    let inside = self.out.split_off(self.cut(start));
                    let inside = trim(&inside);
                    if inside.is_empty() {
                        return;
                    }
                    if self.out.ends_with(' ') {
                        self.out.pop();
                    }
                    self.out.push(mark);
                    let simple = (1..=3).contains(&inside.len())
                        && inside.chars().all(|c| c.is_ascii_alphanumeric() || "_.+-".contains(c))
                        && !inside.starts_with(['+', '-']);
                    if simple {
                        self.out.push_str(inside);
                    } else {
                        self.out.push('(');
                        self.out.push_str(inside);
                        self.out.push(')');
                    }
                }
            }
            "ul" | "ol" => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                } else {
                    self.newline();
                }
            }
            "li" | "dt" | "dd" => self.newline(),
            "tr" => {
                self.cells.pop();
                self.newline();
            }
            "table" => {
                self.cells.clear();
                self.blank();
            }
            "blockquote" => {
                if let Some(start) = self.quotes.pop() {
                    self.trim_end();
                    let quoted = self.out.split_off(self.cut(start));
                    let quoted = trim(&quoted)
                        .split('\n')
                        .map(|line| if line.is_empty() { ">".into() } else { format!("> {line}") })
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.out.push_str(&quoted);
                    self.blank();
                }
            }
            "a" => {
                if let Some((href, start)) = self.links.pop() {
                    let inside = self.out.split_off(self.cut(start));
                    let text = words(&inside);
                    let text = trim(&text);
                    if self.pre == 0
                        && href.as_ref().is_some_and(|href| {
                            !text.is_empty()
                                && utf16_len(text) <= 200
                                && text != href
                                && !text.starts_with("![")
                                && !self.linked.contains(href)
                        })
                    {
                        let href = href.unwrap();
                        self.linked.insert(href.clone());
                        let lead: String = inside.chars().take_while(|c| c.is_whitespace()).collect();
                        self.out.push_str(if lead.contains('\n') {
                            "\n"
                        } else if !lead.is_empty() {
                            " "
                        } else {
                            ""
                        });
                        self.out.push_str(&format!("[{}]({href})", text.replace(['[', ']'], "")));
                    } else {
                        self.out.push_str(&inside);
                    }
                }
            }
            _ => {
                if block(name) {
                    self.newline();
                }
            }
        }
    }
}
/// Markdown-ish text from HTML; `base` resolves its links.
pub fn html_to_text(html: &str, base: &str) -> String {
    let mut out = Builder::default();
    let mut skipping: Option<(String, usize)> = None;
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while at < html.len() {
        let text_end = html[at..].find('<').map_or(html.len(), |v| at + v);
        if text_end > at {
            if skipping.is_none() {
                out.write(&decoded_entities(&html[at..text_end]));
            }
            at = text_end;
            if at == html.len() {
                break;
            }
        }
        let tail = &html[at..];
        if tail.starts_with("<!--") {
            at = tail[4..].find("-->").map_or(html.len(), |v| at + 4 + v + 3);
            continue;
        }
        if tail.starts_with("<![CDATA[") {
            at = tail.find("]]>").map_or(html.len(), |v| at + v + 3);
            continue;
        }
        if tail.starts_with("<!") || tail.starts_with("<?") {
            at = tail.find('>').map_or(html.len(), |v| at + v + 1);
            continue;
        }
        if let Some((name, length)) = close_tag(tail) {
            at += length;
            if let Some((skipped, depth)) = &mut skipping {
                if name.as_ref() == skipped {
                    *depth -= 1;
                    if *depth == 0 {
                        skipping = None;
                    }
                }
                continue;
            }
            out.close(&name);
            continue;
        }
        let Some((name, attributes_text, slash, length)) = open_tag(tail) else {
            if skipping.is_none() {
                out.write("<");
            }
            at += 1;
            continue;
        };
        at += length;
        let self_closing = slash || void(&name);
        if raw(&name) && !self_closing {
            let end = lower[at..].find(&format!("</{name}")).map(|v| at + v);
            let inside = &html[at..end.unwrap_or(html.len())];
            at = end.and_then(|end| html[end..].find('>').map(|v| end + v + 1)).unwrap_or(html.len());
            if skipping.is_none() && name == "textarea" {
                out.write(&decoded_entities(inside));
            }
            continue;
        }
        if let Some((skipped, depth)) = &mut skipping {
            if name.as_ref() == skipped && !self_closing {
                *depth += 1;
            }
            continue;
        }
        let attrs = attributes(attributes_text);
        if name == "math" && !attr(&attrs, "alttext").is_empty() {
            out.write(&format!(" ${}$ ", trim(attr(&attrs, "alttext"))));
        }
        if skip(&name) || (hidden(&attrs) && !self_closing) {
            if !self_closing {
                skipping = Some((name.into_owned(), 1));
            }
            continue;
        }
        out.open(&name, &attrs, self_closing, base);
    }
    while !out.links.is_empty() {
        out.close("a");
    }
    static TRAILING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+\n").unwrap());
    static NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
    trim(&NEWLINES.replace_all(&TRAILING.replace_all(&out.out, "\n"), "\n\n")).into()
}
fn heading(html: &str) -> (Option<String>, Option<String>) {
    static META: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<meta\b([^>]*)>").unwrap());
    static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    let clean = |text: String| {
        let decoded = decoded_entities(&text);
        let t = words(&decoded);
        let t = trim(&t);
        (!t.is_empty()).then(|| t.into())
    };
    let meta = |key: &str| {
        META.captures_iter(html).find_map(|c| {
            let a = attributes(&c[1]);
            (a.get("name").or_else(|| a.get("property")).map_or("", String::as_str).to_lowercase() == key)
                .then(|| a.get("content").cloned())
                .flatten()
        })
    };
    let title = TITLE.captures(html).and_then(|c| clean(c[1].into())).or_else(|| meta("og:title").and_then(clean));
    let description = meta("description").and_then(clean).or_else(|| meta("og:description").and_then(clean));
    (title, description)
}
fn main_part(html: &str) -> Option<&str> {
    static MAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<main\b[^>]*>").unwrap());
    static ARTICLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<article\b[^>]*>").unwrap());
    let lower = html.to_ascii_lowercase();
    if let Some(main) = MAIN.find(html) {
        if let Some(end) = lower.rfind("</main>") {
            if end > main.start() {
                return Some(&html[main.start()..end + 7]);
            }
        }
    }
    let mut articles = ARTICLE.find_iter(html);
    if let Some(article) = articles.next() {
        if articles.next().is_none() {
            if let Some(end) = lower.rfind("</article>") {
                if end > article.start() {
                    return Some(&html[article.start()..end + 10]);
                }
            }
        }
    }
    None
}
pub fn read_html(html: &str, base: &str) -> PageText {
    static BASE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)<base\b[^>]*href\s*=\s*["']([^"']+)["']"#).unwrap());
    static SCRIPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<script\b").unwrap());
    let (title, description) = heading(html);
    let from = BASE.captures(html).and_then(|c| absolute(&decode_entities(&c[1]), base)).unwrap_or_else(|| base.into());
    let mut text = main_part(html).map_or_else(String::new, |main| html_to_text(main, &from));
    if utf16_len(&text) < 300 {
        let whole = html_to_text(html, &from);
        if utf16_len(&whole) > utf16_len(&text) {
            text = whole;
        }
    }
    let scripted = utf16_len(&text) < 250 && SCRIPT.is_match(html);
    PageText { title, description, text, scripted }
}
