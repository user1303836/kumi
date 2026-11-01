//! Max's reference pages (its .maxref.xml files) as Kumi reads them: their elements, and an object's page told in a
//! few lines (what it's for, its inlets and outlets for the arguments it's given, its arguments, what each inlet and
//! outlet is for, its messages, its attributes and what to see also). A page is read from this machine's Max when
//! it's asked for; none is carried in Kumi.

use std::path::{Component, Path, PathBuf};

use super::catalog::catalog;
use super::reference::{atoms, Object, Reference};

/// An element of a reference page: its tag, its attributes, and what's written inside it, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Element {
    pub tag: String,
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    /// Its first element with the tag.
    pub fn child(&self, tag: &str) -> Option<&Element> {
        self.children(tag).next()
    }

    /// Its elements with the tag, in order.
    pub fn children<'a: 'b, 'b>(&'a self, tag: &'b str) -> impl Iterator<Item = &'a Element> + 'b {
        self.elements().filter(move |element| element.tag == tag)
    }

    fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }

    /// What's written inside it as plain words, a line break a space; a page's placeholder (TEXT_HERE) is no text.
    pub fn text(&self) -> String {
        fn gather(element: &Element, words: &mut String) {
            for node in &element.children {
                match node {
                    Node::Text(text) => words.push_str(text),
                    Node::Element(inner) if inner.tag == "br" => words.push(' '),
                    Node::Element(inner) => gather(inner, words),
                }
            }
        }
        let mut words = String::new();
        gather(self, &mut words);
        let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
        if placeholder(&words) {
            String::new()
        } else {
            words
        }
    }
}

/// How deep a page's elements are read inside one another. Max's pages nest a few deep: one deeper isn't Max's, and
/// what's done with a page (its text, a copy, letting it go) goes as deep as it does.
const NESTING: usize = 64;

/// A page's top element (its c74object), read as the pages are written: comments and declarations are passed over,
/// a closing tag nothing opened is too, and what's left open at the end is closed. None for text with no element, or
/// with elements nested deeper than [`NESTING`].
pub fn read(text: &str) -> Option<Element> {
    let mut open: Vec<Element> = vec![Element::default()];
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        add_text(&mut open, &rest[..at]);
        rest = &rest[at..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
        } else if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            // Its text as it's written: nothing in it is markup.
            let end = cdata.find("]]>").unwrap_or(cdata.len());
            if let Some(element) = open.last_mut().filter(|_| end > 0) {
                element.children.push(Node::Text(cdata[..end].to_string()));
            }
            rest = cdata.get(end + 3..).unwrap_or("");
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
        } else if let Some(closing) = rest.strip_prefix("</") {
            let end = closing.find('>').unwrap_or(closing.len());
            let tag = closing[..end].trim();
            if let Some(at) = open.iter().rposition(|element| element.tag == tag).filter(|at| *at > 0) {
                while open.len() > at {
                    close(&mut open);
                }
            }
            rest = closing.get(end + 1..).unwrap_or("");
        } else if rest[1..].starts_with(|c: char| c.is_alphabetic() || c == '_') {
            let end = tag_end(rest);
            let (inside, empty) = match rest[1..end].strip_suffix('/') {
                Some(inside) => (inside, true),
                None => (&rest[1..end], false),
            };
            let element = element(inside);
            if empty {
                open.last_mut()?.children.push(Node::Element(element));
            } else if open.len() > NESTING {
                return None;
            } else {
                open.push(element);
            }
            rest = rest.get(end + 1..).unwrap_or("");
        } else {
            // A < that starts no tag is text.
            add_text(&mut open, "<");
            rest = &rest[1..];
        }
    }
    add_text(&mut open, rest);
    while open.len() > 1 {
        close(&mut open);
    }
    open.pop()?.elements().next().cloned()
}

fn add_text(open: &mut [Element], text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(element) = open.last_mut() {
        let text = if text.trim().is_empty() { " ".to_string() } else { unescape(text) };
        element.children.push(Node::Text(text));
    }
}

/// The innermost open element, closed into the one around it.
fn close(open: &mut Vec<Element>) {
    if let Some(done) = open.pop() {
        match open.last_mut() {
            Some(around) => around.children.push(Node::Element(done)),
            None => open.push(done),
        }
    }
}

/// Where a tag that starts `text` ends: its >, past any in its quoted values.
fn tag_end(text: &str) -> usize {
    let mut quote = None;
    for (at, c) in text.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if open == c => quote = None,
            (None, '>') => return at,
            _ => {}
        }
    }
    text.len()
}

/// An element from what's inside its tag: its name, then name="value" or name='value' pairs.
fn element(inside: &str) -> Element {
    let inside = inside.trim();
    let (tag, mut rest) = inside.split_at(inside.find(char::is_whitespace).unwrap_or(inside.len()));
    let mut attributes = Vec::new();
    while let Some(equals) = rest.find('=') {
        let name = rest[..equals].trim();
        let value = rest[equals + 1..].trim_start();
        let Some(quote) = value.chars().next().filter(|c| matches!(c, '"' | '\'')) else { break };
        let Some(end) = value[1..].find(quote) else { break };
        attributes.push((name.to_string(), unescape(&value[1..end + 1])));
        rest = &value[end + 2..];
    }
    Element { tag: tag.to_string(), attributes, children: Vec::new() }
}

/// Text with the page's entities read: &lt; &gt; &quot; &apos; &amp; and numbered ones (&#8217;, &#x2019;).
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest.find(';').filter(|end| *end <= 10).map(|end| (end, &rest[1..end]));
        let read = entity.and_then(|(end, name)| {
            let c = match name {
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "amp" => '&',
                _ => {
                    let number = name.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((end, c))
        });
        match read {
            Some((end, c)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A word a page leaves for its writers to fill in (TEXT_HERE, OBJARG_NAME, INLET_TYPE, ARG_NAME_0).
fn placeholder(value: &str) -> bool {
    value.contains('_')
        && value.chars().any(|c| c.is_ascii_uppercase())
        && value.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// What an attribute's own attributes say (its label, category, default…).
pub fn meta<'a>(attribute: &'a Element, name: &str) -> Option<&'a str> {
    attribute.child("attributelist")?.children("attribute").find(|meta| meta.attribute("name") == Some(name))?.attribute("value")
}

/// One of an object's inlets or outlets as its page says it: what it takes or sends ("signal", "int"; empty where the
/// page doesn't say), and what it's for.
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub kind: String,
    pub digest: String,
}

/// A page's inlets or outlets (`kind` "inlet" or "outlet"). A Live API object's page writes them in a list of its own,
/// each with a description: its first sentence says what the port is for.
pub fn ports(page: &Element, kind: &str) -> Vec<Port> {
    let listed = |list: &str| page.child(list).map(|list| list.children(kind).collect::<Vec<_>>()).unwrap_or_default();
    let mut found = listed(&format!("{kind}list"));
    let api = found.is_empty();
    if api {
        found = listed(&format!("api{kind}list"));
    }
    found
        .into_iter()
        .map(|port| {
            let kind = port.attribute("type").filter(|kind| !kind.contains("_TYPE")).unwrap_or("").to_string();
            let digest = port.child("digest").map(Element::text).unwrap_or_default();
            let digest =
                if digest.is_empty() && api { sentence(&port.child("description").map(Element::text).unwrap_or_default()) } else { digest };
            Port { kind, digest }
        })
        .collect()
}

/// The first sentence of a text.
fn sentence(text: &str) -> String {
    text.find(". ").map_or(text, |end| &text[..=end]).trim().to_string()
}

/// How long a line's description may be, in characters: past it, it's cut at a word.
const BRIEF: usize = 100;
/// How many messages, and how many attributes, an object's page tells in full; the rest by name.
const IN_FULL: usize = 16;
/// How many attributes that only change how it looks, and how many pages to see also, it names.
const NAMED: usize = 20;
/// How many names it offers for a name this Max doesn't have.
const NEAREST: usize = 5;
/// The messages that carry an object's input: what they hold is what it works on, so their arguments go unsaid.
const INPUTS: [&str; 7] = ["bang", "int", "float", "list", "anything", "symbol", "signal"];
/// The categories of attributes that only change how a box looks or is described.
pub const LOOKS: [&str; 10] = ["Appearance", "Color", "Description", "Font", "Image", "Name", "Pictures", "Ruler", "Shadow", "View"];

/// What to say when this machine has no Max to read.
pub const NO_MAX: &str = "Max wasn't found on this machine, so Kumi can't read its reference pages: \
docs.cycling74.com/reference/<object>/ has them on the web.";

/// What to say when KUMI_MAX is set and holds no Max: it's used alone, so it's the thing to fix. None when it isn't set.
pub fn wrong_kumi_max() -> Option<String> {
    let named = std::env::var_os("KUMI_MAX").filter(|named| !named.is_empty())?;
    Some(format!(
        "KUMI_MAX names no Max ({}): point it at Max's C74 folder or a Max that holds one, or unset it.",
        Path::new(&named).display()
    ))
}

/// What to say when there's no Max to read: why, if KUMI_MAX is why.
pub fn no_max() -> String {
    wrong_kumi_max().unwrap_or_else(|| NO_MAX.to_string())
}

/// A description cut to `most` characters at a word.
fn brief(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let cut: String = text.chars().take(most).collect();
    format!("{}…", cut.rsplit_once(' ').map_or(cut.as_str(), |(kept, _)| kept).trim_end_matches([',', ';', ':']))
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// What this machine's Max says of the objects boxes' texts name, a page for each.
/// A page's text: its bytes as UTF-8, with any that aren't replaced (a page saved in Latin-1, say), so it's read
/// rather than passed over. None when it can't be read.
pub(crate) fn text_of(file: &Path) -> Option<String> {
    std::fs::read(file).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Where a page kept with the reference is, in its C74 folder: folders and a file within it, never the folder itself or
/// a way out of it (a reference.json changed since Kumi wrote it, say). None for no page.
fn within(root: &Path, page: &str) -> Option<PathBuf> {
    let page = Path::new(page);
    let inside = page.components().all(|part| matches!(part, Component::Normal(_)));
    (inside && !page.as_os_str().is_empty()).then(|| root.join(page))
}

pub fn describe_all(reference: &Reference, texts: &[String]) -> String {
    texts.iter().map(|text| describe(reference, text)).collect::<Vec<_>>().join("\n\n")
}

/// What this machine's Max says of the object a box's text names ("makenote 60 100 250"), in a few lines. A name it
/// doesn't have gets the nearest names it has.
pub fn describe(reference: &Reference, text: &str) -> String {
    let text = text.trim().trim_start_matches('[').trim_end_matches(']').trim();
    let words = atoms(text);
    let Some(class) = words.first() else { return "An object's name is missing: give it as it's typed in a box.".into() };
    let Some(object) = reference.object(class) else {
        let near = nearest(reference, class);
        let names = near.iter().map(|name| format!("[{name}]")).collect::<Vec<_>>().join(", ");
        return if near.is_empty() { format!("No [{class}] in this Max.") } else { format!("No [{class}] in this Max. Nearest: {names}.") };
    };
    let read_page = |object: &Object| within(&reference.root, &object.page).and_then(|path| text_of(&path)).and_then(|text| read(&text));
    let own = read_page(object);
    // An object with modes ([zl lookup]) has a page for each that this Max writes as its own name (zl.lookup): one whose
    // page takes a mode message, or lists no argument. Where the first argument is a name ([dict view]), the word is
    // that name.
    let modes = own.as_ref().is_some_and(|page: &Element| {
        let mut methods = page.child("methodlist").into_iter().flat_map(|list| list.children("method"));
        let mut arguments = page.child("objarglist").into_iter().flat_map(|list| list.children("objarg"));
        methods.any(|method| method.attribute("name") == Some("mode"))
            || !arguments.any(|argument| real(argument.attribute("name")).is_some())
    });
    let mode = words
        .get(1)
        .filter(|_| modes)
        .map(|word| format!("{class}.{word}"))
        .filter(|mode| reference.objects.get(mode).is_some_and(|mode| !mode.page.is_empty()));
    let (shown, paged) = match &mode {
        Some(mode) => (format!("{class} {}", words[1]), &reference.objects[mode]),
        None => (class.clone(), object),
    };
    let page = if mode.is_some() { read_page(paged) } else { own };
    let named = match &mode {
        Some(mode) => page.as_ref().and_then(|page| page.attribute("name")).unwrap_or(mode),
        None => page.as_ref().and_then(|page| page.attribute("name")).unwrap_or(catalog().canonical(class)),
    };
    let digest = page.as_ref().and_then(|page| page.child("digest")).map(Element::text).unwrap_or_else(|| paged.digest.clone());
    let mut lines = vec![match (named == shown.as_str(), digest.is_empty()) {
        (true, false) => format!("[{shown}] {digest}"),
        (true, true) => format!("[{shown}]"),
        (false, false) => format!("[{shown}] is [{named}]: {digest}"),
        (false, true) => format!("[{shown}] is [{named}]"),
    }];
    let arguments = words[1..].iter().take_while(|word| !word.starts_with('@')).count();
    let ports = reference.ports(text);
    lines.push(match &ports {
        Some(ports) => format!("[{text}] has {} and {}.", count(ports.inlets, "inlet"), count(ports.outlets, "outlet")),
        None if catalog().ports_from_contents(class) => format!("[{text}]: its inlets and outlets come from what's inside it."),
        None if object.seen == 0 && !object.listed() => format!("[{text}]: its page lists no inlets or outlets, so its cords decide them."),
        None if arguments == 0 => {
            format!("[{text}]: its cords decide its inlets and outlets; Max's help shows no count that holds for it bare.")
        }
        None => format!(
            "[{text}]: its cords decide its inlets and outlets; Max's help shows no count that holds for it with {}.",
            count(arguments, "argument")
        ),
    });
    match &page {
        Some(page) => tell(page, named, ports.as_ref().map(|ports| (ports.inlets, ports.outlets)), &mut lines),
        None if object.module == "abstraction" => lines.push("An abstraction Max ships: it has no reference page.".into()),
        None if object.page.is_empty() => lines.push("Max has no reference page for it: its help patchers show it.".into()),
        None => lines.push("Its reference page couldn't be read.".into()),
    }
    lines.join("\n")
}

/// What a page says of its object, after its name and ports: its arguments, inlets, outlets, messages, attributes and
/// what to see also. `ports` is the box's inlets and outlets, where they're known: a page lists them for the object
/// bare, so a box whose count differs, or isn't known, is told the list is the page's.
fn tell(page: &Element, named: &str, ports: Option<(usize, usize)>, lines: &mut Vec<String>) {
    let arguments: Vec<String> = page
        .child("objarglist")
        .into_iter()
        .flat_map(|list| list.children("objarg"))
        .filter_map(|argument| {
            let name = real(argument.attribute("name"))?;
            let mut about: Vec<String> = real(argument.attribute("type")).map(str::to_string).into_iter().collect();
            about.push(if argument.attribute("optional") == Some("1") { "optional" } else { "required" }.into());
            about.extend(real(argument.attribute("units")).map(str::to_string));
            about.extend(real(argument.attribute("default")).map(|default| format!("default {default}")));
            let digest = brief(&argument.child("digest").map(Element::text).unwrap_or_default(), BRIEF);
            Some(format!("- {name} ({}){}", about.join(", "), if digest.is_empty() { String::new() } else { format!(": {digest}") }))
        })
        .collect();
    if arguments.is_empty() {
        lines.push("Arguments: none listed.".into());
    } else {
        lines.push("Arguments:".into());
        lines.extend(arguments);
    }
    for (kind, title) in [("inlet", "Inlets"), ("outlet", "Outlets")] {
        let listed = self::ports(page, kind);
        if listed.is_empty() {
            continue;
        }
        let has = ports.map(|(inlets, outlets)| if kind == "inlet" { inlets } else { outlets });
        lines.push(match has {
            Some(has) if has == listed.len() => format!("{title}:"),
            Some(has) => format!("{title}, as its page lists them for [{named}] bare (this box has {has}):"),
            None => format!("{title}, as its page lists them for [{named}] bare:"),
        });
        for (at, port) in listed.iter().enumerate() {
            let signal = if port.kind.contains("signal") { format!(" ({})", port.kind) } else { String::new() };
            let digest = brief(&port.digest, BRIEF);
            lines.push(format!("- {at}{signal}{}", if digest.is_empty() { String::new() } else { format!(": {digest}") }));
        }
    }
    let methods: Vec<&Element> = page
        .child("methodlist")
        .into_iter()
        .flat_map(|list| list.children("method"))
        .filter(|method| real(method.attribute("name")).is_some_and(|name| !name.starts_with('(')))
        .collect();
    if !methods.is_empty() {
        lines.push("Messages:".into());
        let mut rest = Vec::new();
        for (at, method) in methods.iter().enumerate() {
            let name = method.attribute("name").unwrap_or_default();
            if at >= IN_FULL {
                rest.push(name.to_string());
                continue;
            }
            let mut said = vec![name.to_string()];
            if !INPUTS.contains(&name) {
                said.extend(
                    method.child("arglist").map(|list| list.elements().filter_map(argument).collect::<Vec<_>>()).unwrap_or_default(),
                );
            }
            let digest = brief(&method.child("digest").map(Element::text).unwrap_or_default(), BRIEF);
            lines.push(format!("- {}{}", said.join(" "), if digest.is_empty() { String::new() } else { format!(": {digest}") }));
        }
        if !rest.is_empty() {
            lines.push(format!("Also: {}.", named_up_to(rest)));
        }
    }
    let (mut full, mut rest, mut looks) = (Vec::new(), Vec::new(), Vec::new());
    for attribute in page.child("attributelist").into_iter().flat_map(|list| list.children("attribute")) {
        let Some(name) = real(attribute.attribute("name")) else { continue };
        if meta(attribute, "invisible") == Some("1") || meta(attribute, "obsolete") == Some("1") {
            continue;
        }
        if meta(attribute, "category").is_some_and(|category| LOOKS.contains(&category)) {
            looks.push(format!("@{name}"));
        } else if full.len() >= IN_FULL {
            rest.push(format!("@{name}"));
        } else {
            full.push(attribute_line(attribute, name));
        }
    }
    if !full.is_empty() {
        lines.push("Attributes:".into());
        lines.append(&mut full);
    }
    if !rest.is_empty() {
        lines.push(format!("Also: {}.", named_up_to(rest)));
    }
    if !looks.is_empty() {
        lines.push(format!("Looks: {}.", named_up_to(looks)));
    }
    // What an MC object takes for every channel, and a Jitter object's own attributes, each in a list of its own.
    let listed = |list: &str, item: &str, prefix: &str| -> Vec<String> {
        page.children(list)
            .flat_map(|list| list.children(item))
            .filter_map(|item| real(item.attribute("name")))
            .map(|name| format!("{prefix}{name}"))
            .collect()
    };
    for (said, names) in [
        ("MC attributes, for each channel", listed("mcwrapper", "mcwrapperattr", "@")),
        ("MC messages, for each channel", listed("mcwrapper", "mcwrappermethod", "")),
        ("Jitter attributes", listed("jitterattributelist", "jitterattribute", "@")),
    ] {
        if !names.is_empty() {
            lines.push(format!("{said}: {}.", named_up_to(names)));
        }
    }
    let mut also: Vec<String> = Vec::new();
    for seen in page.child("seealsolist").into_iter().flat_map(|list| list.children("seealso")) {
        let name = seen.attribute("name").unwrap_or_default();
        let object = matches!(seen.attribute("type"), None | Some("refpage"));
        if object && !name.is_empty() && name != named && !also.iter().any(|kept| kept == name) {
            also.push(name.to_string());
        }
    }
    if !also.is_empty() {
        lines.push(format!("See also: {}.", named_up_to(also.iter().map(|name| format!("[{name}]")).collect())));
    }
}

/// Names in a line, the first [`NAMED`] of them, then how many more there are.
fn named_up_to(names: Vec<String>) -> String {
    let more = names.len().saturating_sub(NAMED);
    let shown = names.into_iter().take(NAMED).collect::<Vec<_>>().join(", ");
    if more > 0 {
        format!("{shown} and {more} more")
    } else {
        shown
    }
}

/// A value a page gives, not one it leaves for its writers to fill in.
fn real(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty() && !placeholder(value))
}

/// A message's argument as its page lists it: [name] when it can be left out; a group of them together.
fn argument(item: &Element) -> Option<String> {
    let optional = item.attribute("optional") == Some("1");
    let said = match item.tag.as_str() {
        "arg" => item.attribute("name").filter(|name| !name.is_empty() && !placeholder(name))?.to_string(),
        "arggroup" => {
            let inner: Vec<String> = item.elements().filter_map(argument).collect();
            if inner.is_empty() {
                return None;
            }
            inner.join(" ")
        }
        _ => return None,
    };
    Some(if optional { format!("[{said}]") } else { said })
}

/// An attribute in a line: its type, its default, the values it takes (by number for one set by number), and what
/// it's for.
fn attribute_line(attribute: &Element, name: &str) -> String {
    let kind = attribute.attribute("type").unwrap_or_default();
    let mut about: Vec<String> = vec![kind.to_string()];
    if meta(attribute, "style") == Some("onoff") {
        about.push("0 or 1".into());
    }
    if let Some(default) = meta(attribute, "default").filter(|default| !default.is_empty()) {
        about.push(format!("default {default}"));
    }
    let values: Vec<&str> = attribute
        .child("attributelist")
        .into_iter()
        .flat_map(|list| list.children("attribute"))
        .filter(|meta| meta.attribute("name") == Some("enumvals"))
        .flat_map(|meta| meta.children("enumlist").flat_map(|list| list.children("enum")))
        .filter_map(|value| value.attribute("name"))
        .collect();
    if values.iter().any(|value| !value.trim().is_empty()) {
        // Numbered as Max numbers them, an empty one counted but not said.
        let numbered = kind == "int" || meta(attribute, "style") == Some("enumindex");
        let values: Vec<String> = values
            .iter()
            .enumerate()
            .filter(|(_, value)| !value.trim().is_empty())
            .map(|(at, value)| if numbered { format!("{at} = {value}") } else { value.to_string() })
            .collect();
        about.push(format!("values {}", named_up_to(values)));
    }
    let digest = brief(&attribute.child("digest").map(Element::text).unwrap_or_default(), BRIEF);
    format!("- @{name} ({}){}", about.join(", "), if digest.is_empty() { String::new() } else { format!(": {digest}") })
}

/// The names this Max has nearest to one it doesn't: a letter or two off, or one starting the other.
pub fn nearest<'a>(reference: &'a Reference, class: &str) -> Vec<&'a str> {
    let wanted = class.to_lowercase();
    let length = wanted.chars().count();
    let most = (length / 3).max(2);
    let mut found: Vec<(usize, &str)> = reference
        .objects
        .keys()
        .filter_map(|name| {
            let lower = name.to_lowercase();
            let starts = wanted.len() >= 3 && lower.len() >= 3 && (lower.starts_with(&wanted) || wanted.starts_with(&lower));
            // Names further apart in length than that are further apart in letters too: they're never counted.
            if !starts && lower.chars().count().abs_diff(length) > most {
                return None;
            }
            let distance = edits(&wanted, &lower);
            (distance <= most || starts).then_some((distance, name.as_str()))
        })
        .collect();
    found.sort();
    found.into_iter().take(NEAREST).map(|(_, name)| name).collect()
}

/// How many letters to add, take out or change to make one word the other.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(x != *y)).min(row[j] + 1).min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::super::reference::{learn, Count, Object};
    use super::*;

    #[test]
    fn a_page_is_read_as_its_elements() {
        let page = read(concat!(
            "<?xml version='1.0'?><!-- a note --><c74object module='max' category=\"Timing\" name=\"a&amp;b &gt;=\">",
            "<digest>\n\t One <o>thing</o><br/>then &lt;another&gt;&#8217;s </digest><list><item id='1'/><item id='2'>x</item></list>",
            "<description>TEXT_HERE</description></c74object>"
        ))
        .unwrap();
        assert_eq!((page.tag.as_str(), page.attribute("name"), page.attribute("module")), ("c74object", Some("a&b >="), Some("max")));
        assert_eq!(page.child("digest").map(Element::text).as_deref(), Some("One thing then <another>’s"));
        assert_eq!(page.child("list").unwrap().children("item").map(|item| item.attribute("id").unwrap()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(page.child("description").map(Element::text).as_deref(), Some(""), "a page's placeholder is no text");
        let stray = read("<a><b>x</c></b> 1 < 2</a>").unwrap();
        assert_eq!(
            (stray.child("b").map(Element::text).as_deref(), stray.text().as_str()),
            (Some("x"), "x 1 < 2"),
            "a closing tag nothing opened"
        );
        assert_eq!(read("no element"), None);
        let cdata = read("<a>x <![CDATA[1 < 2 > 0 & y]]> z</a>").unwrap();
        assert_eq!(cdata.text(), "x 1 < 2 > 0 & y z", "a CDATA section is its text as written");
        // Nested as deep as Kumi reads, and one deeper; and as deep as a page could make it, which isn't read at all.
        let nested = |depth: usize| format!("{}x{}", "<a>".repeat(depth), "</a>".repeat(depth));
        assert_eq!(read(&nested(NESTING)).map(|page| page.text()).as_deref(), Some("x"));
        assert_eq!(read(&nested(NESTING + 1)), None);
        assert_eq!(read(&"<a>".repeat(100_000)), None);
    }

    /// A page written for these tests (not Max's): an object that holds numbers back, its attributes' own attributes
    /// inside them.
    const PAGE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<c74object category="Timing" module="max" name="pipe">
  <digest> Hold numbers back <o>for a time</o> </digest>
  <inletlist>
    <inlet id="0" type="INLET_TYPE"><digest>What to hold back</digest></inlet>
    <inlet id="1" type="INLET_TYPE"><digest>How long</digest></inlet>
  </inletlist>
  <outletlist><outlet id="0" type="OUTLET_TYPE"><digest>What was held back</digest></outlet></outletlist>
  <objarglist>
    <objarg name="initial" optional="1" type="list"><digest>Values to start with, then the time</digest></objarg>
    <objarg name="OBJARG_NAME" optional="0" type="OBJARG_TYPE"><digest>TEXT_HERE</digest></objarg>
  </objarglist>
  <methodlist>
    <method name="int"><arglist><arg name="value" optional="0" type="int" /></arglist><digest>Hold it back</digest></method>
    <method name="flush"><arglist /><digest><![CDATA[Send everything held, now]]></digest></method>
    <method name="setcell"><arglist><arg name="position" type="list" /><arg name="ARG_NAME_0" optional="0" type="int" />
      <arggroup optional="1"><arg name="plane" type="literal" /><arg name="number" type="int" /></arggroup></arglist>
      <attributelist><attribute name="introduced" get="1" set="1" type="symbol" size="1" value="9" /></attributelist>
      <digest>Set a cell</digest></method>
    <method name="(mouse)"><digest>Click it</digest></method>
  </methodlist>
  <attributelist>
    <attribute name="quantize" get="1" set="1" type="Time Value" size="1"><digest>Line up with the beat</digest>
      <attributelist>
        <attribute name="label" get="1" set="1" type="symbol" size="1" value="Quantization" />
        <attribute name="default" get="1" set="1" type="atom" size="2" value="0 ticks" />
      </attributelist>
    </attribute>
    <attribute name="mode" get="1" set="1" type="int" size="1"><digest>What a repeat does</digest>
      <attributelist>
        <attribute name="enumvals" get="1" set="1" type="atom" size="2">
          <enumlist><enum name="Keep Both"><digest>TEXT_HERE</digest></enum><enum name="Stop" /></enumlist>
        </attribute>
      </attributelist>
    </attribute>
    <attribute name="hold" get="1" set="1" type="int" size="1"><digest>Hold every number</digest>
      <attributelist><attribute name="style" get="1" set="1" type="symbol" size="1" value="onoff" /></attributelist>
    </attribute>
    <attribute name="color" get="1" set="1" type="float" size="4"><digest>Its colour</digest>
      <attributelist><attribute name="category" get="1" set="1" type="symbol" size="1" value="Color" /></attributelist>
    </attribute>
    <attribute name="old" get="1" set="1" type="int" size="1"><digest>Gone</digest>
      <attributelist><attribute name="obsolete" get="1" set="1" type="int" size="1" value="1" /></attributelist>
    </attribute>
  </attributelist>
  <seealsolist><seealso name="delay" /><seealso name="pipe" /><seealso name="timing" type="userguide" /></seealsolist>
  <mcwrapper><mcwrapperattr name="chans" /><mcwrappermethod name="deviate" /></mcwrapper>
</c74object>"#;

    /// A C74 folder with the page and a help patcher whose instances say how many ports [pipe] has.
    fn max_folder() -> tempfile::TempDir {
        let c74 = tempfile::tempdir().unwrap();
        let pages = c74.path().join("docs").join("refpages").join("max-ref");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("pipe.maxref.xml"), PAGE).unwrap();
        std::fs::create_dir_all(c74.path().join("help")).unwrap();
        let instance = |text: &str, inlets: usize, outlets: usize| serde_json::json!({ "box": { "maxclass": "newobj", "text": text, "numinlets": inlets, "numoutlets": outlets } });
        let help = serde_json::json!({ "patcher": { "appversion": { "major": 9, "minor": 0, "revision": 0 },
            "boxes": [instance("pipe 100", 2, 1), instance("pipe 0 100", 2, 1), instance("pipe 0 0 100", 3, 2)] } });
        std::fs::write(c74.path().join("help").join("pipe.maxhelp"), help.to_string()).unwrap();
        c74
    }

    #[test]
    fn a_kept_page_is_read_from_inside_maxs_folder_alone() {
        let base = tempfile::tempdir().unwrap();
        let c74 = base.path().join("C74");
        let pages = c74.join("docs").join("refpages").join("max-ref");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("pipe.maxref.xml"), PAGE).unwrap();
        let outside = base.path().join("outside.maxref.xml");
        std::fs::write(&outside, PAGE.replace("Hold numbers back", "Read from outside")).unwrap();
        let mut reference = learn(&c74);
        assert!(describe(&reference, "pipe").contains("Hold numbers back"), "its own page, as learned");
        for kept in ["../outside.maxref.xml".to_string(), outside.display().to_string()] {
            reference.objects.get_mut("pipe").unwrap().page = kept.clone();
            let told = describe(&reference, "pipe");
            assert!(!told.contains("Read from outside"), "{kept}: {told}");
        }
        assert_eq!(within(&c74, ""), None, "no page");
        assert_eq!(within(&c74, "docs/x.maxref.xml"), Some(c74.join("docs/x.maxref.xml")));
    }

    #[test]
    fn an_objects_page_tells_its_arguments_ports_messages_and_attributes() {
        let c74 = max_folder();
        let reference = learn(c74.path());
        let kept = serde_json::to_string(&reference).unwrap();
        assert!(kept.contains("pipe.maxref.xml") && !kept.contains("Line up with the beat"), "where the page is, not what it says");
        let page = describe(&reference, "[pipe 250]");
        let lines: Vec<&str> = page.lines().collect();
        assert_eq!(lines[..3], ["[pipe] Hold numbers back for a time", "[pipe 250] has 2 inlets and 1 outlet.", "Arguments:"]);
        for line in [
            "- initial (list, optional): Values to start with, then the time",
            "- 1: How long",
            "- int: Hold it back",
            "- flush: Send everything held, now",
            "- setcell position [plane number]: Set a cell",
            "- @quantize (Time Value, default 0 ticks): Line up with the beat",
            "- @mode (int, values 0 = Keep Both, 1 = Stop): What a repeat does",
            "- @hold (int, 0 or 1): Hold every number",
            "Looks: @color.",
            "See also: [delay].",
            "MC attributes, for each channel: @chans.",
            "MC messages, for each channel: deviate.",
            "Inlets:",
        ] {
            assert!(lines.contains(&line), "{line:?} in:\n{page}");
        }
        for unsaid in ["OBJARG", "ARG_NAME", "(mouse)", "@label", "@default", "@enumvals", "@introduced", "@old", "timing"] {
            assert!(!page.contains(unsaid), "{unsaid:?} isn't in:\n{page}");
        }
        let unheld = describe(&reference, "pipe 0 0 1 250");
        assert!(
            unheld.contains(
                "[pipe 0 0 1 250]: its cords decide its inlets and outlets; Max's help shows no count that holds for it with 4 arguments."
            ),
            "help instances with three arguments disagree with two inlets"
        );
        assert!(unheld.contains("\nInlets, as its page lists them for [pipe] bare:\n"), "the page's list isn't this box's: {unheld}");
        // Read when it's asked for: a page Max changes says what it says now.
        let file = c74.path().join("docs").join("refpages").join("max-ref").join("pipe.maxref.xml");
        std::fs::write(&file, PAGE.replace("Hold numbers back", "Delay numbers")).unwrap();
        assert!(describe(&reference, "pipe 100").starts_with("[pipe] Delay numbers for a time\n[pipe 100] has 2 inlets and 1 outlet."));
        std::fs::remove_file(&file).unwrap();
        assert!(describe(&reference, "pipe").ends_with("Its reference page couldn't be read."));
    }

    #[test]
    fn an_object_with_modes_is_told_from_its_modes_page_and_long_lists_are_named_up_to_twenty() {
        let c74 = max_folder();
        let pages = c74.path().join("docs").join("refpages").join("max-ref");
        let page = |name: &str, digest: &str, inlet: &str, more: &str| {
            format!(
                r#"<c74object name="{name}" module="max"><digest>{digest}</digest><inletlist><inlet id="0"><digest>{inlet}</digest></inlet></inletlist>{more}</c74object>"#
            )
        };
        let methods: String = (1..=30).map(|at| format!(r#"<method name="m{at}"><digest>Do {at}</digest></method>"#)).collect();
        let shapes: String = std::iter::once(String::new())
            .chain((1..=24).map(|at| format!("s{at}")))
            .map(|name| format!(r#"<enum name="{name}" />"#))
            .collect();
        let more = format!(
            r#"<methodlist>{methods}</methodlist><attributelist><attribute name="shape" type="int"><digest>Its shape</digest><attributelist><attribute name="enumvals"><enumlist>{shapes}</enumlist></attribute></attributelist></attribute></attributelist>"#
        );
        std::fs::write(pages.join("pick.maxref.xml"), page("pick", "Pick from a list", "(inactive)", &more)).unwrap();
        std::fs::write(pages.join("pick.first.maxref.xml"), page("pick.first", "The first of a list", "A list", "")).unwrap();
        let reference = learn(c74.path());
        let told = describe(&reference, "pick first");
        assert!(told.starts_with("[pick first] is [pick.first]: The first of a list\n"), "{told}");
        assert!(told.contains("- 0: A list") && !told.contains("(inactive)"), "{told}");
        let bare = describe(&reference, "pick 3");
        assert!(bare.starts_with("[pick] Pick from a list"), "an argument that names no mode: {bare}");
        // An object whose first argument is a name: the word after it is that name, not a mode.
        let named = r#"<objarglist><objarg name="name" optional="1" type="symbol"><digest>Its name</digest></objarg></objarglist>"#;
        std::fs::write(pages.join("store.maxref.xml"), page("store", "Keep values by name", "Messages", named)).unwrap();
        std::fs::write(pages.join("store.view.maxref.xml"), page("store.view", "Show what's kept", "A store", "")).unwrap();
        let reference = learn(c74.path());
        assert!(describe(&reference, "store view").starts_with("[store] Keep values by name\n"), "[dict view] is a dict named view");
        assert!(bare.contains("Also: m17, m18, m19, m20, m21, m22, m23, m24, m25, m26, m27, m28, m29, m30."), "{bare}");
        assert!(
            bare.contains("values 1 = s1, 2 = s2,") && bare.contains("20 = s20 and 4 more"),
            "an empty value counted, not said: {bare}"
        );
    }

    #[test]
    fn a_name_this_max_lacks_gets_the_nearest_it_has() {
        let c74 = max_folder();
        let mut reference = learn(c74.path());
        let page = "docs/refpages/max-ref/pipe.maxref.xml".to_string();
        let (inlets, outlets) = (Count::Fixed { count: 1 }, Count::Arguments { plus: 0, bare: 2 });
        let trigger = Object { inlets, outlets, page, seen: 4, held: vec![0, 1, 2, 3], ..Object::default() };
        reference.objects.insert("trigger".into(), trigger);
        reference.objects.insert("gen~".into(), Object { module: "msp".into(), digest: "Code for audio".into(), ..Object::default() });
        reference.objects.insert("pipes".into(), Object { module: "abstraction".into(), ..Object::default() });
        assert_eq!(describe(&reference, "pip 100"), "No [pip] in this Max. Nearest: [pipe], [pipes].");
        let started = std::time::Instant::now();
        assert_eq!(nearest(&reference, &"q".repeat(100_000)), Vec::<&str>::new());
        assert!(started.elapsed().as_secs() < 2, "a long unknown name is quick: {:?}", started.elapsed());
        assert_eq!(describe(&reference, "makenote"), "No [makenote] in this Max.");
        let alias = describe(&reference, "t b i");
        assert!(alias.starts_with("[t] is [pipe]: Hold numbers back for a time\n[t b i] has 1 inlet and 2 outlets."), "{alias}");
        assert_eq!(
            describe(&reference, "gen~ @title x"),
            "[gen~] Code for audio\n[gen~ @title x]: its inlets and outlets come from what's inside it.\nMax has no reference page for it: its help patchers show it."
        );
        assert!(describe(&reference, "pipes").ends_with("An abstraction Max ships: it has no reference page."));
        assert_eq!(
            describe_all(&reference, &["pip".into(), "makenote".into()]),
            "No [pip] in this Max. Nearest: [pipe], [pipes].\n\nNo [makenote] in this Max."
        );
    }
}
