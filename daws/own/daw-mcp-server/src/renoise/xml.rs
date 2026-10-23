//! Renoise's XML documents (a device's preset data, an instrument's `Instrument.xml`) read and written back so that what
//! Kumi doesn't edit comes out byte for byte as it went in: every tag, text and blank is kept as it was written, and only
//! the nodes an edit touches are written anew. Nothing is validated: Renoise checks what it loads.
//!
//! The documents are plain: elements, attributes, text (with the five named entities and numeric references), and now
//! and then a comment, a CDATA section or the XML declaration, each kept as written.

/// The deepest a document may nest. Renoise's go a few dozen levels deep (a Doofer in a Splitter in a Doofer adds a
/// handful each); a document past this is refused rather than walked.
pub const MAX_DEPTH: usize = 256;

/// Renoise indents each level by two spaces.
const UNIT: &str = "  ";

/// Why a document couldn't be read: where (a byte offset) and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    pub at: usize,
    pub message: String,
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.at)
    }
}

impl std::error::Error for XmlError {}

fn error(at: usize, message: impl Into<String>) -> XmlError {
    XmlError { at, message: message.into() }
}

/// A node inside an element, or around the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    /// Text between tags, as written (its entities not resolved).
    Text(String),
    /// A comment, a CDATA section, a processing instruction or a doctype, as written.
    Other(String),
}

impl Node {
    fn write(&self, out: &mut String) {
        match self {
            Node::Element(element) => element.write(out),
            Node::Text(text) | Node::Other(text) => out.push_str(text),
        }
    }

    fn is_blank(&self) -> bool {
        matches!(self, Node::Text(text) if text.bytes().all(|b| b.is_ascii_whitespace()))
    }
}

/// An element: its name, attributes and children, and how it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    name: String,
    /// Names and values (entities resolved), in order.
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
    /// The start tag as written, kept while the name and attributes are as read.
    open: Option<String>,
    /// The end tag as written; none for an element written `<Name/>` or made anew.
    close: Option<String>,
    /// Written `<Name/>`.
    empty: bool,
    /// The blanks before it on its line (where a new sibling or child goes, Renoise's way).
    indent: Option<String>,
}

/// A whole document: what is before the root element (the XML declaration and blanks), the root, and what is after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub before: Vec<Node>,
    pub root: Element,
    pub after: Vec<Node>,
}

impl Document {
    /// The document as text: byte for byte what was read, but for the nodes edited since.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        for node in &self.before {
            node.write(&mut out);
        }
        self.root.write(&mut out);
        for node in &self.after {
            node.write(&mut out);
        }
        out
    }
}

/// Reads a document.
pub fn parse(text: &str) -> Result<Document, XmlError> {
    let bytes = text.as_bytes();
    let mut before: Vec<Node> = vec![];
    let mut after: Vec<Node> = vec![];
    let mut root: Option<Element> = None;
    let mut stack: Vec<Element> = vec![];
    let mut i = 0;
    // (where a node goes: the open element, or around the root)
    fn push(stack: &mut [Element], root: &Option<Element>, before: &mut Vec<Node>, after: &mut Vec<Node>, node: Node) {
        match stack.last_mut() {
            Some(open) => open.children.push(node),
            None if root.is_none() => before.push(node),
            None => after.push(node),
        }
    }
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let end = text[i..].find('<').map_or(bytes.len(), |n| i + n);
            let node = Node::Text(text[i..end].to_string());
            if stack.is_empty() && !text[i..end].chars().all(|c| c.is_ascii_whitespace() || c == '\u{feff}') {
                return Err(error(i, "text outside the root element"));
            }
            push(&mut stack, &root, &mut before, &mut after, node);
            i = end;
            continue;
        }
        let rest = &text[i..];
        let other_end = if rest.starts_with("<?") {
            Some(find(text, i, "?>")? + 2)
        } else if rest.starts_with("<!--") {
            Some(find(text, i + 4, "-->")? + 3)
        } else if rest.starts_with("<![CDATA[") {
            if stack.is_empty() {
                return Err(error(i, "a CDATA section outside the root element"));
            }
            Some(find(text, i, "]]>")? + 3)
        } else if rest.starts_with("<!") {
            Some(find(text, i, ">")? + 1)
        } else {
            None
        };
        if let Some(end) = other_end {
            push(&mut stack, &root, &mut before, &mut after, Node::Other(text[i..end].to_string()));
            i = end;
            continue;
        }
        if rest.starts_with("</") {
            let (name, end) = end_tag(text, i)?;
            let Some(mut element) = stack.pop() else { return Err(error(i, format!("</{name}> closes nothing"))) };
            if element.name != name {
                return Err(error(i, format!("</{name}> closes <{}>", element.name)));
            }
            element.close = Some(text[i..end].to_string());
            match stack.last_mut() {
                Some(parent) => parent.children.push(Node::Element(element)),
                None => root = Some(element),
            }
            i = end;
            continue;
        }
        let (mut element, end) = start_tag(text, i)?;
        if stack.is_empty() && root.is_some() {
            return Err(error(i, format!("a second root element <{}>", element.name)));
        }
        if stack.len() >= MAX_DEPTH {
            return Err(error(i, format!("nested deeper than {MAX_DEPTH} elements")));
        }
        let siblings = match stack.last() {
            Some(parent) => &parent.children,
            None => &before,
        };
        element.indent = indent_after(siblings.last());
        if element.empty {
            match stack.last_mut() {
                Some(parent) => parent.children.push(Node::Element(element)),
                None => root = Some(element),
            }
        } else {
            stack.push(element);
        }
        i = end;
    }
    if let Some(open) = stack.last() {
        return Err(error(bytes.len(), format!("the document ends inside <{}>", open.name)));
    }
    let root = root.ok_or_else(|| error(0, "no root element"))?;
    Ok(Document { before, root, after })
}

fn find(text: &str, from: usize, what: &str) -> Result<usize, XmlError> {
    text[from..].find(what).map(|n| from + n).ok_or_else(|| error(from, format!("no {what} to end what starts here")))
}

/// The blanks after the last line break of the node before an element: its indentation.
fn indent_after(node: Option<&Node>) -> Option<String> {
    let Some(Node::Text(text)) = node else { return None };
    let tail = &text[text.rfind('\n')? + 1..];
    tail.bytes().all(|b| b == b' ' || b == b'\t').then(|| tail.to_string())
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b':' || b >= 0x80
}

fn is_name(b: u8) -> bool {
    is_name_start(b) || b.is_ascii_digit() || b == b'-' || b == b'.'
}

fn name_at(text: &str, at: usize) -> Result<(String, usize), XmlError> {
    let bytes = text.as_bytes();
    if at >= bytes.len() || !is_name_start(bytes[at]) {
        return Err(error(at, "a name was expected"));
    }
    let mut end = at + 1;
    while end < bytes.len() && is_name(bytes[end]) {
        end += 1;
    }
    Ok((text[at..end].to_string(), end))
}

fn skip_blanks(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

fn start_tag(text: &str, at: usize) -> Result<(Element, usize), XmlError> {
    let bytes = text.as_bytes();
    let (name, mut i) = name_at(text, at + 1)?;
    let mut attributes = vec![];
    loop {
        let blank = i;
        i = skip_blanks(bytes, i);
        match bytes.get(i) {
            None => return Err(error(at, format!("<{name} isn't closed"))),
            Some(b'>') => {
                let end = i + 1;
                let element = Element::read(name, attributes, &text[at..end], false);
                return Ok((element, end));
            }
            Some(b'/') if bytes.get(i + 1) == Some(&b'>') => {
                let end = i + 2;
                let element = Element::read(name, attributes, &text[at..end], true);
                return Ok((element, end));
            }
            Some(_) if i == blank => return Err(error(i, format!("a blank was expected in <{name}>"))),
            Some(_) => {
                let (attribute, after) = name_at(text, i)?;
                i = skip_blanks(bytes, after);
                if bytes.get(i) != Some(&b'=') {
                    return Err(error(i, format!("{attribute} in <{name}> has no value")));
                }
                i = skip_blanks(bytes, i + 1);
                let quote = match bytes.get(i) {
                    Some(q @ (b'"' | b'\'')) => *q,
                    _ => return Err(error(i, format!("{attribute} in <{name}>: a quoted value was expected"))),
                };
                let close = text[i + 1..].find(quote as char).map(|n| i + 1 + n).ok_or_else(|| error(i, "the value isn't closed"))?;
                attributes.push((attribute, unescape(&text[i + 1..close])));
                i = close + 1;
            }
        }
    }
}

fn end_tag(text: &str, at: usize) -> Result<(String, usize), XmlError> {
    let bytes = text.as_bytes();
    let (name, i) = name_at(text, at + 2)?;
    let i = skip_blanks(bytes, i);
    if bytes.get(i) != Some(&b'>') {
        return Err(error(at, format!("</{name} isn't closed")));
    }
    Ok((name, i + 1))
}

/// Text with its entities resolved (`&lt;` `&gt;` `&amp;` `&quot;` `&apos;`, `&#n;`, `&#xh;`); anything else is kept as it is.
pub fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|end| *end <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let resolved = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()))
                .and_then(char::from_u32),
        };
        match resolved {
            Some(c) => {
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

/// Text as it goes between tags.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

fn escape_attribute(text: &str) -> String {
    escape_text(text).replace('"', "&quot;")
}

impl Element {
    /// A new element, written the way Renoise writes one.
    pub fn new(name: &str) -> Self {
        Element { name: name.to_string(), attributes: vec![], children: vec![], open: None, close: None, empty: false, indent: None }
    }

    fn read(name: String, attributes: Vec<(String, String)>, open: &str, empty: bool) -> Self {
        Element { name, attributes, children: vec![], open: Some(open.to_string()), close: None, empty, indent: None }
    }

    /// With an attribute set (a new element's `type`).
    pub fn with_attribute(mut self, name: &str, value: &str) -> Self {
        self.set_attribute(name, value);
        self
    }

    /// With text in it.
    pub fn with_text(mut self, text: &str) -> Self {
        self.set_text(text);
        self
    }

    /// With a child element added last.
    pub fn with_child(mut self, child: Element) -> Self {
        let count = self.element_count();
        self.insert_element(count, child);
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn set_attribute(&mut self, name: &str, value: &str) {
        if self.attribute(name) == Some(value) {
            return;
        }
        match self.attributes.iter_mut().find(|(n, _)| n == name) {
            Some(pair) => pair.1 = value.to_string(),
            None => self.attributes.push((name.to_string(), value.to_string())),
        }
        self.open = None;
    }

    /// The child elements, in order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
    }

    pub fn elements_mut(&mut self) -> impl Iterator<Item = &mut Element> {
        self.children.iter_mut().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
    }

    pub fn element_count(&self) -> usize {
        self.elements().count()
    }

    pub fn element(&self, at: usize) -> Option<&Element> {
        self.elements().nth(at)
    }

    pub fn element_mut(&mut self, at: usize) -> Option<&mut Element> {
        self.elements_mut().nth(at)
    }

    /// The first child element of that name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|element| element.name == name)
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut Element> {
        self.elements_mut().find(|element| element.name == name)
    }

    /// The text in it (entities resolved, CDATA sections' contents too), or none when it holds elements.
    pub fn text(&self) -> Option<String> {
        let mut out = String::new();
        for node in &self.children {
            match node {
                Node::Element(_) => return None,
                Node::Text(text) => out.push_str(&unescape(text)),
                Node::Other(other) => {
                    if let Some(data) = other.strip_prefix("<![CDATA[").and_then(|rest| rest.strip_suffix("]]>")) {
                        out.push_str(data);
                    }
                }
            }
        }
        Some(out)
    }

    /// The text of the first child of that name.
    pub fn child_text(&self, name: &str) -> Option<String> {
        self.child(name).and_then(Element::text)
    }

    /// Its contents made this text.
    pub fn set_text(&mut self, text: &str) {
        if self.text().as_deref() == Some(text) && !self.empty {
            return;
        }
        self.children = if text.is_empty() { vec![] } else { vec![Node::Text(escape_text(text))] };
        if self.empty {
            self.empty = false;
            self.open = None;
            self.close = None;
        }
    }

    /// The text of the child of that name, made (last) when it isn't there.
    pub fn set_child_text(&mut self, name: &str, text: &str) {
        match self.child_mut(name) {
            Some(child) => child.set_text(text),
            None => {
                let count = self.element_count();
                self.insert_element(count, Element::new(name).with_text(text));
            }
        }
    }

    /// Where its children's lines start: as its first child's do, else one level in from its own.
    fn child_indent(&self) -> Option<String> {
        self.elements().find_map(|element| element.indent.clone()).or_else(|| self.indent.as_ref().map(|own| format!("{own}{UNIT}")))
    }

    /// Puts it at `at` among the child elements (the end when `at` is their count), on a line of its own as Renoise
    /// writes them when this element's children are written so.
    pub fn insert_element(&mut self, at: usize, mut element: Element) {
        let positions: Vec<usize> =
            self.children.iter().enumerate().filter(|(_, node)| matches!(node, Node::Element(_))).map(|(i, _)| i).collect();
        let indent = self.child_indent();
        if let Some(indent) = &indent {
            element.place(indent);
        }
        if self.empty {
            self.empty = false;
            self.open = None;
            self.close = None;
        }
        match (positions.get(at).copied(), indent) {
            (Some(j), Some(indent)) => {
                self.children.insert(j, Node::Element(element));
                self.children.insert(j + 1, Node::Text(format!("\n{indent}")));
            }
            (Some(j), None) => self.children.insert(j, Node::Element(element)),
            (None, Some(indent)) => {
                let closing = self.children.last().is_some_and(|node| node.is_blank() && matches!(node, Node::Text(t) if t.contains('\n')));
                if closing {
                    let at = self.children.len() - 1;
                    self.children.insert(at, Node::Element(element));
                    self.children.insert(at, Node::Text(format!("\n{indent}")));
                } else {
                    let own = self.indent.clone().unwrap_or_default();
                    self.children.retain(|node| !node.is_blank());
                    self.children.push(Node::Text(format!("\n{indent}")));
                    self.children.push(Node::Element(element));
                    self.children.push(Node::Text(format!("\n{own}")));
                }
            }
            (None, None) => self.children.push(Node::Element(element)),
        }
    }

    /// Takes the child element at `at` out (with the blanks before it), or none when there is no such child.
    pub fn remove_element(&mut self, at: usize) -> Option<Element> {
        let j = self.children.iter().enumerate().filter(|(_, node)| matches!(node, Node::Element(_))).map(|(i, _)| i).nth(at)?;
        let Node::Element(element) = self.children.remove(j) else { unreachable!("an element's place") };
        if j > 0 && self.children[j - 1].is_blank() {
            self.children.remove(j - 1);
        }
        if self.children.iter().all(Node::is_blank) {
            self.children.clear();
        }
        Some(element)
    }

    /// An element going on a line that starts with `indent`: its own lines follow (a new one is laid out level by
    /// level; one read from elsewhere keeps its lines, shifted to the new depth).
    fn place(&mut self, indent: &str) {
        let before = self.indent.replace(indent.to_string());
        let has_lines = self.children.iter().any(|node| matches!(node, Node::Text(t) if t.contains('\n')));
        match before {
            Some(old) if has_lines => self.shift(&old, indent),
            _ if has_lines => {}
            _ => {
                if self.children.iter().any(|node| matches!(node, Node::Element(_))) {
                    let inner = format!("{indent}{UNIT}");
                    let children = std::mem::take(&mut self.children);
                    for node in children {
                        match node {
                            Node::Element(mut child) => {
                                child.indent = None;
                                child.place(&inner);
                                self.children.push(Node::Text(format!("\n{inner}")));
                                self.children.push(Node::Element(child));
                            }
                            other => self.children.push(other),
                        }
                    }
                    self.children.push(Node::Text(format!("\n{indent}")));
                }
            }
        }
    }

    /// Every line inside it that started with `from` starts with `to` instead.
    fn shift(&mut self, from: &str, to: &str) {
        for node in &mut self.children {
            match node {
                Node::Text(text) if text.contains('\n') && text.bytes().all(|b| b.is_ascii_whitespace()) => {
                    let lines: Vec<&str> = text.split('\n').collect();
                    let shifted: Vec<String> = lines
                        .iter()
                        .enumerate()
                        .map(|(n, line)| match (n, line.strip_prefix(from)) {
                            (0, _) => line.to_string(),
                            (_, Some(rest)) => format!("{to}{rest}"),
                            (_, None) => line.to_string(),
                        })
                        .collect();
                    *text = shifted.join("\n");
                }
                Node::Element(child) => {
                    if let Some(indent) = child.indent.as_mut() {
                        if let Some(rest) = indent.strip_prefix(from) {
                            *indent = format!("{to}{rest}");
                        }
                    }
                    child.shift(from, to);
                }
                _ => {}
            }
        }
    }

    fn write(&self, out: &mut String) {
        let empty = self.empty && self.children.is_empty();
        match &self.open {
            Some(open) if empty || !self.empty => out.push_str(open),
            _ => {
                out.push('<');
                out.push_str(&self.name);
                for (name, value) in &self.attributes {
                    out.push(' ');
                    out.push_str(name);
                    out.push_str("=\"");
                    out.push_str(&escape_attribute(value));
                    out.push('"');
                }
                out.push_str(if empty { "/>" } else { ">" });
            }
        }
        if empty {
            return;
        }
        for child in &self.children {
            child.write(out);
        }
        match &self.close {
            Some(close) => out.push_str(close),
            None => {
                out.push_str("</");
                out.push_str(&self.name);
                out.push('>');
            }
        }
    }

    /// The element as text, alone.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

/// A number as Renoise writes one in its documents: the shortest text that reads back as the same single-precision
/// value, with ".0" on a whole number ("1.0", "0.25", "0.14620088").
pub fn number_text(value: f64) -> String {
    let single = value as f32;
    if !single.is_finite() {
        return "0.0".into();
    }
    let text = format!("{single}");
    if text.contains('.') || text.contains('e') || text.contains("inf") || text.contains("NaN") {
        text
    } else {
        format!("{text}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESET: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FilterDevicePreset doc_version=\"14\">\n  <DeviceSlot type=\"DooferDevice\">\n    <IsMaximized>true</IsMaximized>\n    <Macro0>\n      <Value>50</Value>\n      <Name>Cut &amp; run</Name>\n    </Macro0>\n    <NumActiveMacros>4</NumActiveMacros>\n    <DeviceChain>\n      <SelectedPresetName>Init</SelectedPresetName>\n      <Devices>\n        <GainerDevice type=\"GainerDevice\">\n          <IsMaximized>true</IsMaximized>\n        </GainerDevice>\n        <Eq5Device type=\"Eq5Device\">\n          <IsMaximized>false</IsMaximized>\n        </Eq5Device>\n      </Devices>\n    </DeviceChain>\n  </DeviceSlot>\n</FilterDevicePreset>\n";

    fn changed_lines(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
        let a: Vec<&str> = before.lines().collect();
        let b: Vec<&str> = after.lines().collect();
        let gone = a.iter().filter(|line| !b.contains(line)).map(|line| line.to_string()).collect();
        let new = b.iter().filter(|line| !a.contains(line)).map(|line| line.to_string()).collect();
        (gone, new)
    }

    #[test]
    fn what_is_read_is_written_back_byte_for_byte() {
        let samples = [
            PRESET.to_string(),
            "<a/>".into(),
            "<?xml version='1.0'?>\r\n<a  x = 'one'   y=\"t&quot;wo\" >\r\n\t<b/>  <!-- note --><c><![CDATA[<raw> & ]]></c>\r\n</a >\r\n"
                .into(),
            "<a><b>caf\u{e9} \u{1f3b5} &#233;&#x1F3B5; &lt;x&gt;</b><c   /></a>".into(),
            "<!DOCTYPE a>\n<a>\n</a>\n\n".into(),
        ];
        for sample in samples {
            let document = parse(&sample).unwrap_or_else(|e| panic!("{e}: {sample}"));
            assert_eq!(document.to_xml(), sample);
        }
    }

    #[test]
    fn a_document_written_any_way_reads_back_the_same_whatever_its_shape() {
        // A small generator of documents: nesting, attributes, text with entities, blanks of every kind, empties.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        fn build(next: &mut dyn FnMut(u64) -> u64, depth: usize, out: &mut String) {
            let name = ["A", "Bee", "c_d", "E.f", "g-h"][next(5) as usize];
            out.push('<');
            out.push_str(name);
            for k in 0..next(3) {
                out.push_str(&format!(" k{k}=\"v&amp;{k}\""));
            }
            if next(4) == 0 {
                out.push_str("/>");
                return;
            }
            out.push('>');
            for _ in 0..next(4) {
                match next(5) {
                    0 => out.push_str("\n    "),
                    1 => out.push_str("t&lt;x &#65;\u{e9}"),
                    2 => out.push_str("<!--c-->"),
                    _ if depth < 6 => build(next, depth + 1, out),
                    _ => out.push(' '),
                }
            }
            out.push_str("</");
            out.push_str(name);
            out.push('>');
        }
        for _ in 0..400 {
            let mut text = String::new();
            if next(2) == 0 {
                text.push_str("<?xml version=\"1.0\"?>\n");
            }
            build(&mut next, 0, &mut text);
            let document = parse(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
            assert_eq!(document.to_xml(), text);
        }
    }

    #[test]
    fn a_broken_document_is_refused_with_where_and_why() {
        for (text, why) in [
            ("<a><b></a>", "</a> closes <b>"),
            ("<a>", "ends inside <a>"),
            ("<a></a><b/>", "a second root"),
            ("<a x='1></a>", "isn't closed"),
            ("<a x=1></a>", "quoted value"),
            ("<a x></a>", "has no value"),
            ("</a>", "closes nothing"),
            ("", "no root element"),
            ("hello <a/>", "text outside"),
            ("<a><!-- open</a>", "-->"),
            ("<1a/>", "a name was expected"),
            ("<a b='1'c='2'/>", "a blank was expected"),
        ] {
            let err = parse(text).expect_err(text);
            assert!(err.message.contains(why), "{text}: {err}");
        }
        let deep = format!("{}{}", "<a>".repeat(MAX_DEPTH + 1), "</a>".repeat(MAX_DEPTH + 1));
        assert!(parse(&deep).unwrap_err().message.contains("deeper"));
        let fine = format!("{}{}", "<a>".repeat(MAX_DEPTH), "</a>".repeat(MAX_DEPTH));
        assert!(parse(&fine).is_ok());
    }

    #[test]
    fn text_reads_with_its_entities_and_cdata_resolved_and_writes_escaped() {
        let document = parse("<a><b>x &amp; y &lt;3 &#x41;&#66; &bogus; &</b><c><![CDATA[1 < 2]]></c><d/><e><f/></e></a>").unwrap();
        assert_eq!(document.root.child_text("b").as_deref(), Some("x & y <3 AB &bogus; &"));
        assert_eq!(document.root.child_text("c").as_deref(), Some("1 < 2"));
        assert_eq!(document.root.child_text("d").as_deref(), Some(""));
        assert_eq!(document.root.child_text("e"), None, "an element holding elements has no text");
        let mut document = document;
        document.root.child_mut("d").unwrap().set_text("R&B <live>");
        assert!(document.to_xml().contains("<d>R&amp;B &lt;live&gt;</d>"));
        assert_eq!(document.root.child_text("d").as_deref(), Some("R&B <live>"));
    }

    #[test]
    fn an_edit_changes_only_the_lines_it_touches() {
        let mut document = parse(PRESET).unwrap();
        let slot = document.root.child_mut("DeviceSlot").unwrap();
        slot.child_mut("Macro0").unwrap().set_child_text("Name", "Bite");
        slot.set_child_text("NumActiveMacros", "2");
        let text = document.to_xml();
        let (gone, new) = changed_lines(PRESET, &text);
        assert_eq!(gone, ["      <Name>Cut &amp; run</Name>", "    <NumActiveMacros>4</NumActiveMacros>"]);
        assert_eq!(new, ["      <Name>Bite</Name>", "    <NumActiveMacros>2</NumActiveMacros>"]);
        // Setting what is there already writes nothing anew.
        let mut same = parse(PRESET).unwrap();
        same.root.child_mut("DeviceSlot").unwrap().set_child_text("NumActiveMacros", "4");
        assert_eq!(same.to_xml(), PRESET);
    }

    #[test]
    fn a_new_element_goes_on_its_own_line_at_its_place_as_renoise_writes_it() {
        let mut document = parse(PRESET).unwrap();
        let devices = document.root.child_mut("DeviceSlot").unwrap().child_mut("DeviceChain").unwrap().child_mut("Devices").unwrap();
        let lfo = Element::new("LfoDevice").with_attribute("type", "LfoDevice").with_child(Element::new("IsMaximized").with_text("true"));
        devices.insert_element(1, lfo);
        let text = document.to_xml();
        let expected = PRESET.replace(
            "        </GainerDevice>\n",
            "        </GainerDevice>\n        <LfoDevice type=\"LfoDevice\">\n          <IsMaximized>true</IsMaximized>\n        </LfoDevice>\n",
        );
        assert_eq!(text, expected);
        // Last, and first.
        let mut document = parse(PRESET).unwrap();
        let devices = document.root.child_mut("DeviceSlot").unwrap().child_mut("DeviceChain").unwrap().child_mut("Devices").unwrap();
        devices.insert_element(2, Element::new("NotePadDevice").with_attribute("type", "NotePadDevice"));
        devices.insert_element(0, Element::new("DcOffsetDevice").with_attribute("type", "DcOffsetDevice"));
        let text = document.to_xml();
        assert!(
            text.contains("      <Devices>\n        <DcOffsetDevice type=\"DcOffsetDevice\"></DcOffsetDevice>\n        <GainerDevice"),
            "{text}"
        );
        assert!(
            text.contains("        </Eq5Device>\n        <NotePadDevice type=\"NotePadDevice\"></NotePadDevice>\n      </Devices>"),
            "{text}"
        );
        assert!(parse(&text).is_ok());
    }

    #[test]
    fn a_child_goes_into_an_empty_element_one_level_in() {
        for empty in ["<Devices/>", "<Devices></Devices>", "<Devices>\n    </Devices>"] {
            let text = format!("<A>\n  <Chain>\n    {empty}\n  </Chain>\n</A>");
            let mut document = parse(&text).unwrap();
            let devices = document.root.child_mut("Chain").unwrap().child_mut("Devices").unwrap();
            devices.insert_element(0, Element::new("GainerDevice").with_attribute("type", "GainerDevice"));
            assert_eq!(
                document.to_xml(),
                "<A>\n  <Chain>\n    <Devices>\n      <GainerDevice type=\"GainerDevice\"></GainerDevice>\n    </Devices>\n  </Chain>\n</A>",
                "{empty}"
            );
        }
    }

    #[test]
    fn an_element_taken_out_leaves_no_blank_line_and_an_emptied_list_is_empty() {
        let mut document = parse(PRESET).unwrap();
        let devices = document.root.child_mut("DeviceSlot").unwrap().child_mut("DeviceChain").unwrap().child_mut("Devices").unwrap();
        let gainer = devices.remove_element(0).unwrap();
        assert_eq!(gainer.attribute("type"), Some("GainerDevice"));
        let text = document.to_xml();
        assert!(text.contains("      <Devices>\n        <Eq5Device type=\"Eq5Device\">"), "{text}");
        let devices = document.root.child_mut("DeviceSlot").unwrap().child_mut("DeviceChain").unwrap().child_mut("Devices").unwrap();
        devices.remove_element(0).unwrap();
        assert!(devices.remove_element(0).is_none());
        assert!(document.to_xml().contains("      <Devices></Devices>\n"), "{}", document.to_xml());
        // An element moved from one place to another keeps its own lines, at the new depth.
        let mut document = parse(PRESET).unwrap();
        let devices = document.root.child_mut("DeviceSlot").unwrap().child_mut("DeviceChain").unwrap().child_mut("Devices").unwrap();
        let gainer = devices.remove_element(0).unwrap();
        document.root.child_mut("DeviceSlot").unwrap().insert_element(0, gainer);
        let text = document.to_xml();
        assert!(text.contains("  <DeviceSlot type=\"DooferDevice\">\n    <GainerDevice type=\"GainerDevice\">\n      <IsMaximized>true</IsMaximized>\n    </GainerDevice>\n    <IsMaximized>"), "{text}");
    }

    #[test]
    fn an_attribute_set_writes_the_start_tag_anew_and_nothing_else() {
        let mut document = parse("<a>\n  <b  type = 'x' n=\"1\"/>\n</a>").unwrap();
        let b = document.root.child_mut("b").unwrap();
        b.set_attribute("type", "x");
        assert_eq!(document.to_xml(), "<a>\n  <b  type = 'x' n=\"1\"/>\n</a>", "the same value writes nothing");
        let b = document.root.child_mut("b").unwrap();
        b.set_attribute("type", "y\"z");
        assert_eq!(document.to_xml(), "<a>\n  <b type=\"y&quot;z\" n=\"1\"/>\n</a>");
        let b = document.root.child_mut("b").unwrap();
        b.set_text("t");
        assert_eq!(document.to_xml(), "<a>\n  <b type=\"y&quot;z\" n=\"1\">t</b>\n</a>");
    }

    /// Every preset Renoise ships, where Renoise is installed here (it isn't in CI): each reads and writes back byte for byte.
    #[test]
    fn renoises_own_presets_come_back_byte_for_byte_where_renoise_is_installed() {
        fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|ext| ext == "xrdp") {
                    found.push(path);
                }
            }
        }
        let library = std::path::Path::new("/Applications/Renoise.app/Contents/Resources/Library/Effect Presets");
        let mut found = vec![];
        walk(library, &mut found);
        for path in &found {
            let text = std::fs::read_to_string(path).unwrap();
            let document = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(document.to_xml(), text, "{}", path.display());
        }
    }

    #[test]
    fn numbers_are_written_as_renoise_writes_them() {
        assert_eq!(number_text(1.0), "1.0");
        assert_eq!(number_text(0.0), "0.0");
        assert_eq!(number_text(0.25), "0.25");
        assert_eq!(number_text(-1.0), "-1.0");
        assert_eq!(number_text(0.146_200_88), "0.14620088");
        assert_eq!(number_text(7.500_000_95), "7.500001");
        assert_eq!(number_text(f64::NAN), "0.0");
        assert_eq!(number_text(123_456.0), "123456.0");
    }
}
