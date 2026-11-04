//! The Doofer, Splitter and Notepad changes Kumi makes in Renoise (M4 n-devices): what Lua can't reach (the devices inside
//! a Doofer or Splitter, its macros' names and mappings, a Splitter's split, a Notepad's text) lives in the device's preset
//! data, an XML text. The host reads it from the tool (`preset_read`), edits it here (xml.rs keeps every byte it doesn't
//! edit), and asks the tool to write it in one step of Renoise's undo while the device's shape (its XML less the values
//! that move by themselves) is still the one it edited.
//!
//! Every index written is checked first: Renoise 3.5.4 crashes on a macro mapping into chain -1 and on routing devices
//! inside a Doofer, and doesn't clamp values. Kumi's undo is the edit taken back from what is there now: a structural edit
//! (a device added, deleted, moved) only while nothing structural changed in the device since (the shape), a value only
//! while it still holds what Kumi wrote.
use crate::renoise::devices;
use crate::renoise::xml::{self, Element};
use serde_json::{json, Map, Value};

/// The change kinds made here, as the tool registers them (`changes_presets.lua`).
pub const KINDS: [&str; 8] = [
    "doofer-macro",
    "doofer-device-add",
    "doofer-device-delete",
    "doofer-device-move",
    "doofer-device-set",
    "splitter-set",
    "notepad-write",
    "doofer-make",
];

/// Whether the host makes this kind from the device's preset data (Renoise only).
pub fn is_host_kind(daw: daw_bridge::Daw, kind: &str) -> bool {
    daw == daw_bridge::Daw::Renoise && KINDS.contains(&kind)
}

/// Whether the host makes this change: the kinds above, and an instrument's macro mappings added or removed.
pub fn host_makes(daw: daw_bridge::Daw, kind: &str, input: &Value) -> bool {
    is_host_kind(daw, kind) || crate::renoise::instruments::is_host_change(daw, kind, input)
}

/// Whether the host takes this change back too (doofer-make's undo is the tool's own: it makes the devices again).
pub fn composes_undo(daw: daw_bridge::Daw, kind: &str, input: &Value) -> bool {
    host_makes(daw, kind, input) && kind != "doofer-make"
}

/// The most preset data edited (as the tool's bound).
pub const MAX_PRESET_BYTES: usize = 8 * 1024 * 1024;

/// A text's print: two 32-bit sums of its bytes and its length, as the tool computes it (`changes_presets.lua`).
pub fn print_of(text: &str) -> String {
    let (mut a, mut b) = (5381u32, 0u32);
    for byte in text.bytes() {
        a = a.wrapping_mul(33).wrapping_add(byte as u32);
        b = b.wrapping_mul(65599).wrapping_add(byte as u32);
    }
    format!("{a:08x}{b:08x}:{}", text.len())
}

/// `<tag>…</tag>` with no tag inside becomes `<tag/>`, everywhere in `text`.
fn blanked(text: &str, tag: &str) -> String {
    let (open, close, empty) = (format!("<{tag}>"), format!("</{tag}>"), format!("<{tag}/>"));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&open) {
        out.push_str(&rest[..at]);
        let after = &rest[at + open.len()..];
        match after.find('<') {
            Some(end) if after[end..].starts_with(&close) => {
                out.push_str(&empty);
                rest = &after[end + close.len()..];
            }
            _ => {
                out.push_str(&open);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What Renoise writes into a device's preset data as its view changes (the producer folds it, shows its devices or a
/// Splitter's other chain, moves a plug-in's editor): no change to what it holds. The tool's `VIEW_STATE` is the same.
pub const VIEW_STATE: [&str; 5] =
    ["IsMaximized", "ShowDevices", "CurrentlyVisibleChain", "MonitorVisibleChain", "PluginEditorWindowPosition"];

/// Preset data with what moves by itself left out, printed: as the tool's `shape_of`. Parameter values and plug-in chunks
/// (automation, meta devices and macros move them), the inner devices' `SelectedPresetIsModified` and `IsSelected`
/// (Renoise 3.5.4 turns the first true when it writes a container's data again, and the second follows the producer's
/// clicks: lab, 2026-10-20), and what Renoise's view shows of them (`VIEW_STATE`).
pub fn shape_of(xml: &str) -> String {
    let shape = blanked(xml, "Value");
    // `<ParameterChunk>…</ParameterChunk>` (the first end after each start) becomes `<ParameterChunk/>`.
    let mut out = String::with_capacity(shape.len());
    let mut rest = shape.as_str();
    while let Some(at) = rest.find("<ParameterChunk>") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 16..];
        match after.find("</ParameterChunk>") {
            Some(end) => {
                out.push_str("<ParameterChunk/>");
                rest = &after[end + 17..];
            }
            None => {
                out.push_str("<ParameterChunk>");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let mut shape = blanked(&blanked(&out, "SelectedPresetIsModified"), "IsSelected");
    for tag in VIEW_STATE {
        shape = blanked(&shape, tag);
    }
    print_of(&shape)
}

/// A device inside's content, printed: its whole XML (values, plug-in state, switch) but what moves by itself and what
/// the view shows. What Kumi's undo of adding it compares with the one Renoise wrote right after the add.
pub fn content_of(device: &Element) -> String {
    let mut text = device.to_xml();
    for tag in ["SelectedPresetIsModified", "IsSelected"].into_iter().chain(VIEW_STATE) {
        text = blanked(&text, tag);
    }
    print_of(&text)
}

/// The content of the device at `at` (places from 1) in preset data: what Kumi keeps of one it added.
pub fn content_at(text: &str, at: &[i64]) -> Option<String> {
    let document = xml::parse(text).ok()?;
    let locus = resolve(slot(&document).ok()?, at, false).ok()?;
    let container = element_at(slot(&document).ok()?, &locus.container)?;
    let kind = type_of(container, locus.container.is_empty());
    devices_in(container, kind, locus.lane).get(locus.place).map(|device| content_of(device))
}

const DOOFER: &str = "DooferDevice";
const SPLITTER: &str = "SplitterDevice";
const NOTEPAD: &str = "NotePadDevice";
const PLUGIN: &str = "AudioPluginDevice";

/// Why an edit can't be made: the request asks what can't be (`bad-params`), or what it was made against moved
/// (`changed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Bad(String),
    Changed(String),
}

impl Refusal {
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::Bad(_) => "bad-params",
            Refusal::Changed(_) => "changed",
        }
    }
    pub fn message(&self) -> &str {
        match self {
            Refusal::Bad(m) | Refusal::Changed(m) => m,
        }
    }
}

pub(crate) fn bad<T>(message: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal::Bad(message.into()))
}

pub(crate) fn changed<T>(message: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal::Changed(message.into()))
}

/// Curly quotes around a name inside a sentence, as Kumi's titles have them.
pub(crate) fn q(text: &str) -> String {
    format!("\u{201c}{text}\u{201d}")
}

/// What the tool's `preset_read` told of the device: its words, and the paths it can make.
#[derive(Debug, Clone, Default)]
pub struct Device {
    /// The device's name as the producer sees it, and its track's.
    pub label: String,
    pub track: String,
    pub path: String,
    /// Which plug-in paths asked about the track can take (`available`).
    pub available: Map<String, Value>,
}

impl Device {
    pub fn from_read(read: &Value) -> Self {
        let text = |key: &str| read.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        Device {
            label: text("label"),
            track: text("trackName"),
            path: text("path"),
            available: read.get("available").and_then(Value::as_object).cloned().unwrap_or_default(),
        }
    }

    fn named(&self) -> String {
        format!("{} on {}", self.label, self.track)
    }
}

// The model: a preset's slot, the containers inside it, their lanes and devices.

/// The element under the document's root that is the device (`<DeviceSlot type="…">`).
fn slot(document: &xml::Document) -> Result<&Element, Refusal> {
    document.root.child("DeviceSlot").map_or_else(|| bad("the preset data has no DeviceSlot"), Ok)
}

fn slot_mut(document: &mut xml::Document) -> &mut Element {
    document.root.child_mut("DeviceSlot").expect("the slot was there when read")
}

/// A device element's type: the top slot's `type`, an inner device's element name.
fn type_of(element: &Element, top: bool) -> &str {
    if top {
        element.attribute("type").unwrap_or_default()
    } else {
        element.name()
    }
}

fn is_container(kind: &str) -> bool {
    kind == DOOFER || kind == SPLITTER
}

fn lanes_of(kind: &str) -> usize {
    if kind == SPLITTER {
        2
    } else {
        1
    }
}

fn chain_name(kind: &str, lane: usize) -> &'static str {
    match (kind == SPLITTER, lane) {
        (true, 0) => "DeviceChain0",
        (true, _) => "DeviceChain1",
        _ => "DeviceChain",
    }
}

fn macro_count(kind: &str) -> usize {
    if kind == SPLITTER {
        16
    } else {
        8
    }
}

fn element_at<'a>(slot: &'a Element, path: &[usize]) -> Option<&'a Element> {
    path.iter().try_fold(slot, |element, &i| element.element(i))
}

fn element_at_mut<'a>(slot: &'a mut Element, path: &[usize]) -> Option<&'a mut Element> {
    let mut element = slot;
    for &i in path {
        element = element.element_mut(i)?;
    }
    Some(element)
}

fn position_of(parent: &Element, name: &str) -> Option<usize> {
    parent.elements().position(|element| element.name() == name)
}

/// The devices in a container's lane (none when its chain holds no list yet).
fn devices_in<'a>(container: &'a Element, kind: &str, lane: usize) -> Vec<&'a Element> {
    container
        .child(chain_name(kind, lane))
        .and_then(|chain| chain.child("Devices"))
        .map(|list| list.elements().collect())
        .unwrap_or_default()
}

/// A place inside a container: the containers walked from the top (each as its element path from the slot), the lane
/// (0 or 1) and the place (from 0).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Locus {
    container: Vec<usize>,
    lane: usize,
    place: usize,
    /// The places as the request gave them, for the words.
    asked: Vec<i64>,
}

/// Where `at` points: places from 1, a Splitter's lane (1 or 2) before its place, a nested container's place first.
/// `insert`: the last place may be one past the end.
fn resolve(slot: &Element, at: &[i64], insert: bool) -> Result<Locus, Refusal> {
    if at.is_empty() {
        return bad("at: the device's place, from 1");
    }
    let mut container: Vec<usize> = vec![];
    let mut i = 0;
    loop {
        let element = element_at(slot, &container).expect("a walked container");
        let kind = type_of(element, container.is_empty()).to_string();
        if !is_container(&kind) {
            return bad("at: that is no Doofer or Splitter");
        }
        let lane = if kind == SPLITTER {
            let Some(&lane) = at.get(i) else { return bad("at: a Splitter's lane (1 or 2) comes before the place") };
            if !(1..=2).contains(&lane) {
                return bad(format!("at: a Splitter's lane is 1 or 2, not {lane}"));
            }
            i += 1;
            (lane - 1) as usize
        } else {
            0
        };
        let Some(&place) = at.get(i) else { return bad("at: the device's place in its chain, from 1") };
        i += 1;
        let list = devices_in(element, &kind, lane);
        let last = i == at.len();
        let most = if last && insert { list.len() + 1 } else { list.len() };
        if place < 1 || place as usize > most {
            let count = list.len();
            return bad(format!(
                "at: there is no place {place} there (it holds {count} device{}{})",
                if count == 1 { "" } else { "s" },
                if last && insert { format!("; {} is the end", count + 1) } else { String::new() }
            ));
        }
        if last {
            return Ok(Locus { container, lane, place: place as usize - 1, asked: at.to_vec() });
        }
        let device = list[place as usize - 1];
        if !is_container(device.name()) {
            return bad(format!("at: the device at place {place} ({}) holds no devices", inner_label(device)));
        }
        let chain = position_of(element, chain_name(&kind, lane)).expect("its chain");
        let devices = position_of(element.element(chain).expect("its chain"), "Devices").expect("its list");
        container.extend([chain, devices, place as usize - 1]);
    }
}

/// The list a place is in, made when the chain has none yet.
fn list_mut<'a>(slot: &'a mut Element, container: &[usize], lane: usize) -> &'a mut Element {
    let element = element_at_mut(slot, container).expect("a resolved container");
    let kind = type_of(element, container.is_empty()).to_string();
    let chain = chain_name(&kind, lane);
    if element.child(chain).is_none() {
        let count = element.element_count();
        element.insert_element(count, Element::new(chain));
    }
    let chain = element.child_mut(chain).expect("made above");
    if chain.child("Devices").is_none() {
        let count = chain.element_count();
        chain.insert_element(count, Element::new("Devices"));
    }
    chain.child_mut("Devices").expect("made above")
}

/// The types of the devices in the lane a place is in, and its ancestors' types: what tells it is the same chain.
fn chain_print(slot: &Element, container: &[usize], lane: usize) -> Value {
    let element = element_at(slot, container).expect("a resolved container");
    let kind = type_of(element, container.is_empty());
    let types: Vec<&str> = devices_in(element, kind, lane).iter().map(|device| device.name()).collect();
    let mut owners = vec![];
    for depth in (0..container.len()).step_by(3) {
        if let Some(owner) = element_at(slot, &container[..depth]) {
            owners.push(type_of(owner, depth == 0).to_string());
        }
    }
    json!({"owners": owners, "lane": lane, "types": types})
}

thread_local! {
    /// The names Renoise gives the plug-ins in the preset data being read or edited, by identifier (a preset_read's
    /// `plugins`): a plug-in inside a Doofer has none in its XML.
    static PLUGIN_NAMES: std::cell::RefCell<Map<String, Value>> = std::cell::RefCell::new(Map::new());
}

/// Runs `f` with these plug-in names known to the labels of the devices it reads.
pub(crate) fn with_plugin_names<T>(names: Option<&Value>, f: impl FnOnce() -> T) -> T {
    let names = names.and_then(Value::as_object).cloned().unwrap_or_default();
    PLUGIN_NAMES.with(|cell| *cell.borrow_mut() = names);
    let out = f();
    PLUGIN_NAMES.with(|cell| cell.borrow_mut().clear());
    out
}

/// A device inside a container as the producer sees it: its own name, or its type's.
fn inner_label(device: &Element) -> String {
    if let Some(name) = device.child_text("CustomDeviceName").filter(|name| !name.is_empty()) {
        return name;
    }
    if device.name() == PLUGIN {
        if let Some(name) = device.child_text("PluginShortDisplayName").or_else(|| device.child_text("PluginDisplayName")) {
            return name;
        }
        let named = device
            .child_text("PluginIdentifier")
            .and_then(|id| PLUGIN_NAMES.with(|cell| cell.borrow().get(&id).and_then(Value::as_str).map(str::to_owned)));
        if let Some(name) = named {
            return name;
        }
    }
    devices::by_xml(device.name()).map_or_else(|| device.name().trim_end_matches("Device").to_string(), |d| d.name.to_string())
}

/// Where a place is, in words: "place 2", "lane 2, place 1", "place 3 > place 1".
fn place_words(at: &[i64], slot: &Element) -> String {
    let mut words = vec![];
    let mut container: Vec<usize> = vec![];
    let mut i = 0;
    while i < at.len() {
        let Some(element) = element_at(slot, &container) else { break };
        let kind = type_of(element, container.is_empty()).to_string();
        let mut lane = 0;
        if kind == SPLITTER && i < at.len() {
            lane = (at[i] - 1).max(0) as usize;
            words.push(format!("lane {}", at[i]));
            i += 1;
        }
        let Some(&place) = at.get(i) else { break };
        words.push(format!("place {place}"));
        i += 1;
        let list = devices_in(element, &kind, lane);
        let Some(device) = list.get(place as usize - 1) else { break };
        let (Some(chain), true) = (position_of(element, chain_name(&kind, lane)), is_container(device.name())) else { break };
        let Some(devices) = element.element(chain).and_then(|chain| position_of(chain, "Devices")) else { break };
        container.extend([chain, devices, place as usize - 1]);
    }
    words.join(", ")
}

fn number(element: Option<&Element>) -> Option<f64> {
    element.and_then(Element::text).and_then(|text| text.trim().parse::<f64>().ok())
}

/// A parameter element's value (`<Name><Value>v</Value></Name>`).
fn value_of(device: &Element, name: &str) -> Option<f64> {
    number(device.child(name).and_then(|parameter| parameter.child("Value")))
}

fn set_value(device: &mut Element, name: &str, value: f64) {
    if device.child(name).is_none() {
        let count = device.element_count();
        device.insert_element(count, Element::new(name).with_child(Element::new("Value").with_text(&xml::number_text(value))));
        return;
    }
    let parameter = device.child_mut(name).expect("there");
    // (a whole number written as Renoise wrote the one it replaces: "3" stays "3"-like, "3.0" "3.0"-like)
    let whole = parameter.child_text("Value").is_some_and(|now| !now.contains('.') && !now.contains('e'));
    let text = if whole && value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { xml::number_text(value) };
    parameter.set_child_text("Value", &text);
}

// Macro mappings and the meta devices inside: what follows a device that is added, deleted or moved.

/// A mapping a change took away: its macro, its place in the macro's list, and its XML.
fn macros_of(container: &Element, kind: &str) -> Vec<(usize, usize)> {
    (0..macro_count(kind)).filter_map(|m| position_of(container, &format!("Macro{m}")).map(|at| (m, at))).collect()
}

/// The mappings of the container into `lane`: each device index n becomes `follow(n)`, or the mapping goes (None).
/// Answers what went: [{macro, at, xml}], `at` its place in the macro's list before any went, ascending in each macro
/// (so putting them back in that order, each at its `at`, gives the list it was).
fn remap(container: &mut Element, kind: &str, lane: usize, follow: &dyn Fn(usize) -> Option<usize>) -> Vec<Value> {
    let mut gone = vec![];
    for (m, at) in macros_of(container, kind) {
        let Some(mappings) = container.element_mut(at).and_then(|macro_| macro_.child_mut("Mappings")) else { continue };
        let mut k = 0;
        let mut removed = 0;
        while k < mappings.element_count() {
            let mapping = mappings.element(k).expect("in range");
            let chain = number(mapping.child("DestChainIndex")).unwrap_or(0.0) as i64;
            let device = number(mapping.child("DestDeviceIndex")).unwrap_or(-1.0) as i64;
            if chain == lane as i64 && device >= 0 {
                match follow(device as usize) {
                    None => {
                        let element = mappings.remove_element(k).expect("in range");
                        gone.push(json!({"macro": m, "at": k + removed, "xml": element.to_xml()}));
                        removed += 1;
                        continue;
                    }
                    Some(n) if n as i64 != device => {
                        mappings.element_mut(k).expect("in range").set_child_text("DestDeviceIndex", &n.to_string());
                    }
                    _ => {}
                }
            }
            k += 1;
        }
    }
    gone
}

/// The destination fields of a meta device inside a chain: "" for Dest…, "Out1".."Out9" for *Hydra and *XY Pad.
fn destination_prefixes(device: &Element) -> Vec<String> {
    device.elements().filter_map(|element| element.name().strip_suffix("DestEffect").map(str::to_owned)).collect()
}

/// The meta devices in a container's lane aim at devices of that lane by index: each index n becomes `follow(n)`, or
/// none (-1, with its parameter) when None. Answers what was cleared: [{device, prefix, effect, parameter}].
fn reaim(container: &mut Element, kind: &str, lane: usize, follow: &dyn Fn(usize) -> Option<usize>) -> Vec<Value> {
    reaim_skipping(container, kind, lane, follow, None)
}

/// As `reaim`, the device at `skip` left as it is (one just put in, whose aims are in the lane's numbering with it).
fn reaim_skipping(
    container: &mut Element,
    kind: &str,
    lane: usize,
    follow: &dyn Fn(usize) -> Option<usize>,
    skip: Option<usize>,
) -> Vec<Value> {
    let mut cleared = vec![];
    let Some(list) = container.child_mut(chain_name(kind, lane)).and_then(|chain| chain.child_mut("Devices")) else { return cleared };
    for (d, device) in list.elements_mut().enumerate() {
        if skip == Some(d) {
            continue;
        }
        for prefix in destination_prefixes(device) {
            let effect_name = format!("{prefix}DestEffect");
            let parameter_name = format!("{prefix}DestParameter");
            let Some(effect) = value_of(device, &effect_name) else { continue };
            if effect < 0.0 {
                continue;
            }
            match follow(effect as usize) {
                None => {
                    let parameter = value_of(device, &parameter_name).unwrap_or(-1.0);
                    set_value(device, &effect_name, -1.0);
                    if device.child(&parameter_name).is_some() {
                        set_value(device, &parameter_name, -1.0);
                    }
                    cleared.push(json!({"device": d, "prefix": prefix, "effect": effect, "parameter": parameter}));
                }
                Some(n) if n as f64 != effect => set_value(device, &effect_name, n as f64),
                _ => {}
            }
        }
    }
    cleared
}

/// What in the container drives this parameter of the device at `place` in `lane` (its macros' mappings and the meta
/// devices beside it), in words.
fn drivers(container: &Element, kind: &str, lane: usize, place: usize, parameter: usize) -> Vec<String> {
    let mut found = vec![];
    for (m, at) in macros_of(container, kind) {
        let macro_ = container.element(at).expect("in range");
        for mapping in macro_.child("Mappings").map(|list| list.elements().collect::<Vec<_>>()).unwrap_or_default() {
            let chain = number(mapping.child("DestChainIndex")).unwrap_or(0.0) as i64;
            let device = number(mapping.child("DestDeviceIndex")).unwrap_or(-1.0) as i64;
            let target = number(mapping.child("DestParameterIndex")).unwrap_or(-1.0) as i64;
            if chain == lane as i64 && device == place as i64 && target == parameter as i64 {
                let name = macro_.child_text("Name").unwrap_or_else(|| format!("Macro {}", m + 1));
                found.push(format!("the macro {}", q(&name)));
            }
        }
    }
    for device in devices_in(container, kind, lane) {
        for prefix in destination_prefixes(device) {
            let effect = value_of(device, &format!("{prefix}DestEffect")).unwrap_or(-1.0);
            let target = value_of(device, &format!("{prefix}DestParameter")).unwrap_or(-1.0);
            if effect as i64 == place as i64 && target as i64 == parameter as i64 {
                found.push(q(&inner_label(device)));
            }
        }
    }
    found
}

// Parameters of the devices inside: Renoise's table (devices.rs) for native devices, a nested container's macros.

/// How many parameters a device inside has (a mapping's parameter index runs to it; 0 is its on/off), or why Kumi can't
/// tell.
fn parameter_count(device: &Element) -> Result<usize, Refusal> {
    match device.name() {
        DOOFER => Ok(8),
        SPLITTER => Ok(19),
        PLUGIN => bad(format!(
            "Kumi can't tell how many parameters the plug-in {} inside has, so it maps no macro to it (map it in Renoise)",
            q(&inner_label(device))
        )),
        other => devices::by_xml(other).map(|d| d.params.len()).map_or_else(|| bad(format!("Kumi doesn't know the device {other}")), Ok),
    }
}

/// A parameter of a device inside, by number (from 1; 0 is its on/off) or name (as Renoise names it, its element, or the
/// start of a name): its number.
fn parameter_number(device: &Element, wanted: &Value, allow_switch: bool) -> Result<usize, Refusal> {
    let count = parameter_count(device)?;
    if let Some(n) = wanted.as_i64() {
        let lowest = if allow_switch { 0 } else { 1 };
        if n < lowest || n as usize > count {
            return bad(format!("param is a number from {lowest} to {count} for {}", q(&inner_label(device))));
        }
        return Ok(n as usize);
    }
    let Some(asked) = wanted.as_str().filter(|text| !text.is_empty()) else { return bad("param: a parameter's number or name") };
    let names = parameter_names(device);
    let lower = asked.to_lowercase();
    for exact in [true, false] {
        let mut found = None;
        for (k, aliases) in names.iter().enumerate() {
            let hit = aliases.iter().any(|alias| {
                let alias = alias.to_lowercase();
                if exact {
                    alias == lower
                } else {
                    alias.starts_with(&lower)
                }
            });
            if hit {
                if found.is_some() {
                    return bad(format!("{asked:?} names more than one parameter of {}: give its number", q(&inner_label(device))));
                }
                found = Some(k + 1);
            }
        }
        if let Some(k) = found {
            return Ok(k);
        }
    }
    let listed: Vec<String> = names.iter().take(24).map(|aliases| aliases[0].clone()).collect();
    bad(format!("{} has no parameter {asked:?} (it has {})", q(&inner_label(device)), listed.join(", ")))
}

/// Each parameter's names: Renoise's (a Doofer's macros by their names; a Splitter's lane gains by every split's
/// words) and its element's.
fn parameter_names(device: &Element) -> Vec<Vec<String>> {
    match device.name() {
        DOOFER | SPLITTER => {
            let kind = device.name();
            let mut names: Vec<Vec<String>> = (0..macro_count(kind))
                .map(|m| {
                    let own = device.child(&format!("Macro{m}")).and_then(|macro_| macro_.child_text("Name"));
                    let default = format!("Macro {}", m + 1);
                    own.into_iter().chain([default]).collect()
                })
                .collect();
            if kind == SPLITTER {
                names.push(vec!["L1 Gain".into(), "Low Gain".into(), "Mid Gain".into(), "VolumeChain1".into()]);
                names.push(vec!["L2 Gain".into(), "High Gain".into(), "Side Gain".into(), "VolumeChain2".into()]);
                names.push(vec!["Frequency".into(), "SplitFrequency".into()]);
            }
            names
        }
        other => devices::by_xml(other)
            .map(|d| {
                d.params
                    .iter()
                    .map(|p| [p.name.to_string(), p.element.to_string()].into_iter().filter(|n| !n.is_empty()).collect())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A parameter of a device inside: its element (where its value is saved; "" for a trigger Renoise doesn't save) and its
/// range.
fn parameter_spec(device: &Element, k: usize) -> Option<(String, f64, f64)> {
    match device.name() {
        DOOFER | SPLITTER if k <= macro_count(device.name()) => Some((format!("Macro{}", k - 1), 0.0, 100.0)),
        SPLITTER => match k {
            17 => Some(("VolumeChain1".into(), 0.0, 1.412_537_575)),
            18 => Some(("VolumeChain2".into(), 0.0, 1.412_537_575)),
            19 => Some(("SplitFrequency".into(), 0.0, 1.0)),
            _ => None,
        },
        other => devices::by_xml(other).and_then(|d| d.params.get(k - 1)).map(|p| {
            // (a meta device's destinations take -1 for none, below their range)
            let min = if p.element.ends_with("DestTrack") || p.element.ends_with("DestEffect") || p.element.ends_with("DestParameter") {
                -1.0
            } else {
                p.min
            };
            (p.element.to_string(), min, p.max)
        }),
    }
}

fn parameter_name(device: &Element, k: usize) -> String {
    if k == 0 {
        return "on/off".into();
    }
    parameter_names(device).get(k - 1).and_then(|aliases| aliases.first().cloned()).unwrap_or_else(|| format!("parameter {k}"))
}

// A device to put inside: a native device by name, or an Audio Unit by its path.

const NATIVE: &str = "Audio/Effects/Native/";
const AU: &str = "Audio/Effects/AU/";

fn new_device(asked: &str, device: &Device) -> Result<Element, Refusal> {
    let name = asked.strip_prefix(NATIVE).unwrap_or(asked);
    if let Some(native) = devices::by_name(name) {
        if !native.nests {
            return bad(format!("{} can't go inside a Doofer or Splitter (Renoise crashes on routing devices there)", q(native.name)));
        }
        return Ok(Element::new(native.xml).with_attribute("type", native.xml));
    }
    if let Some(id) = asked.strip_prefix(AU) {
        if device.available.get(asked).and_then(Value::as_bool) != Some(true) {
            return bad(format!("Renoise has no Audio Unit {asked} here (a path as available devices list it)"));
        }
        return Ok(Element::new(PLUGIN)
            .with_attribute("type", PLUGIN)
            .with_child(Element::new("PluginType").with_text("AU"))
            .with_child(Element::new("PluginIdentifier").with_text(id)));
    }
    if asked.starts_with("Audio/Effects/") {
        return bad("only native devices and Audio Units go inside a Doofer or Splitter for now");
    }
    let near: Vec<&str> = devices::DEVICES
        .iter()
        .filter(|d| d.nests && d.name.to_lowercase().contains(&name.to_lowercase()))
        .map(|d| d.name)
        .take(5)
        .collect();
    bad(format!(
        "Renoise has no native device {asked:?}{}",
        if near.is_empty() { String::new() } else { format!(" (close: {})", near.join(", ")) }
    ))
}

// The edits. Each answers the edited document, its words, and what Kumi's undo needs (`prior`, what was there; `applied`,
// what the edit left).

/// One edit, made.
#[derive(Debug)]
pub struct Edited {
    pub document: xml::Document,
    pub title: String,
    pub note: Option<String>,
    pub prior: Value,
    pub applied: Value,
}

fn int_list(value: Option<&Value>, name: &str) -> Result<Option<Vec<i64>>, Refusal> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let list: Option<Vec<i64>> = items.iter().map(Value::as_i64).collect();
            list.map(Some).map_or_else(|| bad(format!("{name}: places, whole numbers from 1")), Ok)
        }
        Some(_) => bad(format!("{name}: a list of places, from 1")),
    }
}

fn text_field<'a>(input: &'a Value, name: &str) -> Result<Option<&'a str>, Refusal> {
    match input.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => bad(format!("{name} must be text")),
    }
}

/// Makes the edit `kind` asks on the device's preset data.
pub fn edit(kind: &str, input: &Value, text: &str, device: &Device) -> Result<Edited, Refusal> {
    if text.len() > MAX_PRESET_BYTES {
        return bad(format!("{} holds more preset data than Kumi edits", q(&device.named())));
    }
    let document = xml::parse(text).map_err(|e| Refusal::Bad(format!("the preset data of {} can't be read: {e}", q(&device.named()))))?;
    let top = type_of(slot(&document)?, true).to_string();
    match kind {
        "notepad-write" => notepad_write(document, &top, input, device),
        "splitter-set" if top != SPLITTER => bad(format!("{} isn't a Splitter", q(&device.named()))),
        _ if kind != "notepad-write" && !is_container(&top) => bad(format!("{} isn't a Doofer or Splitter", q(&device.named()))),
        "doofer-macro" => doofer_macro(document, &top, input, device),
        "doofer-device-add" => device_add(document, &top, input, device),
        "doofer-device-delete" => device_delete(document, input, device),
        "doofer-device-move" => device_move(document, input, device),
        "doofer-device-set" => device_set(document, input, device),
        "splitter-set" => splitter_set(document, input, device),
        other => bad(format!("{other} isn't made here")),
    }
}

// doofer-macro

pub(crate) const SCALINGS: [(&str, &str); 5] =
    [("log-fast", "Log Fast"), ("log-slow", "Log Slow"), ("linear", "Linear"), ("exp-slow", "Exp Slow"), ("exp-fast", "Exp Fast")];

pub(crate) fn scaling_word(xml: &str) -> &'static str {
    SCALINGS.iter().find(|(_, x)| *x == xml).map_or("linear", |(word, _)| word)
}

/// A macro's mappings, as Kumi names them: [{lane, device, param, min, max, scaling}] (from 1).
fn mappings_of(macro_: &Element) -> Vec<Value> {
    macro_
        .child("Mappings")
        .map(|list| list.elements().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|mapping| {
            json!({
                "lane": number(mapping.child("DestChainIndex")).unwrap_or(0.0) as i64 + 1,
                "device": number(mapping.child("DestDeviceIndex")).unwrap_or(-1.0) as i64 + 1,
                "param": number(mapping.child("DestParameterIndex")).unwrap_or(-1.0) as i64,
                "min": number(mapping.child("Min")).unwrap_or(0.0),
                "max": number(mapping.child("Max")).unwrap_or(1.0),
                "scaling": scaling_word(&mapping.child_text("Scaling").unwrap_or_default()),
            })
        })
        .collect()
}

fn same_mappings(a: &Value, b: &Value) -> bool {
    let (Some(a), Some(b)) = (a.as_array(), b.as_array()) else { return a == b };
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            ["lane", "device", "param", "scaling"].iter().all(|k| x.get(*k) == y.get(*k))
                && ["min", "max"].iter().all(|k| {
                    let (p, r) = (x.get(*k).and_then(Value::as_f64), y.get(*k).and_then(Value::as_f64));
                    matches!((p, r), (Some(p), Some(r)) if (p - r).abs() <= 1e-6)
                })
        })
}

fn mapping_element(mapping: &Value) -> Element {
    let n = |k: &str| mapping.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let scaling = mapping.get("scaling").and_then(Value::as_str).unwrap_or("linear");
    let xml_scaling = SCALINGS.iter().find(|(w, _)| *w == scaling).map_or("Linear", |(_, x)| x);
    Element::new("Mapping")
        .with_child(Element::new("DestChainIndex").with_text(&((n("lane") as i64) - 1).to_string()))
        .with_child(Element::new("DestDeviceIndex").with_text(&((n("device") as i64) - 1).to_string()))
        .with_child(Element::new("DestParameterIndex").with_text(&(n("param") as i64).to_string()))
        .with_child(Element::new("Min").with_text(&xml::number_text(n("min"))))
        .with_child(Element::new("Max").with_text(&xml::number_text(n("max"))))
        .with_child(Element::new("Scaling").with_text(xml_scaling))
}

/// The mappings asked, each checked against the container: its lane, its device (from 1), the parameter's number.
fn checked_mappings(container: &Element, kind: &str, asked: &Value) -> Result<Vec<Value>, Refusal> {
    let Some(items) = asked.as_array() else { return bad("mappings: a list of {device, param, ...}") };
    if items.len() > 64 {
        return bad("mappings: at most 64 for one macro");
    }
    let mut out = vec![];
    for (i, item) in items.iter().enumerate() {
        let lane = match item.get("lane") {
            None | Some(Value::Null) => 1,
            Some(lane) => lane.as_i64().unwrap_or(0),
        };
        if lane < 1 || lane as usize > lanes_of(kind) {
            return bad(if kind == SPLITTER {
                "mappings: a Splitter's lane is 1 or 2"
            } else {
                "mappings: a Doofer has one chain (no lane)"
            });
        }
        let list = devices_in(container, kind, (lane - 1) as usize);
        let place = item.get("device").and_then(Value::as_i64).unwrap_or(0);
        if place < 1 || place as usize > list.len() {
            return bad(format!(
                "mappings[{i}]: device is a place from 1 to {} in {}",
                list.len(),
                if kind == SPLITTER { format!("lane {lane}") } else { "the Doofer".into() }
            ));
        }
        let target = list[place as usize - 1];
        let param = parameter_number(target, item.get("param").unwrap_or(&Value::Null), true)?;
        let bound = |k: &str, default: f64| -> Result<f64, Refusal> {
            match item.get(k) {
                None | Some(Value::Null) => Ok(default),
                Some(v) => match v.as_f64() {
                    Some(n) if (0.0..=1.0).contains(&n) => Ok(n),
                    _ => bad(format!("mappings[{i}].{k} is from 0 to 1 (of the parameter's range)")),
                },
            }
        };
        let (min, max) = (bound("min", 0.0)?, bound("max", 1.0)?);
        let scaling = match item.get("scaling") {
            None | Some(Value::Null) => "linear",
            Some(s) => match s.as_str().and_then(|s| SCALINGS.iter().find(|(w, _)| *w == s)) {
                Some((word, _)) => word,
                None => return bad("mappings: scaling is log-fast, log-slow, linear, exp-slow or exp-fast"),
            },
        };
        out.push(json!({"lane": lane, "device": place, "param": param, "min": min, "max": max, "scaling": scaling}));
    }
    Ok(out)
}

/// Whether mappings Kumi kept (as `mappings_of` gives them) still have their lane and device in the container.
fn mappings_fit(container: &Element, kind: &str, mappings: &Value) -> Result<(), Refusal> {
    for mapping in mappings.as_array().into_iter().flatten() {
        let lane = mapping["lane"].as_i64().unwrap_or(1);
        let place = mapping["device"].as_i64().unwrap_or(0);
        let fits = lane >= 1
            && lane as usize <= lanes_of(kind)
            && place >= 1
            && place as usize <= devices_in(container, kind, (lane - 1) as usize).len();
        if !fits {
            return changed("a device a mapping went to isn't there any more");
        }
    }
    Ok(())
}

fn doofer_macro(mut document: xml::Document, top: &str, input: &Value, device: &Device) -> Result<Edited, Refusal> {
    let m = input.get("macro").and_then(Value::as_i64).unwrap_or(0);
    let count = macro_count(top);
    if m < 1 || m as usize > count {
        return bad(format!("macro is a number from 1 to {count} for a {}", if top == SPLITTER { "Splitter" } else { "Doofer" }));
    }
    let name = text_field(input, "name")?;
    if name.is_some_and(|n| n.chars().count() > 64 || n.chars().any(char::is_control)) {
        return bad("name must be one line of at most 64 letters");
    }
    let value = match input.get("value") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_f64() {
            Some(n) if (0.0..=100.0).contains(&n) => Some(n),
            _ => return bad("value is from 0 to 100 (%)"),
        },
    };
    let active = match input.get("activeMacros") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_i64() {
            Some(n) if (0..=count as i64).contains(&n) => Some(n),
            _ => return bad(format!("activeMacros is from 0 to {count}")),
        },
    };
    let slot_ = slot(&document)?;
    let tag = format!("Macro{}", m - 1);
    let macro_ = slot_.child(&tag);
    let mappings = match input.get("mappings") {
        None | Some(Value::Null) => None,
        Some(asked) => Some(Value::Array(checked_mappings(slot_, top, asked)?)),
    };
    if name.is_none() && value.is_none() && mappings.is_none() && active.is_none() {
        return bad("give name, value, mappings or activeMacros");
    }
    let mut prior = Map::new();
    let mut applied = Map::new();
    prior.insert("macro".into(), m.into());
    applied.insert("macro".into(), m.into());
    // (the container's lanes as previewed and as left, for an edit of its mappings: they are places in them)
    if mappings.is_some() {
        let chains = Value::Array((0..lanes_of(top)).map(|lane| chain_print(slot_, &[], lane)).collect());
        prior.insert("chains".into(), chains.clone());
        applied.insert("chains".into(), chains);
    }
    if let Some(name) = name {
        prior.insert("name".into(), macro_.and_then(|e| e.child_text("Name")).unwrap_or_else(|| format!("Macro {m}")).into());
        applied.insert("name".into(), name.into());
    }
    if let Some(value) = value {
        prior.insert("value".into(), number(macro_.and_then(|e| e.child("Value"))).unwrap_or(50.0).into());
        applied.insert("value".into(), value.into());
    }
    if let Some(mappings) = &mappings {
        prior.insert("mappings".into(), Value::Array(macro_.map(mappings_of).unwrap_or_default()));
        applied.insert("mappings".into(), mappings.clone());
    }
    if let Some(active) = active {
        prior.insert("activeMacros".into(), number(slot_.child("NumActiveMacros")).map_or(4, |n| n as i64).into());
        applied.insert("activeMacros".into(), active.into());
    }
    let names: Vec<String> = mappings
        .as_ref()
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|mapping| {
                    let lane = mapping["lane"].as_i64().unwrap_or(1) as usize - 1;
                    let place = mapping["device"].as_i64().unwrap_or(1) as usize - 1;
                    let target = devices_in(slot_, top, lane)[place];
                    format!(
                        "{} of {}",
                        q(&parameter_name(target, mapping["param"].as_u64().unwrap_or(0) as usize)),
                        q(&inner_label(target))
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    write_macro(slot_mut(&mut document), top, &Value::Object(applied.clone()));
    let mut parts = vec![];
    if let Some(name) = name {
        parts.push(format!("named {}", q(name)));
    }
    if let Some(value) = value {
        parts.push(format!("at {} %", trimmed(value)));
    }
    if mappings.is_some() {
        parts.push(match names.len() {
            0 => "mapped to nothing".into(),
            1 => format!("mapped to {}", names[0]),
            n => format!("mapped to {} and {} more", names[0], n - 1),
        });
    }
    let mut title = format!("{}: macro {m}", device.named());
    if !parts.is_empty() {
        title = format!("{title} {}", parts.join(", "));
    }
    if let Some(active) = active {
        title = if parts.is_empty() {
            format!("{}: shows {active} macro{}", device.named(), if active == 1 { "" } else { "s" })
        } else {
            format!("{title}; shows {active}")
        };
    }
    Ok(Edited { document, title, note: None, prior: Value::Object(prior), applied: Value::Object(applied) })
}

/// Writes a macro's fields that `values` names (name, value, mappings, activeMacros) into the container.
fn write_macro(container: &mut Element, kind: &str, values: &Value) {
    let m = values["macro"].as_i64().unwrap_or(1);
    let tag = format!("Macro{}", m - 1);
    if container.child(&tag).is_none() {
        // (Renoise lists every macro; one left out is put where Renoise puts it, before NumActiveMacros)
        let at = position_of(container, "NumActiveMacros").unwrap_or(container.element_count());
        container.insert_element(at, Element::new(&tag).with_child(Element::new("Value").with_text("50")));
    }
    if let Some(active) = values.get("activeMacros").and_then(Value::as_i64) {
        container.set_child_text("NumActiveMacros", &active.to_string());
    }
    let macro_ = container.child_mut(&tag).expect("made above");
    if let Some(name) = values.get("name").and_then(Value::as_str) {
        macro_.set_child_text("Name", name);
    }
    if let Some(value) = values.get("value").and_then(Value::as_f64) {
        macro_.set_child_text("Value", &trimmed(value));
    }
    if let Some(mappings) = values.get("mappings").and_then(Value::as_array) {
        if let Some(at) = position_of(macro_, "Mappings") {
            macro_.remove_element(at);
        }
        if !mappings.is_empty() {
            let mut list = Element::new("Mappings");
            for mapping in mappings {
                list = list.with_child(mapping_element(mapping));
            }
            let count = macro_.element_count();
            macro_.insert_element(count, list);
        }
    }
    let _ = kind;
}

/// A number as text without a needless ".0" (75, 12.5).
fn trimmed(value: f64) -> String {
    let text = format!("{}", (value * 1000.0).round() / 1000.0);
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

// doofer-device-add, -delete, -move

fn default_end(slot_: &Element, top: &str) -> Vec<i64> {
    let count = devices_in(slot_, top, 0).len() as i64;
    if top == SPLITTER {
        vec![1, count + 1]
    } else {
        vec![count + 1]
    }
}

fn device_add(mut document: xml::Document, top: &str, input: &Value, device: &Device) -> Result<Edited, Refusal> {
    let asked = text_field(input, "path")?.filter(|p| !p.is_empty()).ok_or_else(|| Refusal::Bad("path: a device's path or name".into()))?;
    let mut element = new_device(asked, device)?;
    if let Some(name) = text_field(input, "name")?.filter(|n| !n.is_empty()) {
        if name.chars().count() > 256 || name.chars().any(char::is_control) {
            return bad("name must be one line of text");
        }
        element = element.with_child(Element::new("CustomDeviceName").with_text(name));
    }
    let at = int_list(input.get("at"), "at")?.unwrap_or_else(|| default_end(slot(&document).expect("checked"), top));
    let locus = resolve(slot(&document)?, &at, true)?;
    let label = inner_label(&element);
    let before = chain_print(slot(&document)?, &locus.container, locus.lane);
    let where_ = place_words(&at, slot(&document)?);
    insert_at(slot_mut(&mut document), &locus, element);
    let after = chain_print(slot(&document)?, &locus.container, locus.lane);
    let title = format!("Added {} to {} ({where_})", q(&label), q(&device.named()));
    Ok(Edited {
        document,
        title,
        note: None,
        prior: json!({"at": at, "chain": before}),
        applied: json!({"at": at, "chain": after, "type": label}),
    })
}

/// Puts a device element at a place, the mappings and meta devices of that lane following it.
fn insert_at(slot_: &mut Element, locus: &Locus, element: Element) {
    let list = list_mut(slot_, &locus.container, locus.lane);
    list.insert_element(locus.place, element);
    let container = element_at_mut(slot_, &locus.container).expect("resolved");
    let kind = type_of(container, locus.container.is_empty()).to_string();
    let place = locus.place;
    remap(container, &kind, locus.lane, &|n| Some(if n >= place { n + 1 } else { n }));
    // (the others' aims make room; the device just put in aims as it did, in the lane's numbering with it: a deleted
    // meta device put back by Kumi's undo, one added with its aim)
    reaim_skipping(container, &kind, locus.lane, &|n| Some(if n >= place { n + 1 } else { n }), Some(place));
}

/// Takes the device at a place out: the mappings to it go, the meta devices aimed at it are cleared, the others follow.
fn take_out(slot_: &mut Element, locus: &Locus) -> (Element, Vec<Value>, Vec<Value>) {
    let list = list_mut(slot_, &locus.container, locus.lane);
    let element = list.remove_element(locus.place).expect("resolved");
    let container = element_at_mut(slot_, &locus.container).expect("resolved");
    let kind = type_of(container, locus.container.is_empty()).to_string();
    let place = locus.place;
    let follow = |n: usize| {
        if n == place {
            None
        } else if n > place {
            Some(n - 1)
        } else {
            Some(n)
        }
    };
    let gone = remap(container, &kind, locus.lane, &follow);
    let cleared = reaim(container, &kind, locus.lane, &follow);
    (element, gone, cleared)
}

fn device_delete(mut document: xml::Document, input: &Value, device: &Device) -> Result<Edited, Refusal> {
    let at = int_list(input.get("at"), "at")?.ok_or_else(|| Refusal::Bad("at: the device's place, from 1".into()))?;
    let locus = resolve(slot(&document)?, &at, false)?;
    let where_ = place_words(&at, slot(&document)?);
    let before = chain_print(slot(&document)?, &locus.container, locus.lane);
    let (element, gone, cleared) = take_out(slot_mut(&mut document), &locus);
    let label = inner_label(&element);
    let after = chain_print(slot(&document)?, &locus.container, locus.lane);
    let mut notes = vec![];
    if !gone.is_empty() {
        notes.push(format!("{} macro mapping{} to it went with it.", gone.len(), if gone.len() == 1 { "" } else { "s" }));
    }
    if !cleared.is_empty() {
        notes.push(format!("{} meta device{} aimed at it now aim at nothing.", cleared.len(), if cleared.len() == 1 { "" } else { "s" }));
    }
    notes.push("Kumi keeps it to put back.".into());
    Ok(Edited {
        document,
        title: format!("Took {} out of {} ({where_})", q(&label), q(&device.named())),
        note: Some(notes.join(" ")),
        prior: json!({"at": at, "chain": before, "xml": element.to_xml(), "mappings": gone, "aims": cleared, "type": label}),
        applied: json!({"at": at, "chain": after}),
    })
}

/// Whether anything in the container (a macro, a meta device beside it) aims at the device at a place, or the device
/// itself aims somewhere (a meta device): such a device keeps its routes only inside its own container.
fn routed(slot_: &Element, locus: &Locus) -> bool {
    let container = element_at(slot_, &locus.container).expect("resolved");
    let kind = type_of(container, locus.container.is_empty());
    let mut found = false;
    for (_, at) in macros_of(container, kind) {
        let macro_ = container.element(at).expect("in range");
        for mapping in macro_.child("Mappings").map(|list| list.elements().collect::<Vec<_>>()).unwrap_or_default() {
            let chain = number(mapping.child("DestChainIndex")).unwrap_or(0.0) as i64;
            let device = number(mapping.child("DestDeviceIndex")).unwrap_or(-1.0) as i64;
            found |= chain == locus.lane as i64 && device == locus.place as i64;
        }
    }
    for (d, device) in devices_in(container, kind, locus.lane).into_iter().enumerate() {
        for prefix in destination_prefixes(device) {
            let effect = value_of(device, &format!("{prefix}DestEffect")).unwrap_or(-1.0);
            found |= effect as i64 == locus.place as i64 || (d == locus.place && effect >= 0.0);
        }
    }
    found
}

fn device_move(mut document: xml::Document, input: &Value, device: &Device) -> Result<Edited, Refusal> {
    let at = int_list(input.get("at"), "at")?.ok_or_else(|| Refusal::Bad("at: the device's place, from 1".into()))?;
    let to = int_list(input.get("to"), "to")?.ok_or_else(|| Refusal::Bad("to: where it goes, as places from 1".into()))?;
    let from = resolve(slot(&document)?, &at, false)?;
    let label = inner_label(
        element_at(slot(&document)?, &from.container)
            .map(|c| {
                let kind = type_of(c, from.container.is_empty());
                devices_in(c, kind, from.lane)[from.place]
            })
            .expect("resolved"),
    );
    let before = json!({"from": chain_print(slot(&document)?, &from.container, from.lane)});
    // Where it lands is read with the device out of its place (as the producer's drag does).
    let mut probe = document.clone();
    let (element, _, _) = take_out(slot_mut(&mut probe), &from);
    let target = resolve(slot(&probe)?, &to, true)?;
    // (where it lands, in words, read the same way: with it out of its place)
    let where_ = place_words(&to, slot(&probe)?);
    if let Some(why) = move_refusal(slot(&document)?, &from, &target, &label) {
        return bad(why);
    }
    if target.container == from.container && target.lane == from.lane && target.place == from.place {
        return Ok(Edited {
            document,
            title: format!("{} stays at its place in {}", q(&label), q(&device.named())),
            note: None,
            prior: json!({"at": at, "to": to, "chain": before}),
            applied: json!({"at": at, "to": to}),
        });
    }
    move_device(slot_mut(&mut document), &from, &target, element);
    let to_print = chain_print(slot(&document)?, &target.container, target.lane);
    // (the source lane as the move left it: the same lane, or as it is with the device out, whose path an insert before it
    // in the top lane would move)
    let from_print = if target.container == from.container && target.lane == from.lane {
        to_print.clone()
    } else {
        chain_print(slot(&probe)?, &from.container, from.lane)
    };
    let after = json!({"to": to_print, "from": from_print});
    Ok(Edited {
        document,
        title: format!("Moved {} in {} to {where_}", q(&label), q(&device.named())),
        note: None,
        prior: json!({"at": at, "to": to, "chain": before}),
        applied: json!({"at": at, "to": to, "chain": after}),
    })
}

/// Why the device at `from` can't go to `target` (found with it out of its place): something aims at it (a macro, a meta
/// device beside it) or it aims somewhere, and it would leave its own container; or meta devices aim at it, or it at
/// others, and it would change lanes (they don't aim across lanes). Macro mappings follow a device within its container.
fn move_refusal(slot_: &Element, from: &Locus, target: &Locus, label: &str) -> Option<String> {
    if target.container != from.container && routed(slot_, from) {
        return Some(format!(
            "macros or meta devices aim at {} (or it aims at others): it keeps them only inside its own Doofer or Splitter",
            q(label)
        ));
    }
    if target.container == from.container && target.lane != from.lane && routed(slot_, from) {
        let container = element_at(slot_, &from.container).expect("resolved");
        let kind = type_of(container, from.container.is_empty());
        let aimed = devices_in(container, kind, from.lane).iter().enumerate().any(|(d, device)| {
            destination_prefixes(device).iter().any(|prefix| {
                let effect = value_of(device, &format!("{prefix}DestEffect")).unwrap_or(-1.0);
                effect as i64 == from.place as i64 || (d == from.place && effect >= 0.0)
            })
        });
        if aimed {
            return Some(format!("meta devices aim at {} (or it aims at others), and they don't aim across lanes", q(label)));
        }
    }
    None
}

/// Moves the device from one place to another (in its container: its mappings follow; to another container: nothing aims
/// at it, as checked, and what aims at the others in both lanes follows them).
fn move_device(slot_: &mut Element, from: &Locus, to: &Locus, element: Element) {
    let list = list_mut(slot_, &from.container, from.lane);
    list.remove_element(from.place);
    let (a, b) = (from.place, to.place);
    if from.container != to.container {
        // Into or out of a Doofer inside: the mappings and meta devices aimed at the devices after it in its lane close up,
        // and those aimed at the devices at or after its new place make room. (The source lane first: the new place may be
        // in a list that holds the source's container, which the insert would move.)
        let source = element_at_mut(slot_, &from.container).expect("resolved");
        let kind = type_of(source, from.container.is_empty()).to_string();
        let close = move |n: usize| Some(if n > a { n - 1 } else { n });
        remap(source, &kind, from.lane, &close);
        reaim(source, &kind, from.lane, &close);
        let list = list_mut(slot_, &to.container, to.lane);
        list.insert_element(to.place, element);
        let target = element_at_mut(slot_, &to.container).expect("resolved");
        let kind = type_of(target, to.container.is_empty()).to_string();
        let open = move |n: usize| Some(if n >= b { n + 1 } else { n });
        remap(target, &kind, to.lane, &open);
        reaim(target, &kind, to.lane, &open);
        return;
    }
    let list = list_mut(slot_, &to.container, to.lane);
    list.insert_element(to.place, element);
    let container = element_at_mut(slot_, &from.container).expect("resolved");
    let kind = type_of(container, from.container.is_empty()).to_string();
    if from.lane == to.lane {
        let follow = move |n: usize| {
            Some(if n == a {
                b
            } else if a < b && n > a && n <= b {
                n - 1
            } else if b < a && n >= b && n < a {
                n + 1
            } else {
                n
            })
        };
        remap(container, &kind, from.lane, &follow);
        reaim(container, &kind, from.lane, &follow);
        return;
    }
    // Across a Splitter's lanes: the device's mappings change lane; the others in each lane close up or make room.
    for (_, at) in macros_of(container, &kind) {
        let Some(mappings) = container.element_mut(at).and_then(|macro_| macro_.child_mut("Mappings")) else { continue };
        for mapping in mappings.elements_mut() {
            let chain = number(mapping.child("DestChainIndex")).unwrap_or(0.0) as usize;
            let device = number(mapping.child("DestDeviceIndex")).unwrap_or(-1.0);
            if device < 0.0 {
                continue;
            }
            let device = device as usize;
            let (lane, n) = if chain == from.lane && device == a {
                (to.lane, b)
            } else if chain == from.lane && device > a {
                (chain, device - 1)
            } else if chain == to.lane && device >= b {
                (chain, device + 1)
            } else {
                (chain, device)
            };
            if lane != chain {
                mapping.set_child_text("DestChainIndex", &lane.to_string());
            }
            if n != device {
                mapping.set_child_text("DestDeviceIndex", &n.to_string());
            }
        }
    }
    reaim(container, &kind, from.lane, &|n| Some(if n > a { n - 1 } else { n }));
    reaim(container, &kind, to.lane, &|n| Some(if n >= b { n + 1 } else { n }));
}

// doofer-device-set

fn device_set(mut document: xml::Document, input: &Value, device: &Device) -> Result<Edited, Refusal> {
    let at = int_list(input.get("at"), "at")?.ok_or_else(|| Refusal::Bad("at: the device's place, from 1".into()))?;
    let locus = resolve(slot(&document)?, &at, false)?;
    let chain = chain_print(slot(&document)?, &locus.container, locus.lane);
    let container = element_at(slot(&document)?, &locus.container).expect("resolved");
    let kind = type_of(container, locus.container.is_empty()).to_string();
    let target = devices_in(container, &kind, locus.lane)[locus.place];
    let label = inner_label(target);
    let mut prior = Map::new();
    let mut applied = Map::new();
    let mut parts = vec![];
    let mut notes = vec![];
    let mut writes: Vec<(String, f64)> = vec![];
    if input.get("param").is_some_and(|p| !p.is_null()) {
        if target.name() == PLUGIN {
            return bad(format!("the plug-in {}'s parameters are in its own state: set them in its editor", q(&label)));
        }
        let k = parameter_number(target, &input["param"], false)?;
        let Some((element, min, max)) = parameter_spec(target, k) else { return bad("Kumi doesn't know that parameter") };
        if element.is_empty() {
            return bad(format!(
                "Renoise doesn't save {} of {} (a trigger), so it can't be set here",
                q(&parameter_name(target, k)),
                q(&label)
            ));
        }
        let value = match (input.get("value").and_then(Value::as_f64), input.get("valueString").and_then(Value::as_str)) {
            (Some(v), _) => v,
            (None, Some(text)) => {
                let (value, approximate) = value_from_text(target, k, text)?;
                if approximate {
                    notes.push(format!("Kumi read {text:?} on Renoise's curve between measured points: it may land a little off."));
                }
                value
            }
            (None, None) => return bad("give value (in the parameter's own range) or valueString"),
        };
        if !(min..=max).contains(&value) || !value.is_finite() {
            return bad(format!(
                "{} of {} is from {} to {} (Renoise doesn't check what Kumi writes there)",
                q(&parameter_name(target, k)),
                q(&label),
                trimmed(min),
                trimmed(max)
            ));
        }
        if let Some(problem) = aim_problem(container, &kind, locus.lane, locus.place, target, &element, value) {
            return bad(problem);
        }
        let now = value_of(target, &element);
        prior.insert("param".into(), json!({"k": k, "element": element, "value": now}));
        applied.insert("param".into(), json!({"k": k, "element": element, "value": value}));
        parts.push(format!("{} {}", parameter_name(target, k), trimmed(value)));
        let driven = drivers(container, &kind, locus.lane, locus.place, k);
        if !driven.is_empty() {
            notes.push(format!(
                "{} drive{} it, and set{} it again as {} move{}.",
                driven.join(" and "),
                if driven.len() == 1 { "s" } else { "" },
                if driven.len() == 1 { "s" } else { "" },
                if driven.len() == 1 { "it" } else { "they" },
                if driven.len() == 1 { "s" } else { "" }
            ));
        }
        writes.push((element, value));
    }
    if let Some(bypass) = input.get("bypass").filter(|b| !b.is_null()) {
        let Some(bypass) = bypass.as_bool() else { return bad("bypass must be true or false") };
        let now = value_of(target, "IsActive").map(|v| v < 0.5);
        prior.insert("bypass".into(), now.map_or(Value::Null, Value::from));
        applied.insert("bypass".into(), bypass.into());
        parts.push(if bypass { "bypassed".into() } else { "switched on".into() });
        writes.push(("IsActive".into(), if bypass { 0.0 } else { 1.0 }));
    }
    let name = text_field(input, "name")?;
    if let Some(name) = name {
        if name.chars().count() > 256 || name.chars().any(char::is_control) {
            return bad("name must be one line of text");
        }
        prior.insert("name".into(), target.child_text("CustomDeviceName").unwrap_or_default().into());
        applied.insert("name".into(), name.into());
        parts.push(if name.is_empty() { "its own name again".into() } else { format!("renamed {}", q(name)) });
    }
    if parts.is_empty() {
        return bad("give param with value or valueString, bypass, or name");
    }
    let target_type = target.name().to_string();
    let element = target_mut(slot_mut(&mut document), &locus);
    for (name, value) in writes {
        set_value(element, &name, value);
    }
    if let Some(name) = name {
        set_name(element, name);
    }
    prior.insert("at".into(), json!(at));
    // (the lane as previewed: a device dropped in before the apply makes the place another device's)
    prior.insert("chain".into(), chain.clone());
    applied.insert("at".into(), json!(at));
    applied.insert("chain".into(), chain);
    applied.insert("type".into(), target_type.into());
    Ok(Edited {
        document,
        title: format!("{} in {}: {}", label, device.named(), parts.join(", ")),
        note: (!notes.is_empty()).then(|| notes.join(" ")),
        prior: Value::Object(prior),
        applied: Value::Object(applied),
    })
}

/// What's wrong with aiming a meta device inside a container (its Dest. Track, Effect or Parameter, or *Hydra's OutN ones)
/// at `value`, which Renoise doesn't check: inside, it aims along its own lane (its track is its own: -1), at another device
/// there (from 0, as Renoise keeps it) or none (-1), and at a parameter that device has (0 is its on/off).
fn aim_problem(container: &Element, kind: &str, lane: usize, place: usize, device: &Element, element: &str, value: f64) -> Option<String> {
    let label = q(&inner_label(device));
    if element.ends_with("DestTrack") {
        let inside = if kind == SPLITTER { "Splitter" } else { "Doofer" };
        return (value != -1.0).then(|| format!("{label} aims inside its {inside} only: its track is its own (-1)"));
    }
    let (prefix, effect_asked) = match (element.strip_suffix("DestEffect"), element.strip_suffix("DestParameter")) {
        (Some(prefix), _) => (prefix, true),
        (_, Some(prefix)) => (prefix, false),
        _ => return None,
    };
    if value == -1.0 {
        return None;
    }
    let devices = devices_in(container, kind, lane);
    if effect_asked && (value.fract() != 0.0 || value < 0.0 || value as usize >= devices.len() || value as usize == place) {
        return Some(format!(
            "{label} aims at another device of its chain: from 0 to {} but {place} (itself), or -1 for none",
            devices.len().saturating_sub(1)
        ));
    }
    let effect = if effect_asked { value } else { value_of(device, &format!("{prefix}DestEffect")).unwrap_or(-1.0) };
    if effect < 0.0 {
        return Some(format!("{label} aims at no device yet: set its Dest. Effect first (or -1 for none)"));
    }
    let parameter = if effect_asked { value_of(device, &format!("{prefix}DestParameter")).unwrap_or(-1.0) } else { value };
    let target = devices.get(effect as usize)?;
    match parameter_count(target) {
        Ok(count) if parameter < 0.0 || (parameter.fract() == 0.0 && parameter as usize <= count) => None,
        Ok(count) if effect_asked => Some(format!(
            "{label}'s Dest. Parameter ({}) is past what {} has (0 for its on/off to {count}): set it first, or -1",
            trimmed(parameter),
            q(&inner_label(target))
        )),
        Ok(count) => Some(format!("{label} aims at a parameter of {} from 0 (its on/off) to {count}", q(&inner_label(target)))),
        // (a plug-in's parameters Kumi can't count: Renoise takes it as asked)
        Err(_) => None,
    }
}

fn target_mut<'a>(slot_: &'a mut Element, locus: &Locus) -> &'a mut Element {
    list_mut(slot_, &locus.container, locus.lane).element_mut(locus.place).expect("resolved")
}

fn set_name(device: &mut Element, name: &str) {
    if name.is_empty() {
        if let Some(at) = position_of(device, "CustomDeviceName") {
            device.remove_element(at);
        }
        return;
    }
    if device.child("CustomDeviceName").is_none() {
        let at = position_of(device, "SelectedPresetIsModified").map_or(0, |at| at + 1);
        device.insert_element(at, Element::new("CustomDeviceName").with_text(name));
    } else {
        device.set_child_text("CustomDeviceName", name);
    }
}

/// A parameter's value from text as Renoise shows it ("-6 dB", "Saw", "800 Hz"), read with Renoise's own kinds of text
/// (texts.rs), and whether it may land a little off (a curve read between measured points).
fn value_from_text(target: &Element, k: usize, text: &str) -> Result<(f64, bool), Refusal> {
    let label = inner_label(target);
    let name = parameter_name(target, k);
    let spec = devices::by_xml(target.name()).and_then(|device| device.params.get(k - 1));
    let Some(spec) = spec.filter(|spec| spec.text != devices::Text::None) else {
        return bad(format!("give {} of {} as value (in its own range): Kumi can't read its text", q(&name), q(&label)));
    };
    match spec.text.value(text, spec.min) {
        Some(value) => Ok((value, spec.text.approximate())),
        None => bad(format!("{} of {} doesn't read {text:?} (it shows {})", q(&name), q(&label), spec.text.example(spec.min, spec.max))),
    }
}

// splitter-set

const SPLITS: [(&str, &str); 3] = [("parallel", "Parallel"), ("mid-side", "Mid/Side"), ("frequency", "Frequency")];

fn splitter_set(mut document: xml::Document, input: &Value, device: &Device) -> Result<Edited, Refusal> {
    let asked = text_field(input, "type")?.unwrap_or_default();
    let Some((word, xml_word)) = SPLITS.iter().find(|(w, _)| *w == asked) else { return bad("type is parallel, mid-side or frequency") };
    let now = slot(&document)?.child_text("Type").unwrap_or_else(|| "Parallel".into());
    let now_word = SPLITS.iter().find(|(_, x)| **x == now).map_or("parallel", |(w, _)| w);
    slot_mut(&mut document).set_child_text("Type", xml_word);
    let how = match *word {
        "parallel" => "in parallel",
        "mid-side" => "into mid and side",
        _ => "by frequency",
    };
    Ok(Edited {
        document,
        title: format!("{} splits {how}", device.named()),
        note: None,
        prior: json!({"type": now_word}),
        applied: json!({"type": word}),
    })
}

// notepad-write

/// A Notepad's lines; one empty paragraph is an empty Notepad (how Kumi clears one: see `write_paragraphs`).
fn paragraphs_of(element: &Element) -> Vec<String> {
    let lines: Vec<String> =
        element.child("Paragraphs").map(|list| list.elements().filter_map(Element::text).collect()).unwrap_or_default();
    if lines.len() == 1 && lines[0].is_empty() {
        return vec![];
    }
    lines
}

fn notepad_write(mut document: xml::Document, top: &str, input: &Value, device: &Device) -> Result<Edited, Refusal> {
    let text = text_field(input, "text")?.ok_or_else(|| Refusal::Bad("text: what the Notepad says".into()))?;
    if text.chars().count() > 16_384 {
        return bad("text: at most 16,384 letters");
    }
    let lines: Vec<String> =
        if text.is_empty() { vec![] } else { text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect() };
    // (XML 1.0 takes no control characters but tab and the line breaks, and what Renoise does with them is unmeasured)
    if lines.iter().any(|line| line.chars().any(|c| c.is_control() && c != '\t')) {
        return bad("text: no control characters but tabs and new lines");
    }
    let at = int_list(input.get("at"), "at")?;
    let (locus, title) = match &at {
        None if top == NOTEPAD => (None, format!("Wrote the Notepad on {}", device.track)),
        None => return bad(format!("{} isn't a Notepad: give at, the Notepad's place inside it", q(&device.named()))),
        Some(_) if !is_container(top) => return bad("at is for a Notepad inside a Doofer or Splitter"),
        Some(at) => {
            let locus = resolve(slot(&document)?, at, false)?;
            let container = element_at(slot(&document)?, &locus.container).expect("resolved");
            let kind = type_of(container, locus.container.is_empty());
            let target = devices_in(container, kind, locus.lane)[locus.place];
            if target.name() != NOTEPAD {
                return bad(format!("the device at that place is {}, not a Notepad", q(&inner_label(target))));
            }
            (Some(locus), format!("Wrote the Notepad in {}", device.named()))
        }
    };
    let element = match &locus {
        None => slot_mut(&mut document),
        Some(locus) => target_mut(slot_mut(&mut document), locus),
    };
    let before = paragraphs_of(element);
    write_paragraphs(element, &lines);
    Ok(Edited {
        document,
        title,
        note: None,
        prior: json!({"at": at, "paragraphs": before}),
        applied: json!({"at": at, "paragraphs": lines}),
    })
}

/// The Notepad's lines written. No lines are one empty paragraph: Renoise 3.5.4 keeps the text it had when the preset data
/// has no Paragraphs, or an empty one (lab, 2026-10-20), so leaving them out never cleared a Notepad.
fn write_paragraphs(element: &mut Element, lines: &[String]) {
    if let Some(at) = position_of(element, "Paragraphs") {
        element.remove_element(at);
    }
    let mut list = Element::new("Paragraphs");
    if lines.is_empty() {
        list = list.with_child(Element::new("Paragraph"));
    }
    for line in lines {
        list = list.with_child(Element::new("Paragraph").with_text(line));
    }
    let count = element.element_count();
    element.insert_element(count, list);
}

// doofer-make: a Doofer or Splitter made of devices on a track (the multi-device sound): its preset data holds them in
// order (a Splitter's second lane too), meta devices aimed at each other inside, and the macros asked; the tool puts it in
// and takes the devices out in one step.

const DOOFER_DEFAULT: &str = include_str!("../../tests/fixtures/renoise/doofer_default.xml");
const SPLITTER_DEFAULT: &str = include_str!("../../tests/fixtures/renoise/splitter_default.xml");

/// A device's place from its ref (`E:device:t.d`): its track and its place on it.
fn device_numbers(reference: &str) -> Option<(i64, i64)> {
    let rest = reference.rsplit_once(":device:")?.1;
    let (t, d) = rest.split_once('.')?;
    Some((t.parse().ok()?, d.parse().ok()?))
}

/// A device as it goes inside a container, from what preset_read gave of it: its preset slot's settings under its type's
/// element, its name and bypass, and a meta device's destinations as places in its new chain (`chain`: the moved devices'
/// places on the track, in the chain's order).
fn inner_from(read: &Value, own_track: i64, chain: &[(i64, i64)]) -> Result<Element, Refusal> {
    let label = read.get("label").and_then(Value::as_str).unwrap_or("the device").to_string();
    let text = read.get("presetData").and_then(Value::as_str).unwrap_or_default();
    let document = xml::parse(text).map_err(|e| Refusal::Bad(format!("the preset data of {} can't be read: {e}", q(&label))))?;
    let slot_ = slot(&document)?;
    let kind = type_of(slot_, true).to_string();
    if let Some(native) = devices::by_xml(&kind) {
        if !native.nests {
            return bad(format!("{} can't go inside a Doofer or Splitter (Renoise crashes on routing devices there)", q(&label)));
        }
    } else if kind != PLUGIN {
        return bad(format!("Kumi doesn't know the device {kind} of {}", q(&label)));
    }
    let mut element = Element::new(&kind).with_attribute("type", &kind);
    let name = read.get("name").and_then(Value::as_str).unwrap_or_default();
    if !label.is_empty() && label != name {
        element = element.with_child(Element::new("CustomDeviceName").with_text(&label));
    }
    let active = read.get("active").and_then(Value::as_bool).unwrap_or(true);
    element = element.with_child(Element::new("IsActive").with_child(Element::new("Value").with_text(if active { "1.0" } else { "0.0" })));
    for child in slot_.elements() {
        element = element.with_child(child.clone());
    }
    // A meta device's destinations: kept only when they aim at a device going into the same chain.
    let params: Vec<&Value> = read.get("parameters").and_then(Value::as_array).map(|list| list.iter().collect()).unwrap_or_default();
    let value = |name: &str| {
        params.iter().find(|p| p.get("name").and_then(Value::as_str) == Some(name)).and_then(|p| p.get("value")).and_then(Value::as_f64)
    };
    let prefixes: Vec<String> = params
        .iter()
        .filter_map(|p| p.get("name").and_then(Value::as_str)?.strip_suffix("Track").map(str::to_owned))
        .filter(|prefix| value(&format!("{prefix}Effect")).is_some())
        .collect();
    for prefix in prefixes {
        let track = value(&format!("{prefix}Track")).unwrap_or(-1.0);
        if track < 0.0 {
            continue;
        }
        let effect = value(&format!("{prefix}Effect")).unwrap_or(-1.0);
        let place = chain.iter().position(|&(t, d)| t == track as i64 + 1 && d == effect as i64 + 1);
        let Some(place) = place.filter(|_| track as i64 + 1 == own_track) else {
            return bad(format!(
                "{} aims at a device that stays outside (or in the other lane): a meta device inside a Doofer or Splitter drives only devices in its own chain",
                q(&label)
            ));
        };
        let xml_prefix = if prefix == "Dest. " { String::new() } else { prefix.trim_end().to_string() };
        set_value(&mut element, &format!("{xml_prefix}DestTrack"), -1.0);
        set_value(&mut element, &format!("{xml_prefix}DestEffect"), place as f64);
        if let Some(parameter) = value(&format!("{prefix}Parameter")) {
            set_value(&mut element, &format!("{xml_prefix}DestParameter"), parameter);
        }
    }
    Ok(element)
}

/// The container doofer-make makes, from the reads of the devices going into it (`lanes`: one list a Doofer, two a
/// Splitter), and its words. `prior` names the devices and where they were.
pub fn make(input: &Value, lanes: &[Vec<(String, Value)>], track_name: &str) -> Result<Edited, Refusal> {
    let kind = match input.get("type").and_then(Value::as_str).unwrap_or("doofer") {
        "doofer" => DOOFER,
        "splitter" => SPLITTER,
        _ => return bad("type is doofer or splitter"),
    };
    if kind == DOOFER && lanes.get(1).is_some_and(|lane| !lane.is_empty()) {
        return bad("laneRefs is a Splitter's second lane");
    }
    let mut document = xml::parse(if kind == SPLITTER { SPLITTER_DEFAULT } else { DOOFER_DEFAULT }).expect("Renoise's own default");
    let own_track = lanes.iter().flatten().find_map(|(reference, _)| device_numbers(reference)).map_or(0, |(t, _)| t);
    let mut names = vec![];
    for (lane, list) in lanes.iter().enumerate() {
        let chain: Vec<(i64, i64)> = list.iter().filter_map(|(reference, _)| device_numbers(reference)).collect();
        for (place, (_, read)) in list.iter().enumerate() {
            let element = inner_from(read, own_track, &chain)?;
            names.push(inner_label(&element));
            let locus = Locus { container: vec![], lane, place, asked: vec![] };
            list_mut(slot_mut(&mut document), &locus.container, lane).insert_element(locus.place, element);
        }
    }
    if kind == SPLITTER {
        if let Some(split) = input.get("split").and_then(Value::as_str) {
            let Some((_, xml_word)) = SPLITS.iter().find(|(w, _)| *w == split) else {
                return bad("split is parallel, mid-side or frequency");
            };
            slot_mut(&mut document).set_child_text("Type", xml_word);
        }
    }
    // The macros asked, as doofer_macro takes them, numbered in order unless they say which.
    for (i, asked) in input.get("macros").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let m = asked.get("macro").and_then(Value::as_i64).unwrap_or(i as i64 + 1);
        if m < 1 || m as usize > macro_count(kind) {
            return bad(format!(
                "macros: a {} has macros 1 to {}",
                if kind == SPLITTER { "Splitter" } else { "Doofer" },
                macro_count(kind)
            ));
        }
        let mut values = Map::new();
        values.insert("macro".into(), m.into());
        if let Some(name) = asked.get("name").and_then(Value::as_str) {
            if name.chars().count() > 64 || name.chars().any(char::is_control) {
                return bad("a macro's name is one line of at most 64 letters");
            }
            values.insert("name".into(), name.into());
        }
        if let Some(value) = asked.get("value").and_then(Value::as_f64) {
            if !(0.0..=100.0).contains(&value) {
                return bad("a macro's value is from 0 to 100 (%)");
            }
            values.insert("value".into(), value.into());
        }
        if let Some(mappings) = asked.get("mappings") {
            values.insert("mappings".into(), Value::Array(checked_mappings(slot(&document)?, kind, mappings)?));
        }
        write_macro(slot_mut(&mut document), kind, &Value::Object(values));
    }
    if let Some(active) = input.get("activeMacros").and_then(Value::as_i64) {
        if !(0..=macro_count(kind) as i64).contains(&active) {
            return bad(format!("activeMacros is from 0 to {}", macro_count(kind)));
        }
        slot_mut(&mut document).set_child_text("NumActiveMacros", &active.to_string());
    }
    let word = if kind == SPLITTER { "Splitter" } else { "Doofer" };
    let listed = match names.len() {
        0 => return bad("deviceRefs: the devices to put in it"),
        1 => q(&names[0]),
        2 => format!("{} and {}", q(&names[0]), q(&names[1])),
        n => format!("{}, {} and {} more", q(&names[0]), q(&names[1]), n - 2),
    };
    let title = format!("Made a {word} of {listed} on {}", q(track_name));
    Ok(Edited { document, title, note: None, prior: json!({"devices": names}), applied: json!({"type": word}) })
}

// Kumi's undo: the edit taken back from what is there now.

/// What a record keeps of an edit, to take it back: the kind, what was there (`prior`), what the edit left (`applied`),
/// and the device's shape right after the edit (`shape`).
pub struct Kept<'a> {
    pub kind: &'a str,
    pub prior: &'a Value,
    pub applied: &'a Value,
    pub shape: &'a str,
    pub discard: bool,
}

/// What an undo does: write this document, or nothing (it is back already).
#[derive(Debug)]
pub enum Undo {
    Write { document: Box<xml::Document>, title: String, note: Option<String> },
    Back(String),
}

/// The edit taken back from the device's preset data now (`text`, whose shape is `shape_now`).
pub fn undo(kept: &Kept, text: &str, shape_now: &str, device: &Device) -> Result<Undo, Refusal> {
    let mut document =
        xml::parse(text).map_err(|e| Refusal::Bad(format!("the preset data of {} can't be read: {e}", q(&device.named()))))?;
    let structural = matches!(kept.kind, "doofer-device-add" | "doofer-device-delete" | "doofer-device-move");
    if structural && shape_now != kept.shape {
        return changed(format!(
            "the devices or mappings in {} changed in Renoise since Kumi changed them, so it can't tell what to take back",
            q(&device.named())
        ));
    }
    let put_back = format!("{} put back", device.named());
    match kept.kind {
        "doofer-device-add" => {
            let at = list_of(&kept.applied["at"]);
            let locus = resolve(slot(&document)?, &at, false)?;
            let (label, content, aimed) = {
                let container = element_at(slot(&document)?, &locus.container).expect("resolved");
                let kind = type_of(container, locus.container.is_empty());
                let devices = devices_in(container, kind, locus.lane);
                let added = devices[locus.place];
                // (the meta devices beside it that aim at it now: their aims go with it)
                let aimed: Vec<String> = devices
                    .iter()
                    .enumerate()
                    .filter(|(d, device)| {
                        *d != locus.place
                            && destination_prefixes(device)
                                .iter()
                                .any(|prefix| value_of(device, &format!("{prefix}DestEffect")) == Some(locus.place as f64))
                    })
                    .map(|(_, device)| q(&inner_label(device)))
                    .collect();
                (inner_label(added), content_of(added), aimed)
            };
            // What the producer did with it since Kumi added it (its knobs, its switch, a plug-in's state) is theirs, and so
            // are the aims they gave meta devices at it: Kumi takes it out only on discard, and says what goes with it.
            let turned = kept.applied.get("content").and_then(Value::as_str).is_some_and(|added| added != content);
            // (what Renoise wrote for it couldn't be read back after the add: what the producer did since can't be told)
            let unknown = kept.applied.get("contentUnknown") == Some(&Value::Bool(true));
            if (turned || unknown || !aimed.is_empty()) && !kept.discard {
                let mut why = vec![];
                if turned {
                    why.push(format!("{} was changed in Renoise since Kumi added it", q(&label)));
                }
                if unknown {
                    why.push(format!(
                        "Kumi couldn't read what {} held right after adding it, so it can't tell whether it was changed since",
                        q(&label)
                    ));
                }
                if !aimed.is_empty() {
                    why.push(format!("{} aim{} at it now", aimed.join(" and "), if aimed.len() == 1 { "s" } else { "" }));
                }
                return changed(format!("{}, so Kumi leaves it (undo with discard to take it out anyway)", why.join(", and ")));
            }
            take_out(slot_mut(&mut document), &locus);
            let note = (!aimed.is_empty())
                .then(|| format!("{} aim{} at nothing now.", aimed.join(" and "), if aimed.len() == 1 { "s" } else { "" }));
            Ok(Undo::Write { document: Box::new(document), title: put_back, note })
        }
        "doofer-device-delete" => {
            let at = list_of(&kept.prior["at"]);
            let locus = resolve(slot(&document)?, &at, true)?;
            let text = kept.prior["xml"].as_str().unwrap_or_default();
            let element = xml::parse(text).map_err(|e| Refusal::Bad(format!("the device Kumi kept can't be read: {e}")))?.root;
            insert_at(slot_mut(&mut document), &locus, element);
            let container = element_at_mut(slot_mut(&mut document), &locus.container).expect("resolved");
            let kind = type_of(container, locus.container.is_empty()).to_string();
            // Its mappings, each at its place in its macro's list again (kept in ascending order of those places).
            for gone in kept.prior["mappings"].as_array().into_iter().flatten() {
                let m = gone["macro"].as_u64().unwrap_or(0);
                let k = gone["at"].as_u64().unwrap_or(0) as usize;
                let Ok(mapping) = xml::parse(gone["xml"].as_str().unwrap_or_default()) else { continue };
                let tag = format!("Macro{m}");
                let Some(macro_) = container.child_mut(&tag) else { continue };
                if macro_.child("Mappings").is_none() {
                    let count = macro_.element_count();
                    macro_.insert_element(count, Element::new("Mappings"));
                }
                let list = macro_.child_mut("Mappings").expect("made above");
                let k = k.min(list.element_count());
                list.insert_element(k, mapping.root);
            }
            // The meta devices it cleared, where nothing aimed them elsewhere since.
            let mut left = 0;
            if let Some(list) = container.child_mut(chain_name(&kind, locus.lane)).and_then(|chain| chain.child_mut("Devices")) {
                for aim in kept.prior["aims"].as_array().into_iter().flatten() {
                    let d = aim["device"].as_u64().unwrap_or(0) as usize;
                    let d = if d >= locus.place { d + 1 } else { d };
                    let prefix = aim["prefix"].as_str().unwrap_or_default();
                    let Some(meta) = list.element_mut(d) else { continue };
                    if value_of(meta, &format!("{prefix}DestEffect")) == Some(-1.0) {
                        set_value(meta, &format!("{prefix}DestEffect"), locus.place as f64);
                        if let Some(parameter) = aim["parameter"].as_f64().filter(|p| *p >= 0.0) {
                            set_value(meta, &format!("{prefix}DestParameter"), parameter);
                        }
                    } else {
                        left += 1;
                    }
                }
            }
            let note = (left > 0).then(|| {
                format!(
                    "{left} meta device{} aimed elsewhere since stay{} as {}.",
                    if left == 1 { "" } else { "s" },
                    if left == 1 { "s" } else { "" },
                    if left == 1 { "it is" } else { "they are" }
                )
            });
            Ok(Undo::Write { document: Box::new(document), title: put_back, note })
        }
        "doofer-device-move" => {
            let (at, to) = (list_of(&kept.applied["at"]), list_of(&kept.applied["to"]));
            if at == to {
                return Ok(Undo::Back(format!("{} as it was", device.named())));
            }
            // Taken back: from where it went to where it was, read the same way (with it out of its place), and only as the
            // move itself may go (an aim the producer gave a meta device at it since would point elsewhere after).
            let from = resolve(slot(&document)?, &to, false)?;
            let mut probe = document.clone();
            let (element, _, _) = take_out(slot_mut(&mut probe), &from);
            let target = resolve(slot(&probe)?, &at, true)?;
            if let Some(why) = move_refusal(slot(&document)?, &from, &target, &inner_label(&element)) {
                return changed(format!("{why} (since Kumi moved it)"));
            }
            move_device(slot_mut(&mut document), &from, &target, element);
            Ok(Undo::Write { document: Box::new(document), title: put_back, note: None })
        }
        "doofer-macro" => {
            let m = kept.applied["macro"].as_i64().unwrap_or(1);
            let top = type_of(slot(&document)?, true).to_string();
            let slot_ = slot(&document)?;
            let macro_ = slot_.child(&format!("Macro{}", m - 1));
            let mut back = Map::new();
            back.insert("macro".into(), m.into());
            let mut wrote = false;
            for field in ["name", "value", "mappings", "activeMacros"] {
                let (Some(was), Some(set)) = (kept.prior.get(field), kept.applied.get(field)) else { continue };
                let now = match field {
                    "name" => macro_.and_then(|e| e.child_text("Name")).map(Value::from).unwrap_or(Value::Null),
                    "value" => number(macro_.and_then(|e| e.child("Value"))).map(Value::from).unwrap_or(Value::Null),
                    "mappings" => Value::Array(macro_.map(mappings_of).unwrap_or_default()),
                    _ => number(slot_.child("NumActiveMacros")).map(|n| Value::from(n as i64)).unwrap_or(Value::Null),
                };
                let same = |a: &Value, b: &Value| if field == "mappings" { same_mappings(a, b) } else { close(a, b) };
                if same(&now, was) {
                    continue;
                }
                if !same(&now, set) && !kept.discard {
                    return changed(format!("macro {m} of {} changed in Renoise since Kumi set it", q(&device.named())));
                }
                if field == "mappings" {
                    // (its mappings are places in the container's lanes: put back only while they are as Kumi left them)
                    let chains = Value::Array((0..lanes_of(&top)).map(|lane| chain_print(slot_, &[], lane)).collect());
                    if kept.applied.get("chains").is_some_and(|kept| *kept != chains) {
                        return changed(format!(
                            "the devices in {} were added, deleted or moved in Renoise since, so macro {m}'s mappings can't be put back where they were",
                            q(&device.named())
                        ));
                    }
                    // (what goes back was a mapping of these places: only the places are checked, not what Kumi knows of
                    // the parameters, as a plug-in's can't be counted)
                    mappings_fit(slot_, &top, was)?;
                }
                back.insert(field.into(), was.clone());
                wrote = true;
            }
            if !wrote {
                return Ok(Undo::Back(format!("macro {m} of {} is as it was already", device.named())));
            }
            write_macro(slot_mut(&mut document), &top, &Value::Object(back));
            Ok(Undo::Write { document: Box::new(document), title: put_back, note: None })
        }
        "doofer-device-set" => {
            let at = list_of(&kept.applied["at"]);
            let locus = resolve(slot(&document)?, &at, false)?;
            if chain_print(slot(&document)?, &locus.container, locus.lane) != kept.applied["chain"] {
                return changed(format!(
                    "the devices in {} were added, deleted or moved in Renoise since, so the one Kumi changed can't be told",
                    q(&device.named())
                ));
            }
            let target = {
                let container = element_at(slot(&document)?, &locus.container).expect("resolved");
                let kind = type_of(container, locus.container.is_empty());
                devices_in(container, kind, locus.lane)[locus.place].clone()
            };
            let mut writes: Vec<(String, f64)> = vec![];
            let mut name_back = None;
            if let (Some(was), Some(set)) = (kept.prior.get("param"), kept.applied.get("param")) {
                let element = set["element"].as_str().unwrap_or_default();
                let now = value_of(&target, element).map(Value::from).unwrap_or(Value::Null);
                if !close(&now, &was["value"]) {
                    if !close(&now, &set["value"]) && !kept.discard {
                        return changed(format!("that parameter of {} changed in Renoise since Kumi set it", q(&inner_label(&target))));
                    }
                    if let Some(v) = was["value"].as_f64() {
                        writes.push((element.to_string(), v));
                    }
                }
            }
            if let (Some(was), Some(set)) = (kept.prior.get("bypass"), kept.applied.get("bypass")) {
                let now = value_of(&target, "IsActive").map(|v| Value::from(v < 0.5)).unwrap_or(Value::Null);
                if now != *was {
                    if now != *set && !kept.discard {
                        return changed(format!("{} was switched in Renoise since Kumi set it", q(&inner_label(&target))));
                    }
                    if let Some(bypass) = was.as_bool() {
                        writes.push(("IsActive".into(), if bypass { 0.0 } else { 1.0 }));
                    }
                }
            }
            if let (Some(was), Some(set)) = (kept.prior.get("name"), kept.applied.get("name")) {
                let now = Value::from(target.child_text("CustomDeviceName").unwrap_or_default());
                if now != *was {
                    if now != *set && !kept.discard {
                        return changed(format!("{} was renamed in Renoise since Kumi named it", q(&inner_label(&target))));
                    }
                    name_back = was.as_str().map(str::to_owned);
                }
            }
            if writes.is_empty() && name_back.is_none() {
                return Ok(Undo::Back(format!("{} in {} is as it was already", inner_label(&target), device.named())));
            }
            let element = target_mut(slot_mut(&mut document), &locus);
            for (name, value) in writes {
                set_value(element, &name, value);
            }
            if let Some(name) = name_back {
                set_name(element, &name);
            }
            Ok(Undo::Write { document: Box::new(document), title: put_back, note: None })
        }
        "splitter-set" => {
            let now = slot(&document)?.child_text("Type").unwrap_or_else(|| "Parallel".into());
            let word = |w: &Value| SPLITS.iter().find(|(k, _)| Some(*k) == w.as_str()).map(|(_, x)| *x);
            let (Some(was), Some(set)) = (word(&kept.prior["type"]), word(&kept.applied["type"])) else {
                return bad("prior and applied: the split");
            };
            if now == was {
                return Ok(Undo::Back(format!("{} splits as it did already", device.named())));
            }
            if now != set && !kept.discard {
                return changed(format!("{}'s split changed in Renoise since Kumi set it", q(&device.named())));
            }
            slot_mut(&mut document).set_child_text("Type", was);
            Ok(Undo::Write { document: Box::new(document), title: put_back, note: None })
        }
        "notepad-write" => {
            let at = kept.applied.get("at").filter(|at| !at.is_null()).map(list_of);
            let element = match &at {
                None => slot_mut(&mut document),
                Some(at) => {
                    let locus = resolve(slot(&document)?, at, false)?;
                    let element = target_mut(slot_mut(&mut document), &locus);
                    if element.name() != NOTEPAD {
                        return changed("the Notepad isn't at its place any more");
                    }
                    element
                }
            };
            let now = Value::from(paragraphs_of(element));
            if now == kept.prior["paragraphs"] {
                return Ok(Undo::Back(format!("the Notepad of {} says what it did already", device.named())));
            }
            if now != kept.applied["paragraphs"] && !kept.discard {
                return changed("the Notepad's text changed in Renoise since Kumi wrote it");
            }
            let lines: Vec<String> =
                kept.prior["paragraphs"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
            write_paragraphs(element, &lines);
            Ok(Undo::Write { document: Box::new(document), title: put_back, note: None })
        }
        other => bad(format!("{other} isn't made here")),
    }
}

fn list_of(value: &Value) -> Vec<i64> {
    value.as_array().into_iter().flatten().filter_map(Value::as_i64).collect()
}

fn close(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0),
        _ => a == b,
    }
}

// Reads: what a Doofer or Splitter holds, and a Notepad's text, from its preset data (a device read by ref, the DSP body).

/// The most devices and parameters a read lists inside one container (the answer stays inside the frame bound).
const MOST_LISTED: usize = 64;
const MOST_PARAMS: usize = 40;

fn split_word(element: &Element) -> &'static str {
    let now = element.child_text("Type").unwrap_or_else(|| "Parallel".into());
    SPLITS.iter().find(|(_, x)| **x == now).map_or("parallel", |(w, _)| w)
}

/// A device inside, for a read: its place, names, bypass, its parameters (native ones, with Renoise's text) and, for a
/// container, what it holds.
fn describe_device(device: &Element, at: &[i64], with_params: bool) -> Value {
    let mut row = json!({
        "at": at,
        "name": inner_label(device),
        "device": devices::by_xml(device.name()).map_or_else(|| device.name().to_string(), |d| d.name.to_string()),
        "bypassed": value_of(device, "IsActive").is_some_and(|v| v < 0.5),
    });
    if device.name() == PLUGIN {
        row["plugin"] = device.child_text("PluginIdentifier").unwrap_or_default().into();
    }
    if device.name() == NOTEPAD {
        row["text"] = paragraphs_of(device).join("\n").into();
    }
    if with_params {
        if let Some(native) = devices::by_xml(device.name()) {
            let params: Vec<Value> = native
                .params
                .iter()
                .enumerate()
                .filter(|(_, p)| !p.element.is_empty())
                .take(MOST_PARAMS)
                .filter_map(|(k, p)| {
                    let value = value_of(device, p.element)?;
                    Some(json!({"param": k + 1, "name": parameter_name(device, k + 1), "value": value, "text": p.text.show(value, p.min)}))
                })
                .collect();
            if !params.is_empty() {
                row["params"] = Value::Array(params);
            }
        }
    }
    if is_container(device.name()) {
        row["inside"] = describe_container(device, device.name(), at, with_params);
    }
    row
}

/// A container for a read: its split (a Splitter), how many macros it shows, its macros (name, value, what each drives,
/// in words), and its devices by place.
fn describe_container(container: &Element, kind: &str, at: &[i64], with_params: bool) -> Value {
    let macros: Vec<Value> = macros_of(container, kind)
        .into_iter()
        .map(|(m, i)| {
            let macro_ = container.element(i).expect("in range");
            let mappings: Vec<Value> = mappings_of(macro_)
                .into_iter()
                .map(|mut mapping| {
                    let lane = (mapping["lane"].as_i64().unwrap_or(1) - 1).max(0) as usize;
                    let place = (mapping["device"].as_i64().unwrap_or(1) - 1).max(0) as usize;
                    if let Some(target) = devices_in(container, kind, lane).get(place) {
                        let k = mapping["param"].as_u64().unwrap_or(0) as usize;
                        mapping["drives"] = format!("{} of {}", parameter_name(target, k), inner_label(target)).into();
                    }
                    if kind != SPLITTER {
                        mapping.as_object_mut().expect("an object").remove("lane");
                    }
                    mapping
                })
                .collect();
            let mut row = json!({
                "macro": m + 1,
                "name": macro_.child_text("Name").unwrap_or_else(|| format!("Macro {}", m + 1)),
                "value": number(macro_.child("Value")).unwrap_or(50.0),
            });
            if !mappings.is_empty() {
                row["mappings"] = Value::Array(mappings);
            }
            row
        })
        .collect();
    let mut out = json!({
        "type": if kind == SPLITTER { "splitter" } else { "doofer" },
        "activeMacros": number(container.child("NumActiveMacros")).map_or(4, |n| n as i64),
        "macros": macros,
    });
    let mut listed = 0;
    let mut truncated = false;
    let mut lanes = vec![];
    for lane in 0..lanes_of(kind) {
        let mut rows = vec![];
        for (p, device) in devices_in(container, kind, lane).into_iter().enumerate() {
            if listed >= MOST_LISTED {
                truncated = true;
                break;
            }
            listed += 1;
            let mut place = at.to_vec();
            if kind == SPLITTER {
                place.push(lane as i64 + 1);
            }
            place.push(p as i64 + 1);
            rows.push(describe_device(device, &place, with_params));
        }
        lanes.push(Value::Array(rows));
    }
    if kind == SPLITTER {
        out["split"] = split_word(container).into();
        out["lanes"] = Value::Array(lanes);
    } else {
        out["devices"] = lanes.into_iter().next().unwrap_or(Value::Array(vec![]));
    }
    if truncated {
        out["truncated"] = true.into();
    }
    out
}

/// What a device's preset data holds, for a read: a container's macros and devices (`inside`), or a Notepad's text;
/// none for another device. `with_params` lists the parameters of the devices inside.
pub fn inside(text: &str, with_params: bool) -> Option<(&'static str, Value)> {
    let document = xml::parse(text).ok()?;
    let slot_ = document.root.child("DeviceSlot")?;
    let kind = type_of(slot_, true);
    if is_container(kind) {
        return Some(("inside", describe_container(slot_, kind, &[], with_params)));
    }
    (kind == NOTEPAD).then(|| ("text", Value::from(paragraphs_of(slot_).join("\n"))))
}

/// A Splitter's lanes as FOCUS names them, by its type: "Low" and "High" for Frequency (its gains' names), "Mid" and
/// "Side", or "Lane 1" and "Lane 2" in parallel. None for another device.
fn lane_names(container: &Element, kind: &str) -> Option<Vec<&'static str>> {
    (kind == SPLITTER).then(|| match split_word(container) {
        "frequency" => vec!["Low", "High"],
        "mid-side" => vec!["Mid", "Side"],
        _ => vec!["Lane 1", "Lane 2"],
    })
}

/// A container's lanes as FOCUS names them (a Splitter's), from its preset data; None for a Doofer or another device.
pub fn lanes(text: &str) -> Option<Vec<&'static str>> {
    let document = xml::parse(text).ok()?;
    let slot_ = document.root.child("DeviceSlot")?;
    lane_names(slot_, type_of(slot_, true))
}

/// A container's devices as the DSP body's child nodes: names, on or off, plug-in or not, a container's own children
/// (and a Splitter's lanes' names), each Splitter's device's `lane` (from 1), and `parallel` on the first device of a
/// Splitter's second lane.
pub fn child_nodes(text: &str) -> Vec<Value> {
    fn nodes(container: &Element, kind: &str, at: &[i64]) -> Vec<Value> {
        let mut out = vec![];
        for lane in 0..lanes_of(kind) {
            for (p, device) in devices_in(container, kind, lane).into_iter().enumerate() {
                let mut place = at.to_vec();
                if kind == SPLITTER {
                    place.push(lane as i64 + 1);
                }
                place.push(p as i64 + 1);
                let mut node = json!({
                    "at": place,
                    "name": inner_label(device),
                    "enabled": !value_of(device, "IsActive").is_some_and(|v| v < 0.5),
                    "plugin": device.name() == PLUGIN,
                    "container": is_container(device.name()),
                    "childNodes": if is_container(device.name()) { Value::Array(nodes(device, device.name(), &place)) } else { json!([]) },
                });
                if kind == SPLITTER {
                    node["lane"] = (lane + 1).into();
                }
                if let Some(names) = lane_names(device, device.name()) {
                    node["lanes"] = names.into();
                }
                if lane == 1 && p == 0 {
                    node["parallel"] = true.into();
                }
                out.push(node);
                if out.len() >= MOST_LISTED {
                    return out;
                }
            }
        }
        out
    }
    let Ok(document) = xml::parse(text) else { return vec![] };
    let Some(slot_) = document.root.child("DeviceSlot") else { return vec![] };
    let kind = type_of(slot_, true);
    if !is_container(kind) {
        return vec![];
    }
    nodes(slot_, kind, &[])
}

// The host's part: previews, applies and undos of these kinds (changes.rs hands them here).

const AU_PATH: &str = AU;

pub(crate) fn refusal_answer(refusal: &Refusal) -> Value {
    json!({"error": refusal.message(), "code": refusal.code()})
}

/// What a record's prior is compared by between the preview and the apply: a kept device's XML by its shape (its values
/// move by themselves).
fn comparable(prior: &Value) -> Value {
    let mut prior = prior.clone();
    if let Some(text) = prior.get("xml").and_then(Value::as_str).map(str::to_owned) {
        prior["xml"] = shape_of(&text).into();
    }
    prior
}

/// What Kumi is told of a prior: no kept XML (a deleted device's, which can be megabytes).
fn prior_for_kumi(prior: &Value) -> Value {
    let mut prior = prior.clone();
    if let Some(fields) = prior.as_object_mut() {
        fields.remove("xml");
        fields.remove("mappings");
        fields.remove("aims");
    }
    prior
}

fn read_params(input: &Value, kind: &str) -> Value {
    let mut params = json!({"ref": input.get("ref").cloned().unwrap_or(Value::Null)});
    if kind == "doofer-device-add" {
        if let Some(path) = input.get("path").and_then(Value::as_str).filter(|p| p.starts_with(AU_PATH)) {
            params["paths"] = json!([path]);
        }
    }
    params
}

fn display_of(edited: &Edited, unchanged: bool) -> Value {
    let mut display = json!({"title": edited.title});
    if let Some(note) = &edited.note {
        display["note"] = note.clone().into();
    }
    if unchanged {
        display["unchanged"] = true.into();
    }
    display
}

/// How long a read's look into preset data waits.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// The most containers of one DSP body looked into.
const MOST_CONTAINERS: usize = 16;

fn too_big(answer: &Value) -> bool {
    serde_json::to_string(answer).map_or(true, |text| text.len() > crate::host::MAX_ANSWER_BYTES)
}

impl crate::host::Host {
    /// A Renoise read of one device by ref: a Doofer's or Splitter's contents (`inside`), a Notepad's text, and its
    /// preset data when asked, from the tool's `preset_read`; each left out, and said, past the frame bound.
    pub(crate) async fn renoise_inside(&self, answer: &mut Value, pick: &crate::host::Pick, preset_data: bool) {
        self.renoise_macro_mappings(answer, pick).await;
        let Some(row) = answer.get("items").and_then(Value::as_array).and_then(|items| items.first()) else { return };
        let Some(reference) = row.get("ref").and_then(Value::as_str).filter(|r| r.contains(":device:")).map(str::to_owned) else { return };
        let contains = row.get("isDoofer") == Some(&Value::Bool(true))
            || row.get("isSplitter") == Some(&Value::Bool(true))
            || row.get("path").and_then(Value::as_str) == Some("Audio/Effects/Native/Notepad");
        if !contains && !preset_data {
            return;
        }
        let mut params = json!({"ref": reference});
        crate::changes::place(&mut params, answer.get("epoch").and_then(Value::as_str), answer.get("layout").and_then(Value::as_u64));
        let Ok(read) = self.call("preset_read", params, pick, READ_TIMEOUT).await else { return };
        let text = read.get("presetData").and_then(Value::as_str).unwrap_or_default().to_string();
        let found = with_plugin_names(read.get("plugins"), || inside(&text, true));
        let item = &mut answer["items"][0];
        if let Some((key, value)) = &found {
            item[*key] = value.clone();
        }
        if preset_data {
            item["presetData"] = text.clone().into();
        }
        if too_big(answer) {
            if let Some((key, value)) = with_plugin_names(read.get("plugins"), || inside(&text, false)) {
                answer["items"][0][key] = value;
            }
        }
        if too_big(answer) && preset_data {
            let item = answer["items"][0].as_object_mut().expect("a row");
            item.remove("presetData");
            item.insert("presetDataTooBig".into(), true.into());
        }
        if too_big(answer) {
            if let Some((key, _)) = found {
                let item = answer["items"][0].as_object_mut().expect("a row");
                item.remove(key);
                item.insert(format!("{key}TooBig"), true.into());
            }
        }
    }

    /// `device_editor` / `preset_save` / `show_status`: Renoise's actions on a device and its status bar line (no step of its
    /// undo), sent on to the tool.
    pub(crate) async fn renoise_action(&self, method: &str, arguments: &Value) -> (Value, bool) {
        let pick = match crate::host::Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        let mut params = json!({});
        for field in ["ref", "show", "name", "replace", "epoch", "layout", "text"] {
            if let Some(value) = arguments.get(field) {
                params[field] = value.clone();
            }
        }
        if method == "preset_save" {
            if let Some(library) = crate::renoise::library_files::user_library() {
                params["library"] = library.into();
            }
        }
        match self.call(method, params, &pick, crate::changes::CHANGE_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    /// A Renoise DSP body: each Doofer's and Splitter's devices as its child nodes (the first containers of the chain).
    pub(crate) async fn renoise_dsp_children(&self, answer: &mut Value, pick: &crate::host::Pick) {
        let epoch = answer.get("epoch").and_then(Value::as_str).map(str::to_owned);
        let layout = answer.get("layout").and_then(Value::as_u64);
        let Some(nodes) = answer.get("nodes").and_then(Value::as_array) else { return };
        let wanted: Vec<(usize, String)> = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.get("container") == Some(&Value::Bool(true)))
            .filter_map(|(i, node)| node.get("ref").and_then(Value::as_str).map(|r| (i, r.to_owned())))
            .take(MOST_CONTAINERS)
            .collect();
        for (i, reference) in wanted {
            let mut params = json!({"ref": reference});
            crate::changes::place(&mut params, epoch.as_deref(), layout);
            if let Ok(read) = self.call("preset_read", params, pick, READ_TIMEOUT).await {
                let data = read.get("presetData").and_then(Value::as_str).unwrap_or_default();
                let children = with_plugin_names(read.get("plugins"), || child_nodes(data));
                answer["nodes"][i]["childNodes"] = Value::Array(children);
                if let Some(names) = lanes(data) {
                    answer["nodes"][i]["lanes"] = names.into();
                }
            }
        }
        if too_big(answer) {
            for node in answer["nodes"].as_array_mut().into_iter().flatten() {
                node["childNodes"] = json!([]);
            }
            answer["childNodesTooBig"] = true.into();
        }
    }

    /// `change_preview` of a kind made here: the device's preset data read, the edit made on it (nothing written), the
    /// record kept with what was there.
    pub(crate) async fn preset_preview(
        &self,
        pick: &crate::host::Pick,
        kind: &str,
        input: &Value,
        epoch: Option<&str>,
        layout: Option<u64>,
    ) -> (Value, bool) {
        if kind == "doofer-make" {
            return self.make_preview(pick, input, epoch, layout).await;
        }
        if kind == crate::renoise::instruments::KIND {
            return self.mapping_preview(pick, input, epoch, layout).await;
        }
        if !input.get("ref").is_some_and(Value::is_string) {
            return (json!({"error": "ref: the device", "code": "bad-params"}), true);
        }
        let mut params = read_params(input, kind);
        crate::changes::place(&mut params, epoch, layout);
        let read = match self.call("preset_read", params, pick, crate::changes::CHANGE_TIMEOUT).await {
            Ok(read) => read,
            Err(error) => return (self.call_error(&error), true),
        };
        let text = read.get("presetData").and_then(Value::as_str).unwrap_or_default();
        let edited = match with_plugin_names(read.get("plugins"), || edit(kind, input, text, &Device::from_read(&read))) {
            Ok(edited) => edited,
            Err(refusal) => return (refusal_answer(&refusal), true),
        };
        let display = display_of(&edited, edited.document.to_xml() == text);
        let answer = json!({
            "epoch": read.get("epoch").cloned().unwrap_or(Value::Null),
            "layout": read.get("layout").cloned().unwrap_or(Value::Null),
            "target": read.get("target").cloned().unwrap_or(Value::Null),
            "prior": edited.prior,
            "fence": {"id": read.get("id").cloned().unwrap_or(Value::Null), "shape": read.get("shape").cloned().unwrap_or(Value::Null)},
            "display": display,
        });
        match self.journal().previewed(kind, input, &answer, layout, pick.clone()) {
            Ok(record) => {
                let mut reply = json!({"transactionId": record.id, "confirmation": record.confirmation, "prior": prior_for_kumi(&record.prior), "display": record.display, "target": record.target});
                crate::changes::place(&mut reply, record.epoch.as_deref(), record.layout);
                (reply, false)
            }
            Err(why) => (json!({"error": why.message, "code": why.code}), true),
        }
    }

    /// What the bridge's `change_apply` is sent for a kind made here: the edit made again on the preset data as it is now
    /// (refused `changed`, nothing written, when what the preview found moved), with the shape it was made from.
    pub(crate) async fn preset_apply_params(&self, record: &crate::changes::Record) -> Result<Value, Value> {
        if record.kind == "doofer-make" {
            return self.make_apply_params(record).await;
        }
        if record.kind == crate::renoise::instruments::KIND {
            return self.mapping_apply_params(record).await;
        }
        let mut params = read_params(&record.input, &record.kind);
        crate::changes::place(&mut params, record.epoch.as_deref(), record.input_layout);
        let read =
            self.call("preset_read", params, &record.instance, crate::changes::CHANGE_TIMEOUT).await.map_err(|e| self.call_error(&e))?;
        let device = Device::from_read(&read);
        if read.get("id") != record.fence.get("id") {
            return Err(
                json!({"error": format!("{} isn't the device at that place any more (a device was made, deleted or moved): read again", q(&device.named())), "code": "changed"}),
            );
        }
        let text = read.get("presetData").and_then(Value::as_str).unwrap_or_default();
        let edited =
            with_plugin_names(read.get("plugins"), || edit(&record.kind, &record.input, text, &device)).map_err(|r| refusal_answer(&r))?;
        if comparable(&edited.prior) != comparable(&record.prior) {
            return Err(
                json!({"error": format!("{} changed in Renoise since the preview: preview it again", q(&device.named())), "code": "changed"}),
            );
        }
        // What Kumi's undo puts back is what this apply found (a deleted device's knobs turned since the preview, say).
        self.journal().refresh_prior(&record.id, edited.prior.clone());
        let composed = edited.document.to_xml();
        let mut apply = json!({
            "kind": record.kind,
            "input": {"ref": record.input.get("ref").cloned().unwrap_or(Value::Null), "presetData": composed, "shape": read.get("shape").cloned().unwrap_or(Value::Null), "display": display_of(&edited, false), "edit": edited.applied},
            "fence": {"id": read.get("id").cloned().unwrap_or(Value::Null)},
            "desc": record.desc,
        });
        crate::changes::place(&mut apply, record.epoch.as_deref(), record.input_layout);
        Ok(apply)
    }

    /// A doofer-device-add's apply answer with what the device added holds as Renoise wrote it (`applied.edit.content`, read
    /// back right after): Kumi's undo compares it, and leaves a device the producer worked on since. Read again once when the
    /// first read fails; read neither time, it says so (`contentUnknown`), and the undo can't tell. Unchanged for any other
    /// kind.
    pub(crate) async fn with_added_content(&self, record: &crate::changes::Record, mut answer: Value) -> Value {
        if record.kind != "doofer-device-add" || !host_makes(self.daw(), &record.kind, &record.input) {
            return answer;
        }
        let Some(at) = answer.pointer("/applied/edit/at").map(list_of).filter(|at| !at.is_empty()) else { return answer };
        let mut params = read_params(&record.input, &record.kind);
        crate::changes::place(&mut params, record.epoch.as_deref(), record.input_layout);
        let mut content = None;
        for _ in 0..2 {
            let Ok(read) = self.call("preset_read", params.clone(), &record.instance, crate::changes::CHANGE_TIMEOUT).await else {
                continue;
            };
            content = content_at(read.get("presetData").and_then(Value::as_str).unwrap_or_default(), &at);
            if content.is_some() {
                break;
            }
        }
        if let Some(edit) = answer.pointer_mut("/applied/edit") {
            match content {
                Some(content) => edit["content"] = content.into(),
                None => edit["contentUnknown"] = true.into(),
            }
        }
        answer
    }

    /// The reads of the devices doofer-make puts in a container, by lane (each its ref and its preset_read), checked to be
    /// on the track it names.
    async fn make_reads(
        &self,
        input: &Value,
        epoch: Option<&str>,
        layout: Option<u64>,
        pick: &crate::host::Pick,
    ) -> Result<(Vec<Vec<(String, Value)>>, String), Value> {
        let bad_params = |message: &str| json!({"error": message, "code": "bad-params"});
        let track = input.get("trackRef").and_then(Value::as_str).ok_or_else(|| bad_params("trackRef: the track the devices are on"))?;
        let own = track
            .rsplit_once(":track:")
            .and_then(|(_, n)| n.parse::<i64>().ok())
            .ok_or_else(|| bad_params("trackRef must be a track ref"))?;
        let mut lanes = vec![];
        let mut seen = std::collections::BTreeSet::new();
        for field in ["deviceRefs", "laneRefs"] {
            let refs: Vec<&str> = match input.get(field) {
                None | Some(Value::Null) => vec![],
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| item.as_str().ok_or_else(|| bad_params(&format!("{field}: device refs"))))
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(bad_params(&format!("{field}: a list of device refs"))),
            };
            if field == "deviceRefs" && refs.is_empty() {
                return Err(bad_params("deviceRefs: the devices to put in it, in order"));
            }
            let mut lane = vec![];
            for reference in refs {
                let Some((t, _)) = device_numbers(reference) else { return Err(bad_params(&format!("{reference} isn't a device ref"))) };
                if t != own {
                    return Err(bad_params("every device must be on the track trackRef names"));
                }
                if !seen.insert(reference.to_string()) {
                    return Err(bad_params(&format!("{reference} is named twice")));
                }
                let mut params = json!({"ref": reference});
                crate::changes::place(&mut params, epoch, layout);
                let read = self.call("preset_read", params, pick, crate::changes::CHANGE_TIMEOUT).await.map_err(|e| self.call_error(&e))?;
                lane.push((reference.to_string(), read));
            }
            lanes.push(lane);
        }
        if lanes.iter().flatten().count() > 16 {
            return Err(bad_params("at most 16 devices go into one container at once"));
        }
        let track_name =
            lanes.iter().flatten().find_map(|(_, read)| read.get("trackName").and_then(Value::as_str)).unwrap_or("the track").to_string();
        Ok((lanes, track_name))
    }

    async fn make_preview(&self, pick: &crate::host::Pick, input: &Value, epoch: Option<&str>, layout: Option<u64>) -> (Value, bool) {
        let (lanes, track_name) = match self.make_reads(input, epoch, layout, pick).await {
            Ok(found) => found,
            Err(answer) => return (answer, true),
        };
        let edited = match make(input, &lanes, &track_name) {
            Ok(edited) => edited,
            Err(refusal) => return (refusal_answer(&refusal), true),
        };
        let first = &lanes[0][0].1;
        let ids: Vec<Value> = lanes.iter().flatten().map(|(_, read)| read.get("id").cloned().unwrap_or(Value::Null)).collect();
        let answer = json!({
            "epoch": first.get("epoch").cloned().unwrap_or(Value::Null),
            "layout": first.get("layout").cloned().unwrap_or(Value::Null),
            "target": {"kind": "track", "track": first["target"]["track"]},
            "prior": edited.prior,
            "fence": {"ids": ids},
            "display": display_of(&edited, false),
        });
        match self.journal().previewed("doofer-make", input, &answer, layout, pick.clone()) {
            Ok(record) => {
                let mut reply = json!({"transactionId": record.id, "confirmation": record.confirmation, "prior": record.prior, "display": record.display, "target": record.target});
                crate::changes::place(&mut reply, record.epoch.as_deref(), record.layout);
                (reply, false)
            }
            Err(why) => (json!({"error": why.message, "code": why.code}), true),
        }
    }

    async fn make_apply_params(&self, record: &crate::changes::Record) -> Result<Value, Value> {
        let (lanes, track_name) = self.make_reads(&record.input, record.epoch.as_deref(), record.input_layout, &record.instance).await?;
        let ids: Vec<Value> = lanes.iter().flatten().map(|(_, read)| read.get("id").cloned().unwrap_or(Value::Null)).collect();
        if Value::Array(ids.clone()) != record.fence["ids"] {
            return Err(
                json!({"error": "the devices aren't the ones previewed any more (a device was made, deleted or moved): read again", "code": "changed"}),
            );
        }
        let edited = make(&record.input, &lanes, &track_name).map_err(|r| refusal_answer(&r))?;
        if edited.prior != record.prior {
            return Err(json!({"error": "the devices changed in Renoise since the preview: preview it again", "code": "changed"}));
        }
        let shapes: Vec<Value> = lanes.iter().flatten().map(|(_, read)| read.get("shape").cloned().unwrap_or(Value::Null)).collect();
        let refs: Vec<Value> = lanes.iter().flatten().map(|(reference, _)| Value::from(reference.as_str())).collect();
        let mut apply = json!({
            "kind": "doofer-make",
            "input": {
                "trackRef": record.input.get("trackRef").cloned().unwrap_or(Value::Null),
                "deviceRefs": refs, "shapes": shapes, "presetData": edited.document.to_xml(),
                "type": record.input.get("type").cloned().unwrap_or_else(|| "doofer".into()),
                "index": record.input.get("index").cloned().unwrap_or(Value::Null),
                "name": record.input.get("name").cloned().unwrap_or(Value::Null),
                "display": display_of(&edited, false),
            },
            "fence": {"ids": ids},
            "desc": record.desc,
        });
        crate::changes::place(&mut apply, record.epoch.as_deref(), record.input_layout);
        Ok(apply)
    }

    /// What the bridge's `change_restore` is sent for a kind made here: the edit taken back from the preset data as it is
    /// now, found by the record's place. Ok(Err(answer)) when it is back already (nothing to write).
    pub(crate) async fn preset_restore_params(
        &self,
        record: &crate::changes::Record,
        desc: &str,
        discard: bool,
    ) -> Result<Result<Value, Value>, Value> {
        if record.kind == crate::renoise::instruments::KIND {
            return self.mapping_restore_params(record, desc).await;
        }
        let material = record.material.clone().unwrap_or(Value::Null);
        let mut params = json!({"at": material});
        crate::changes::place(&mut params, record.epoch.as_deref(), record.layout);
        let read =
            self.call("preset_read", params, &record.instance, crate::changes::CHANGE_TIMEOUT).await.map_err(|e| self.call_error(&e))?;
        let device = Device::from_read(&read);
        let text = read.get("presetData").and_then(Value::as_str).unwrap_or_default();
        let applied = record.applied.clone().unwrap_or(Value::Null);
        let null = Value::Null;
        let kept = Kept {
            kind: &record.kind,
            prior: &record.prior,
            applied: applied.get("edit").unwrap_or(&null),
            shape: applied.get("shape").and_then(Value::as_str).unwrap_or_default(),
            discard,
        };
        match with_plugin_names(read.get("plugins"), || {
            undo(&kept, text, read.get("shape").and_then(Value::as_str).unwrap_or_default(), &device)
        }) {
            Err(refusal) => Err(refusal_answer(&refusal)),
            Ok(Undo::Back(title)) => {
                Ok(Err(json!({"restored": {}, "display": {"title": title, "note": "It was back already: nothing was written."}})))
            }
            Ok(Undo::Write { document, title, note }) => {
                let mut material = material;
                let mut display = json!({"title": title});
                if let Some(note) = note {
                    display["note"] = note.into();
                }
                material["write"] = json!({"presetData": document.to_xml(), "shape": read.get("shape").cloned().unwrap_or(Value::Null), "display": display});
                let mut params =
                    json!({"kind": record.kind, "target": record.target, "prior": {}, "applied": {}, "material": material, "desc": desc});
                if discard {
                    params["discard"] = true.into();
                }
                crate::changes::place(&mut params, record.epoch.as_deref(), record.layout);
                Ok(Ok(params))
            }
        }
    }
}

#[cfg(test)]
mod tests;
