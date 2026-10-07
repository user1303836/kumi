//! Plain XML as plug-ins write their presets (Ozone's): a declaration, elements with attributes, the five
//! named entities and numeric ones, comments. No DOCTYPE or entity definitions, which a preset never needs
//! and which are refused rather than expanded. Mixed text keeps only its trimmed characters.

use super::tree::Node;
use super::FormatError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    /// In the order written.
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// The element's own text, trimmed ("" when it holds only elements and whitespace).
    pub text: String,
}

impl Element {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The tree surveys read: attributes as "@name", children by their tag (repeats become a list).
    pub fn to_node(&self) -> Node {
        let mut entries: Vec<(String, Node)> = self.attributes.iter().map(|(k, v)| (format!("@{k}"), Node::Text(v.clone()))).collect();
        // Where each child tag's entry is, so a repeat finds it without a search: linear in the children.
        let mut by_tag: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for child in &self.children {
            let node = child.to_node();
            match by_tag.get(child.name.as_str()) {
                Some(&at) => match &mut entries[at].1 {
                    Node::List(items) => items.push(node),
                    existing => {
                        let first = std::mem::replace(existing, Node::Null);
                        *existing = Node::List(vec![first, node]);
                    }
                },
                None => {
                    by_tag.insert(&child.name, entries.len());
                    entries.push((child.name.clone(), node));
                }
            }
        }
        if !self.text.is_empty() {
            entries.push(("#text".to_string(), Node::Text(self.text.clone())));
        }
        Node::Map(entries)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// The `<?xml …?>` declaration's inside, as written.
    pub declaration: Option<String>,
    pub root: Element,
}

const MAX_DEPTH: usize = 256;

pub fn parse(text: &str) -> Result<Document, FormatError> {
    let mut parser = Parser { text: text.strip_prefix('\u{feff}').unwrap_or(text), at: 0 };
    parser.space_and_comments()?;
    let declaration = if parser.rest().starts_with("<?xml") {
        let end = parser.rest().find("?>").ok_or_else(|| parser.error("an unclosed declaration"))?;
        let inside = parser.rest()[5..end].trim().to_string();
        parser.at += end + 2;
        Some(inside)
    } else {
        None
    };
    parser.space_and_comments()?;
    if parser.rest().starts_with("<!") {
        return Err(parser.error("a DOCTYPE"));
    }
    let root = parser.element(0)?;
    parser.space_and_comments()?;
    if !parser.rest().is_empty() {
        return Err(parser.error("content after the root element"));
    }
    Ok(Document { declaration, root })
}

/// The document written out, indented by four spaces as Ozone writes its presets.
pub fn write(document: &Document) -> String {
    let mut out = String::new();
    if let Some(declaration) = &document.declaration {
        out.push_str(&format!("<?xml {declaration} ?>\n"));
    }
    write_element(&mut out, &document.root, 0);
    out
}

fn write_element(out: &mut String, element: &Element, depth: usize) {
    let indent = "    ".repeat(depth);
    out.push_str(&indent);
    out.push('<');
    out.push_str(&element.name);
    for (key, value) in &element.attributes {
        out.push_str(&format!(" {key}=\"{}\"", escape(value)));
    }
    if element.children.is_empty() && element.text.is_empty() {
        out.push_str(" />\n");
        return;
    }
    out.push('>');
    out.push_str(&escape(&element.text));
    if !element.children.is_empty() {
        out.push('\n');
        for child in &element.children {
            write_element(out, child, depth + 1);
        }
        out.push_str(&indent);
    }
    out.push_str(&format!("</{}>\n", element.name));
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn error(&self, what: &str) -> FormatError {
        let line = self.text[..self.at].matches('\n').count() + 1;
        FormatError::new(format!("XML line {line}: {what}"))
    }

    fn space_and_comments(&mut self) -> Result<(), FormatError> {
        loop {
            let skipped = self.rest().len() - self.rest().trim_start().len();
            self.at += skipped;
            if !self.rest().starts_with("<!--") {
                return Ok(());
            }
            let end = self.rest().find("-->").ok_or_else(|| self.error("an unclosed comment"))?;
            self.at += end + 3;
        }
    }

    fn name(&mut self) -> Result<String, FormatError> {
        let len = self.rest().find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))).unwrap_or(self.rest().len());
        if len == 0 {
            return Err(self.error("a missing name"));
        }
        let name = self.rest()[..len].to_string();
        self.at += len;
        Ok(name)
    }

    fn element(&mut self, depth: usize) -> Result<Element, FormatError> {
        if depth > MAX_DEPTH {
            return Err(self.error("elements nested too deep"));
        }
        if !self.rest().starts_with('<') {
            return Err(self.error("text where an element should start"));
        }
        self.at += 1;
        let name = self.name()?;
        let mut attributes = Vec::new();
        loop {
            self.space_and_comments_in_tag();
            if self.rest().starts_with("/>") {
                self.at += 2;
                return Ok(Element { name, attributes, children: Vec::new(), text: String::new() });
            }
            if self.rest().starts_with('>') {
                self.at += 1;
                break;
            }
            let key = self.name()?;
            self.space_and_comments_in_tag();
            if !self.rest().starts_with('=') {
                return Err(self.error("an attribute without a value"));
            }
            self.at += 1;
            self.space_and_comments_in_tag();
            let quote =
                self.rest().chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| self.error("an unquoted attribute"))?;
            self.at += 1;
            let end = self.rest().find(quote).ok_or_else(|| self.error("an unclosed attribute"))?;
            let raw = &self.rest()[..end];
            if raw.contains('<') {
                return Err(self.error("a '<' inside an attribute"));
            }
            let value = unescape(raw).map_err(|what| self.error(&what))?;
            self.at += end + 1;
            if attributes.iter().any(|(k, _)| *k == key) {
                return Err(self.error("a repeated attribute"));
            }
            attributes.push((key, value));
        }
        let mut children = Vec::new();
        let mut text = String::new();
        loop {
            let end = self.rest().find('<').ok_or_else(|| self.error("an unclosed element"))?;
            text.push_str(&unescape(&self.rest()[..end]).map_err(|what| self.error(&what))?);
            self.at += end;
            if self.rest().starts_with("<!--") {
                let close = self.rest().find("-->").ok_or_else(|| self.error("an unclosed comment"))?;
                self.at += close + 3;
            } else if self.rest().starts_with("</") {
                self.at += 2;
                let closing = self.name()?;
                if closing != name {
                    return Err(self.error(&format!("</{closing}> closing <{name}>")));
                }
                self.space_and_comments_in_tag();
                if !self.rest().starts_with('>') {
                    return Err(self.error("an unclosed end tag"));
                }
                self.at += 1;
                return Ok(Element { name, attributes, children, text: text.trim().to_string() });
            } else if self.rest().starts_with("<!") || self.rest().starts_with("<?") {
                return Err(self.error("markup a preset doesn't use"));
            } else {
                children.push(self.element(depth + 1)?);
            }
        }
    }

    fn space_and_comments_in_tag(&mut self) {
        self.at += self.rest().len() - self.rest().trim_start().len();
    }
}

fn unescape(text: &str) -> Result<String, String> {
    if !text.contains('&') {
        return Ok(text.to_string());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let end = rest[i..].find(';').ok_or("an unterminated entity")?;
        let entity = &rest[i + 1..i + end];
        let c = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32).ok_or_else(|| format!("an unknown entity &{entity};"))?
            }
        };
        out.push(c);
        rest = &rest[i + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESET: &str = "\u{feff}<?xml version=\"1.0\" standalone=\"yes\" ?>\n<Ozone9Maximizer PresetVer=\"1\" Comments=\"Loud &amp; clear\">\n    <!-- a comment -->\n    <Global Enabled=\"0\">\n        <ExtraBytes ElementID=\"ElementChain\" Data=\"AAkAAABNYXhpbWl6ZXI=\" />\n    </Global>\n    <Maximizer Enabled=\"1\">\n        <Param ElementID=\"Maximizer\" ParamID=\"Threshold\" Value=\"-2.22614670\" />\n        <Param ElementID='Maximizer' ParamID=\"Character\" Value=\"1.17647064\"/>\n    </Maximizer>\n    <Meters Enabled=\"0\" />\n</Ozone9Maximizer>\n";

    #[test]
    fn reads_a_preset_and_writes_the_same_tree() {
        let document = parse(PRESET).unwrap();
        assert_eq!(document.declaration.as_deref(), Some("version=\"1.0\" standalone=\"yes\""));
        assert_eq!(document.root.name, "Ozone9Maximizer");
        assert_eq!(document.root.attribute("Comments"), Some("Loud & clear"));
        let maximizer = document.root.child("Maximizer").unwrap();
        assert_eq!(maximizer.children.len(), 2);
        assert_eq!(maximizer.children[1].attribute("ElementID"), Some("Maximizer"));
        assert_eq!(parse(&write(&document)).unwrap(), document);
        let node = document.root.to_node();
        assert!(matches!(node.get("Maximizer").and_then(|m| m.get("Param")), Some(Node::List(items)) if items.len() == 2));
    }

    #[test]
    fn refuses_what_a_preset_never_holds() {
        for bad in [
            "<!DOCTYPE x [<!ENTITY a \"b\">]><x/>",
            "<x a=\"&ext;\"/>",
            "<x><y></x>",
            "<x a=1/>",
            "<x a=\"1\" a=\"2\"/>",
            "<x/><y/>",
            "<x>",
            "<x a=\"<\"/>",
            "text",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(parse("<x>&#65;&#x42;</x>").unwrap().root.text, "AB");
        assert!(parse(&"<a>".repeat(MAX_DEPTH + 2)).is_err());
    }
}
