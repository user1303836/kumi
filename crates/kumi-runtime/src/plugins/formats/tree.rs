//! A format-neutral tree: what each decoder's own tree (CBOR, JSON, XML, Ozone's typed JSON) becomes when a
//! survey counts its fields or a structure file describes them.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Text(String),
    /// Binary data, by its length.
    Bytes(usize),
    List(Vec<Node>),
    Map(Vec<(String, Node)>),
}

impl Node {
    /// The kind of a value as structure files name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Node::Null => "null",
            Node::Bool(_) => "bool",
            Node::Int(_) => "int",
            Node::Float(_) => "float",
            Node::Text(_) => "text",
            Node::Bytes(_) => "bytes",
            Node::List(_) => "list",
            Node::Map(_) => "map",
        }
    }

    pub fn is_container(&self) -> bool {
        matches!(self, Node::List(_) | Node::Map(_))
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Node::Int(n) => Some(*n as f64),
            Node::Float(n) => Some(*n),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, node)| node),
            _ => None,
        }
    }

    /// Every node with its path: map keys joined by "/", a list's items as "[]" (all items share a path, so a
    /// survey counts them together). `rule` turns a key into its path segment.
    pub fn walk(&self, rule: PathRule, visit: &mut dyn FnMut(&str, &Node)) {
        fn go(node: &Node, path: &mut String, rule: PathRule, visit: &mut dyn FnMut(&str, &Node)) {
            visit(path, node);
            let len = path.len();
            match node {
                Node::Map(entries) => {
                    for (key, child) in entries {
                        let segment = rule.segment(path, key, child.is_container());
                        if !path.is_empty() {
                            path.push('/');
                        }
                        path.push_str(&segment);
                        go(child, path, rule, visit);
                        path.truncate(len);
                    }
                }
                Node::List(items) => {
                    for child in items {
                        path.push_str("[]");
                        go(child, path, rule, visit);
                        path.truncate(len);
                    }
                }
                _ => {}
            }
        }
        go(self, &mut String::new(), rule, visit);
    }
}

impl From<&Value> for Node {
    fn from(value: &Value) -> Node {
        match value {
            Value::Null => Node::Null,
            Value::Bool(b) => Node::Bool(*b),
            Value::Number(n) => match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => Node::Int(i as i128),
                (_, Some(u)) => Node::Int(u as i128),
                _ => Node::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            Value::String(s) => Node::Text(s.clone()),
            Value::Array(items) => Node::List(items.iter().map(Node::from).collect()),
            Value::Object(map) => Node::Map(map.iter().map(|(k, v)| (k.clone(), Node::from(v))).collect()),
        }
    }
}

/// How a tree's keys become the paths fields are known by, so numbered copies of one section share a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathRule {
    /// Keys as written.
    Exact,
    /// A section key's trailing number becomes "{n}" ("Oscillator3" → "Oscillator{n}") while a leaf keeps its
    /// name ("kParamCurve1" is one of a section's knobs). Keys anywhere inside a map named "files" are folder
    /// and file names, and become "{file}".
    NumberedSections,
    /// Numbers between underscores are instances: "osc_2_level" → "osc_{n}_level", "lfo_3" → "lfo_{n}";
    /// other digits stay ("macro1" is a name, not an instance).
    UnderscoredNumbers,
}

impl PathRule {
    /// The path segment for `key`, inside the map at `parent`; `container` says whether its value holds more.
    pub fn segment(self, parent: &str, key: &str, container: bool) -> String {
        match self {
            PathRule::Exact => key.to_string(),
            PathRule::NumberedSections => {
                if parent.split('/').any(|segment| segment == "files") {
                    return "{file}".to_string();
                }
                let stem = key.trim_end_matches(|c: char| c.is_ascii_digit());
                if container && stem.len() < key.len() && !stem.is_empty() {
                    format!("{stem}{{n}}")
                } else {
                    key.to_string()
                }
            }
            PathRule::UnderscoredNumbers => key
                .split('_')
                .map(|part| if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) { "{n}" } else { part })
                .collect::<Vec<_>>()
                .join("_"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_share_numbered_sections_and_list_items() {
        let tree = Node::from(
            &serde_json::json!({"Osc0": {"kParam1": 1, "kParam2": 2.5}, "Osc1": {"kParam1": 3}, "fx": [{"a": "x"}, {"a": "y"}]}),
        );
        let mut seen = Vec::new();
        tree.walk(PathRule::NumberedSections, &mut |path, node| seen.push(format!("{path}:{}", node.kind())));
        assert_eq!(
            seen,
            [
                ":map",
                "Osc{n}:map",
                "Osc{n}/kParam1:int",
                "Osc{n}/kParam2:float",
                "Osc{n}:map",
                "Osc{n}/kParam1:int",
                "fx:list",
                "fx[]:map",
                "fx[]/a:text",
                "fx[]:map",
                "fx[]/a:text"
            ]
        );
        assert_eq!(PathRule::UnderscoredNumbers.segment("settings", "modulation_12_amount", false), "modulation_{n}_amount");
        assert_eq!(PathRule::UnderscoredNumbers.segment("", "macro1", false), "macro1");
        assert_eq!(PathRule::NumberedSections.segment("", "0", true), "0");
        assert_eq!(PathRule::NumberedSections.segment("Osc{n}/MultiSampleOsc{n}/files", "Oud Samples/Kt 01 A#1.flac", false), "{file}");
        assert_eq!(PathRule::Exact.segment("", "Osc1", true), "Osc1");
    }
}
