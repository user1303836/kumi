//! Logic's Accessibility tree, read through the helper: a walker that asks for subtrees and attributes by root and
//! path, and [`Node`], the element as the helper reports it. Where Logic's elements are and what their labels say in
//! the producer's language is [`paths`] and [`strings`]; this module only reads what it is pointed at. The contract's
//! rules live here: a pop-up button's children are never read, and a window is addressed as `main` (`AXMainWindow`),
//! never through `AXWindows`, which is empty while Logic's Space isn't the current one.

pub mod paths;
pub mod strings;

use serde_json::{json, Map, Value};

use crate::helper::{Helper, HelperError};

/// A `tree` deeper than this costs more than the helper's 2 s allow on a full project window; readers ask for
/// subtrees instead.
pub const MAX_DEPTH: usize = 8;
/// Children per node a reader asks for unless it says otherwise.
pub const CHILDREN: usize = 100;
/// The most children of one element a reader ever asks for: a project of more tracks than this is paged by scrolling
/// later; the helper cuts arrays at 256 anyway.
pub const MAX_CHILDREN: usize = 256;

/// What the helper answered, or why it couldn't.
#[derive(Debug, Clone, PartialEq)]
pub enum AxError {
    /// The helper refused, timed out or is gone.
    Helper(HelperError),
    /// The element a reader looked for isn't where Logic was expected to keep it.
    Missing(String),
}

impl std::fmt::Display for AxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AxError::Helper(error) => write!(f, "{error}"),
            AxError::Missing(what) => write!(f, "Logic Pro's window doesn't show {what}"),
        }
    }
}

impl From<HelperError> for AxError {
    fn from(error: HelperError) -> Self {
        AxError::Helper(error)
    }
}

/// Whether an error is the helper's `untrusted`: the Accessibility grant is missing.
pub fn untrusted(error: &AxError) -> bool {
    matches!(error, AxError::Helper(HelperError::Refused { code, .. }) if code == "untrusted")
}

/// Where a path starts: the app, its main window, the focused window, the menu bar, or a window by a part of its title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Root {
    App,
    Main,
    Focused,
    MenuBar,
    Title(String),
}

impl Root {
    pub fn to_json(&self) -> Value {
        match self {
            Root::App => json!("app"),
            Root::Main => json!("main"),
            Root::Focused => json!("focused"),
            Root::MenuBar => json!("menubar"),
            Root::Title(part) => json!({"title": part}),
        }
    }
}

/// One element as the helper reports it: the fields that aren't empty, its children as far as the walk went, and how
/// many more children it has beyond those.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    /// The path from the root of the walk, child indexes joined by dots (empty for the root itself).
    pub path: String,
    pub role: String,
    pub subrole: String,
    pub title: String,
    pub desc: String,
    pub value: Value,
    pub value_desc: String,
    pub help: String,
    pub id: String,
    pub enabled: Option<bool>,
    pub selected: Option<bool>,
    pub settable: Vec<String>,
    pub actions: Vec<String>,
    pub children: Vec<Node>,
    /// Children beyond those listed (or all of them, at the depth limit).
    pub more: usize,
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn names(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

impl Node {
    /// The helper's `node` as a tree.
    pub fn from_json(value: &Value) -> Node {
        Node {
            path: text(value, "i"),
            role: text(value, "role"),
            subrole: text(value, "subrole"),
            title: text(value, "title"),
            desc: text(value, "desc"),
            value: value.get("value").cloned().unwrap_or(Value::Null),
            value_desc: text(value, "valueDesc"),
            help: text(value, "help"),
            id: text(value, "id"),
            enabled: value.get("enabled").and_then(Value::as_bool),
            selected: value.get("selected").and_then(Value::as_bool),
            settable: names(value, "set"),
            actions: names(value, "act"),
            children: value.get("c").and_then(Value::as_array).map(|c| c.iter().map(Node::from_json).collect()).unwrap_or_default(),
            more: value.get("more").and_then(Value::as_u64).unwrap_or(0) as usize,
        }
    }

    /// The node as the helper writes it, for fixtures and tests.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        if !self.path.is_empty() {
            out.insert("i".into(), self.path.clone().into());
        }
        for (key, value) in [
            ("role", &self.role),
            ("subrole", &self.subrole),
            ("title", &self.title),
            ("desc", &self.desc),
            ("valueDesc", &self.value_desc),
            ("help", &self.help),
            ("id", &self.id),
        ] {
            if !value.is_empty() {
                out.insert(key.into(), value.clone().into());
            }
        }
        if !self.value.is_null() {
            out.insert("value".into(), self.value.clone());
        }
        if let Some(enabled) = self.enabled {
            out.insert("enabled".into(), enabled.into());
        }
        if let Some(selected) = self.selected {
            out.insert("selected".into(), selected.into());
        }
        if !self.settable.is_empty() {
            out.insert("set".into(), self.settable.clone().into());
        }
        if !self.actions.is_empty() {
            out.insert("act".into(), self.actions.clone().into());
        }
        if !self.children.is_empty() {
            out.insert("c".into(), Value::Array(self.children.iter().map(Node::to_json).collect()));
        }
        if self.more > 0 {
            out.insert("more".into(), self.more.into());
        }
        Value::Object(out)
    }

    /// The value as a number, if it is one.
    pub fn number(&self) -> Option<f64> {
        self.value.as_f64().filter(|n| n.is_finite())
    }

    /// A checkbox's or switch's state: a number other than 0, `true`, or the text `on`.
    pub fn is_on(&self) -> bool {
        match &self.value {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
            Value::String(s) => matches!(s.as_str(), "on" | "1" | "true"),
            _ => false,
        }
    }

    /// The value as text: a string as it is, a number written out.
    pub fn value_text(&self) -> String {
        match &self.value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }

    pub fn is_selected(&self) -> bool {
        self.selected == Some(true)
    }

    /// The child at an index.
    pub fn child(&self, index: usize) -> Option<&Node> {
        self.children.get(index)
    }

    /// The first child for which `wanted` holds.
    pub fn child_where(&self, wanted: impl Fn(&Node) -> bool) -> Option<&Node> {
        self.children.iter().find(|child| wanted(child))
    }

    /// Every child for which `wanted` holds.
    pub fn children_where<'a>(&'a self, wanted: impl Fn(&Node) -> bool + 'a) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |child| wanted(child))
    }

    /// The first node, depth first, for which `wanted` holds: this one included.
    pub fn find(&self, wanted: &dyn Fn(&Node) -> bool) -> Option<&Node> {
        if wanted(self) {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(wanted))
    }

    /// Every node, depth first, for which `wanted` holds; a matching node's children aren't searched further.
    pub fn find_all<'a>(&'a self, wanted: &dyn Fn(&Node) -> bool, out: &mut Vec<&'a Node>) {
        if wanted(self) {
            out.push(self);
            return;
        }
        for child in &self.children {
            child.find_all(wanted, out);
        }
    }

    /// The node at a path below this one (child indexes joined by dots, as the helper writes them), when the walk went
    /// that far.
    pub fn at(&self, path: &str) -> Option<&Node> {
        if path.is_empty() {
            return Some(self);
        }
        let mut node = self;
        for segment in path.split('.') {
            node = node.children.get(segment.parse::<usize>().ok()?)?;
        }
        Some(node)
    }

    /// How many nodes the walk holds.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }
}

/// A path below a root, extended by a child index.
pub fn join(path: &str, index: usize) -> String {
    if path.is_empty() {
        index.to_string()
    } else {
        format!("{path}.{index}")
    }
}

/// The attributes a reader asks for when it wants the usual fields but not the costly `set` and `act`.
pub const CHEAP: &[&str] = &["role", "subrole", "title", "desc", "value", "valueDesc", "help", "id", "enabled", "selected"];
/// The fields that cost nothing much and say what an element is: no help text, which Logic writes at length.
pub const BRIEF: &[&str] = &["role", "subrole", "title", "desc", "value", "valueDesc", "id", "enabled", "selected"];

/// Reads Logic's elements through the helper, for one Logic (by pid).
pub struct Ax<'a> {
    pub helper: &'a mut dyn Helper,
    pub pid: u32,
    /// How many helper calls this walker made, and how long they took together: the lab's numbers.
    pub calls: u32,
    pub helper_ms: u64,
}

impl<'a> Ax<'a> {
    pub fn new(helper: &'a mut dyn Helper, pid: u32) -> Ax<'a> {
        Ax { helper, pid, calls: 0, helper_ms: 0 }
    }

    fn call(&mut self, op: &str, params: Value) -> Result<Value, AxError> {
        let reply = self.helper.call(op, params)?;
        self.calls += 1;
        self.helper_ms += reply.get("ms").and_then(Value::as_u64).unwrap_or(0);
        Ok(reply)
    }

    /// The element at `path` below `root` and its descendants, `depth` levels down and `children` children per node,
    /// with the fields in `attrs` (every cheap one when None).
    pub fn tree(&mut self, root: &Root, path: &str, depth: usize, children: usize, attrs: Option<&[&str]>) -> Result<Node, AxError> {
        let mut params = json!({"pid": self.pid, "root": root.to_json(), "path": path, "depth": depth.min(MAX_DEPTH), "children": children.min(MAX_CHILDREN)});
        if let Some(attrs) = attrs {
            params["attrs"] = attrs.iter().map(|a| Value::String(a.to_string())).collect();
        }
        let reply = self.call("tree", params)?;
        let node = reply.get("node").ok_or_else(|| AxError::Missing(format!("the element at {path:?}")))?;
        Ok(Node::from_json(node))
    }

    /// A subtree through a pop-up button (its menu, once the button is pressed): the helper's `popups` rule lifted
    /// for this one read, which opens nothing the press hasn't.
    pub fn tree_in_popup(
        &mut self,
        root: &Root,
        path: &str,
        depth: usize,
        children: usize,
        attrs: Option<&[&str]>,
    ) -> Result<Node, AxError> {
        let mut params = json!({"pid": self.pid, "root": root.to_json(), "path": path, "depth": depth.min(MAX_DEPTH), "children": children.min(MAX_CHILDREN), "popups": true});
        if let Some(attrs) = attrs {
            params["attrs"] = attrs.iter().map(|a| Value::String(a.to_string())).collect();
        }
        let reply = self.call("tree", params)?;
        let node = reply.get("node").ok_or_else(|| AxError::Missing(format!("the element at {path:?}")))?;
        Ok(Node::from_json(node))
    }

    /// An action on an element through a pop-up button (a menu item of its open menu).
    pub fn action_in_popup(&mut self, root: &Root, path: &str, name: &str, settle_ms: u64) -> Result<Value, AxError> {
        self.call(
            "action",
            json!({"pid": self.pid, "root": root.to_json(), "path": path, "name": name, "settleMs": settle_ms, "popups": true}),
        )
    }

    /// The attributes named of the element at `path` below `root`, by their AX names.
    pub fn get(&mut self, root: &Root, path: &str, attrs: &[&str]) -> Result<Map<String, Value>, AxError> {
        let params = json!({"pid": self.pid, "root": root.to_json(), "path": path, "attrs": attrs});
        let reply = self.call("get", params)?;
        Ok(reply.get("attrs").and_then(Value::as_object).cloned().unwrap_or_default())
    }

    /// Logic's windows: the helper's `windows` reply as it is.
    pub fn windows(&mut self) -> Result<Value, AxError> {
        self.call("windows", json!({"pid": self.pid}))
    }

    /// Presses an element's action (`AXPress`, `AXConfirm`, …), waiting `settle_ms` before the helper reads back.
    pub fn action(&mut self, root: &Root, path: &str, name: &str, settle_ms: u64) -> Result<Value, AxError> {
        self.call("action", json!({"pid": self.pid, "root": root.to_json(), "path": path, "name": name, "settleMs": settle_ms}))
    }

    /// Sets an attribute, waiting `settle_ms` before the helper reads back.
    pub fn set(&mut self, root: &Root, path: &str, attr: &str, value: Value, settle_ms: u64) -> Result<Value, AxError> {
        self.call(
            "set",
            json!({"pid": self.pid, "root": root.to_json(), "path": path, "attr": attr, "value": value, "settleMs": settle_ms}),
        )
    }

    /// Whether Logic is the app in front, from the helper's `apps`.
    pub fn in_front(&mut self) -> Result<bool, AxError> {
        let reply = self.call("apps", json!({"bundles": crate::logic::BUNDLES}))?;
        Ok(reply["apps"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|app| app["pid"].as_u64() == Some(u64::from(self.pid)) && app["active"] == Value::Bool(true)))
    }

    /// Asks macOS to bring Logic in front (the helper waits up to a second): whether it is.
    pub fn activate(&mut self) -> Result<bool, AxError> {
        let reply = self.call("activate", json!({"pid": self.pid}))?;
        Ok(reply["active"] == Value::Bool(true))
    }

    /// Brings another app in front, by its pid: what was there before Logic was brought forward.
    pub fn activate_pid(&mut self, pid: u32) -> Result<bool, AxError> {
        let reply = self.call("activate", json!({"pid": pid}))?;
        Ok(reply["active"] == Value::Bool(true))
    }

    /// Posts a key with modifiers to Logic: whether Logic was in front to take it.
    pub fn keys(&mut self, key: &str, mods: &[&str]) -> Result<bool, AxError> {
        let reply = self.call("keys", json!({"pid": self.pid, "key": key, "mods": mods}))?;
        Ok(reply["active"] == Value::Bool(true))
    }

    /// Posts a key given as the helper takes it (a name, a character or a virtual key code) with modifiers.
    pub fn chord(&mut self, key: &Value, mods: &[&str]) -> Result<bool, AxError> {
        let reply = self.call("keys", json!({"pid": self.pid, "key": key, "mods": mods}))?;
        Ok(reply["active"] == Value::Bool(true))
    }

    /// Presses a menu item by its titles from the menu bar down: the title as shown.
    pub fn menu(&mut self, titles: &[&str]) -> Result<String, AxError> {
        let reply = self.call("menu", json!({"pid": self.pid, "titles": titles}))?;
        Ok(reply["title"].as_str().unwrap_or_default().to_string())
    }

    /// Clicks a point on the screen (the helper brings Logic in front first).
    pub fn click(&mut self, x: f64, y: f64) -> Result<(), AxError> {
        self.call("click", json!({"pid": self.pid, "x": x, "y": y}))?;
        Ok(())
    }

    /// A click with modifier keys held (`shift` extends a track selection).
    pub fn click_with(&mut self, x: f64, y: f64, mods: &[&str]) -> Result<(), AxError> {
        self.call("click", json!({"pid": self.pid, "x": x, "y": y, "mods": mods}))?;
        Ok(())
    }

    /// An element's frame on the screen: its position and size.
    pub fn frame(&mut self, root: &Root, path: &str) -> Result<(f64, f64, f64, f64), AxError> {
        let attrs = self.get(root, path, &["AXPosition", "AXSize"])?;
        let number = |key: &str, field: &str| attrs.get(key).and_then(|v| v.get(field)).and_then(Value::as_f64).unwrap_or(0.0);
        Ok((number("AXPosition", "x"), number("AXPosition", "y"), number("AXSize", "w"), number("AXSize", "h")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::MockHands;

    #[test]
    fn a_node_is_read_from_the_helpers_shape_and_written_back_the_same() {
        let json = json!({"i": "5.0", "role": "AXGroup", "desc": "Control Bar", "c": [
            {"i": "5.0.2", "role": "AXSlider", "desc": "Tempo", "value": 160, "valueDesc": "160", "enabled": true, "set": ["AXValue"], "act": ["AXIncrement"]},
            {"i": "5.0.4", "role": "AXPopUpButton", "desc": "Time Signature", "value": "4/4", "more": 3}
        ]});
        let node = Node::from_json(&json);
        assert_eq!((node.path.as_str(), node.role.as_str(), node.desc.as_str()), ("5.0", "AXGroup", "Control Bar"));
        let tempo = node.child(0).unwrap();
        assert_eq!((tempo.number(), tempo.value_desc.as_str(), tempo.enabled), (Some(160.0), "160", Some(true)));
        assert_eq!(tempo.settable, ["AXValue"]);
        assert_eq!(node.child(1).unwrap().more, 3);
        assert_eq!(node.at("1").map(|n| n.value_text()), Some("4/4".into()));
        assert_eq!(node.at("7"), None);
        assert_eq!(node.find(&|n| n.desc == "Time Signature").map(|n| n.path.as_str()), Some("5.0.4"));
        assert_eq!(node.count(), 3);
        assert_eq!(node.to_json(), json);
        assert_eq!(join("", 3), "3");
        assert_eq!(join("5.0", 3), "5.0.3");
        let switch = Node { value: json!("on"), ..Node::default() };
        assert!(switch.is_on());
        assert!(!Node { value: json!(0), ..Node::default() }.is_on());
    }

    #[test]
    fn the_walker_asks_the_helper_by_root_and_path_and_counts_its_calls() {
        let hands = MockHands::new();
        hands.set_tree(json!({"role": "AXWindow", "c": [{"i": "0", "role": "AXButton"}]}));
        let mut launcher = hands.clone();
        let mut helper = crate::helper::Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 42);
        let node = ax.tree(&Root::Main, "", 2, 50, Some(BRIEF)).unwrap();
        assert_eq!(node.children.len(), 1);
        let (op, params) = hands.calls().remove(0);
        assert_eq!(op, "tree");
        assert_eq!(params["root"], "main");
        assert_eq!(params["pid"], 42);
        assert_eq!(params["depth"], 2);
        assert!(params["attrs"].as_array().unwrap().iter().any(|a| a == "desc"));
        assert_eq!(ax.calls, 1);
        assert_eq!(Root::Title("Mixer".into()).to_json(), json!({"title": "Mixer"}));
        hands.reply("get", Ok(json!({"attrs": {"AXValue": "4/4"}})));
        let attrs = ax.get(&Root::Main, "5.0.4", &["AXValue"]).unwrap();
        assert_eq!(attrs["AXValue"], "4/4");
        hands.set_trusted(false);
        let error = ax.tree(&Root::Main, "", 1, 10, None).unwrap_err();
        assert!(untrusted(&error));
        assert_eq!(error.to_string(), "the helper refused: untrusted");
    }
}
