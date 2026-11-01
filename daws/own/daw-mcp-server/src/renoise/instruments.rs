//! An instrument's macro mappings added and removed (M4 n-devices). Renoise's API sets a mapping's range and scaling (the
//! tool does those itself, `changes_macros.lua`) but can't add or remove one, so Kumi's host edits the instrument's file:
//! the tool saves the instrument into Kumi's work folder (`instrument_export`), the host edits the macro's `<Mappings>` in
//! its `Instrument.xml` and packs it again beside it (every other entry copied as it was), and the tool loads that into the
//! same slot in one step. Kumi's undo puts the macro's mappings back the same way, byte for byte, refused when they, or
//! the chains and devices they point into by number, changed since.
//!
//! The file numbers a mapping's device as Renoise 3.5.4 does (measured): in a modulation set, k is the API's device k (the
//! set's hidden first device is 0); in a sample FX chain, 0 is its mixer device (the API's device 1). A parameter 0 is the
//! device's on/off switch, k the API's parameter k.

use serde_json::{json, Map, Value};

use crate::renoise::presets::{bad, changed, print_of, q, refusal_answer, scaling_word, Refusal, SCALINGS};
use crate::renoise::xml::{self, Element};

pub const KIND: &str = "instrument-macro-map";
/// The most mappings Kumi gives one macro.
const MOST_MAPPINGS: usize = 64;
const MODULATION: &str = "ModulationSet";
const FX: &str = "SampleDSP";

/// Whether the host makes this change (Renoise only): mappings added or removed. Ranges and scaling alone are the tool's.
pub fn is_host_change(daw: daw_bridge::Daw, kind: &str, input: &Value) -> bool {
    daw == daw_bridge::Daw::Renoise && kind == KIND && ["add", "remove"].iter().any(|k| input.get(*k).is_some_and(|v| !v.is_null()))
}

/// One mapping as the instrument's file holds it: the chain's type (`ModulationSet`, `SampleDSP`, `PhraseScript`) and
/// index, the device and parameter in it as the file numbers them, its range (0 to 1 of the parameter's) and scaling (the
/// file's word).
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    pub chain_type: String,
    pub chain: i64,
    pub device: i64,
    pub param: i64,
    pub min: f64,
    pub max: f64,
    pub scaling: String,
}

impl Mapping {
    fn read(element: &Element) -> Option<Mapping> {
        let number = |name: &str| element.child_text(name).and_then(|t| t.trim().parse::<f64>().ok());
        Some(Mapping {
            chain_type: element.child_text("DestChainType")?,
            chain: number("DestChainIndex")? as i64,
            device: number("DestDeviceIndex")? as i64,
            param: number("DestParameterIndex")? as i64,
            min: number("Min").unwrap_or(0.0),
            max: number("Max").unwrap_or(1.0),
            scaling: element.child_text("Scaling").unwrap_or_else(|| "Linear".into()),
        })
    }

    fn element(&self) -> Element {
        Element::new("Mapping")
            .with_child(Element::new("DestChainType").with_text(&self.chain_type))
            .with_child(Element::new("DestChainIndex").with_text(&self.chain.to_string()))
            .with_child(Element::new("DestDeviceIndex").with_text(&self.device.to_string()))
            .with_child(Element::new("DestParameterIndex").with_text(&self.param.to_string()))
            .with_child(Element::new("Min").with_text(&xml::number_text(self.min)))
            .with_child(Element::new("Max").with_text(&xml::number_text(self.max)))
            .with_child(Element::new("Scaling").with_text(&self.scaling))
    }

    fn same_target(&self, other: &Mapping) -> bool {
        self.chain_type == other.chain_type && self.chain == other.chain && self.device == other.device && self.param == other.param
    }

    fn same(&self, other: &Mapping) -> bool {
        self.same_target(other)
            && self.scaling == other.scaling
            && (self.min - other.min).abs() <= 1e-6
            && (self.max - other.max).abs() <= 1e-6
    }

    /// Its chain list ("modulation" or "fx"), the chain's number and the device's, as the API numbers them (from 1).
    fn api_place(&self) -> Option<(&'static str, i64, i64)> {
        match self.chain_type.as_str() {
            MODULATION => Some(("modulation", self.chain + 1, self.device)),
            FX => Some(("fx", self.chain + 1, self.device + 1)),
            _ => None,
        }
    }
}

fn same_list(a: &[Mapping], b: &[Mapping]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same(y))
}

/// The device a mapping points at, as the tool read the chains: {name, params}.
fn device_of<'a>(chains: &'a Value, list: &str, chain: i64, device: i64) -> Option<&'a Value> {
    let chain = chains.get(list)?.as_array()?.get(usize::try_from(chain - 1).ok()?)?;
    chain.get("devices")?.as_array()?.get(usize::try_from(device - 1).ok()?)
}

fn param_name(device: &Value, k: i64) -> Option<String> {
    if k == 0 {
        return Some("on/off".into());
    }
    device.get("params")?.as_array()?.get(usize::try_from(k - 1).ok()?)?.as_str().map(str::to_owned)
}

fn chain_words(list: &str, chain: i64) -> String {
    if list == "fx" {
        format!("sample FX chain {chain}")
    } else {
        format!("modulation set {chain}")
    }
}

/// A mapping as a phrase: "Gain (Gainer in sample FX chain 1)".
fn named(mapping: &Mapping, chains: &Value) -> String {
    match mapping.api_place() {
        Some((list, chain, device)) => match device_of(chains, list, chain, device) {
            Some(found) => format!(
                "{} ({} in {})",
                param_name(found, mapping.param).unwrap_or_else(|| format!("parameter {}", mapping.param)),
                found.get("name").and_then(Value::as_str).unwrap_or("a device"),
                chain_words(list, chain)
            ),
            None => format!("parameter {} of device {device} in {}", mapping.param, chain_words(list, chain)),
        },
        None => format!("a {} parameter", mapping.chain_type),
    }
}

/// A mapping in Kumi's words: {modulation | fx, device, deviceName, param, min, max, scaling}.
fn words(mapping: &Mapping, chains: &Value) -> Value {
    let mut out = Map::new();
    match mapping.api_place() {
        Some((list, chain, device)) => {
            out.insert(list.into(), chain.into());
            out.insert("device".into(), device.into());
            if let Some(found) = device_of(chains, list, chain, device) {
                out.insert("deviceName".into(), found.get("name").cloned().unwrap_or(Value::Null));
                if let Some(name) = param_name(found, mapping.param) {
                    out.insert("param".into(), name.into());
                }
            }
        }
        None => {
            out.insert("chain".into(), mapping.chain_type.clone().into());
        }
    }
    out.entry("param").or_insert_with(|| mapping.param.into());
    out.insert("min".into(), mapping.min.into());
    out.insert("max".into(), mapping.max.into());
    out.insert("scaling".into(), scaling_word(&mapping.scaling).into());
    Value::Object(out)
}

/// A print of the chains' devices (their names and parameter counts), which the mappings point into by number.
pub fn chains_print(chains: &Value) -> String {
    let mut text = String::new();
    for list in ["modulation", "fx"] {
        for (c, chain) in chains.get(list).and_then(Value::as_array).into_iter().flatten().enumerate() {
            for device in chain.get("devices").and_then(Value::as_array).into_iter().flatten() {
                let name = device.get("name").and_then(Value::as_str).unwrap_or_default();
                let count = device.get("params").and_then(Value::as_array).map_or(0, Vec::len);
                text.push_str(&format!("{list}{c}|{name}|{count}\n"));
            }
            text.push_str(&format!("{list}{c}.\n"));
        }
    }
    print_of(&text)
}

fn label_of(export: &Value) -> String {
    export.get("label").and_then(Value::as_str).unwrap_or("the instrument").to_string()
}

fn macro_number(input: &Value) -> Result<usize, Refusal> {
    match input.get("macro").and_then(Value::as_i64) {
        Some(m @ 1..=8) => Ok(m as usize),
        _ => bad("macro is a number from 1 to 8"),
    }
}

fn parse(text: &str) -> Result<xml::Document, Refusal> {
    xml::parse(text).map_err(|e| Refusal::Bad(format!("the instrument's file doesn't read as Renoise's ({e})")))
}

fn macro_element(document: &xml::Document, m: usize) -> Result<&Element, Refusal> {
    document
        .root
        .child("GlobalProperties")
        .and_then(|global| global.child(&format!("Macro{}", m - 1)))
        .ok_or_else(|| Refusal::Bad(format!("the instrument's file has no macro {m}")))
}

fn macro_element_mut(document: &mut xml::Document, m: usize) -> Result<&mut Element, Refusal> {
    document
        .root
        .child_mut("GlobalProperties")
        .and_then(|global| global.child_mut(&format!("Macro{}", m - 1)))
        .ok_or_else(|| Refusal::Bad(format!("the instrument's file has no macro {m}")))
}

/// A macro's mappings in the file, each with its element as written.
fn mappings_in(macro_: &Element, m: usize) -> Result<Vec<(Mapping, Element)>, Refusal> {
    let Some(list) = macro_.child("Mappings") else { return Ok(vec![]) };
    list.elements()
        .map(|element| {
            Mapping::read(element)
                .map(|mapping| (mapping, element.clone()))
                .ok_or_else(|| Refusal::Bad(format!("Kumi can't read macro {m}'s mappings in the instrument's file")))
        })
        .collect()
}

/// The macro's `<Mappings>` made these elements, in order (none: no `<Mappings>`, as Renoise writes a macro without).
fn set_mappings(macro_: &mut Element, wanted: Vec<Element>) {
    let at = macro_.elements().position(|element| element.name() == "Mappings");
    if wanted.is_empty() {
        if let Some(at) = at {
            macro_.remove_element(at);
        }
        return;
    }
    if at.is_none() {
        let after_name = macro_.elements().position(|element| element.name() == "Name").map_or(macro_.element_count(), |n| n + 1);
        macro_.insert_element(after_name, Element::new("Mappings"));
    }
    let list = macro_.child_mut("Mappings").expect("its mappings, there or just made");
    while list.element_count() > 0 {
        list.remove_element(0);
    }
    for element in wanted {
        let count = list.element_count();
        list.insert_element(count, element);
    }
}

/// The macro's mappings as the API read them must be the file's: as many, the same ranges and scalings, the same
/// parameters (by the names the tool read of the chains).
fn agree(prior: &[(Mapping, Element)], export: &Value, m: usize) -> Result<(), Refusal> {
    let mismatch = || {
        Refusal::Bad(format!(
            "Kumi can't line up macro {m}'s mappings in {}'s file with what Renoise reads of them: map it in Renoise",
            q(&label_of(export))
        ))
    };
    let read = match export.pointer(&format!("/macros/{}/mappings", m - 1)) {
        Some(Value::Array(items)) => items.clone(),
        // (an empty list, as an older tool's JSON wrote it)
        Some(Value::Object(fields)) if fields.is_empty() => vec![],
        _ => return Err(mismatch()),
    };
    if read.len() != prior.len() {
        return Err(mismatch());
    }
    let chains = export.get("chains").unwrap_or(&Value::Null);
    for ((mapping, _), api) in prior.iter().zip(read) {
        let close = |k: &str, v: f64| api.get(k).and_then(Value::as_f64).is_some_and(|x| (x - v).abs() <= 1e-4);
        if !close("min", mapping.min)
            || !close("max", mapping.max)
            || api.get("scaling").and_then(Value::as_str) != Some(scaling_word(&mapping.scaling))
        {
            return Err(mismatch());
        }
        if mapping.param > 0 {
            if let Some((list, chain, device)) = mapping.api_place() {
                let name = device_of(chains, list, chain, device).and_then(|found| param_name(found, mapping.param));
                if name.is_some() && api.get("param").and_then(Value::as_str) != name.as_deref() {
                    return Err(mismatch());
                }
            }
        }
    }
    Ok(())
}

fn unit_number(item: &Value, field: &str, n: usize, list: &str, default: f64) -> Result<f64, Refusal> {
    match item.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => match value.as_f64() {
            Some(v) if (0.0..=1.0).contains(&v) => Ok(v),
            _ => bad(format!("{list}[{n}].{field} is from 0 to 1 (of the parameter's range)")),
        },
    }
}

fn scaling_of(item: &Value, n: usize, list: &str, default: &str) -> Result<String, Refusal> {
    match item.get("scaling") {
        None | Some(Value::Null) => Ok(default.to_string()),
        Some(word) => match word.as_str().and_then(|w| SCALINGS.iter().find(|(name, _)| *name == w)) {
            Some((_, xml_word)) => Ok(xml_word.to_string()),
            None => bad(format!("{list}[{n}].scaling is log-fast, log-slow, linear, exp-slow or exp-fast")),
        },
    }
}

/// A mapping asked by `add`: {modulation | fx, device, param, min?, max?, scaling?}, checked against the chains.
fn asked(item: &Value, n: usize, chains: &Value) -> Result<Mapping, Refusal> {
    let (list, chain) = match (item.get("modulation").and_then(Value::as_i64), item.get("fx").and_then(Value::as_i64)) {
        (Some(c), None) => ("modulation", c),
        (None, Some(c)) => ("fx", c),
        _ => return bad(format!("add[{n}]: give modulation (a modulation set's number) or fx (a sample FX chain's number)")),
    };
    let lists = chains.get(list).and_then(Value::as_array).map_or(0, Vec::len) as i64;
    if chain < 1 || chain > lists {
        let what = if list == "fx" { "sample FX chains" } else { "modulation sets" };
        return bad(format!("add[{n}].{list}: the instrument has {lists} {what}"));
    }
    let devices = chains.pointer(&format!("/{list}/{}/devices", chain - 1)).and_then(Value::as_array).map_or(0, Vec::len) as i64;
    let device = item.get("device").and_then(Value::as_i64).unwrap_or(0);
    if device < 1 || device > devices {
        return bad(format!(
            "add[{n}].device is a number from 1 to {devices} (the devices of {} as a read lists them)",
            chain_words(list, chain)
        ));
    }
    let found = device_of(chains, list, chain, device).expect("a device in range");
    let names: Vec<&str> =
        found.get("params").and_then(Value::as_array).map(|p| p.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let device_name = found.get("name").and_then(Value::as_str).unwrap_or("the device");
    let param = match item.get("param") {
        Some(Value::Number(k)) => match k.as_i64() {
            Some(k) if k >= 0 && k <= names.len() as i64 => k,
            _ => return bad(format!("add[{n}].param is a number from 0 (its on/off) to {} or a parameter's name", names.len())),
        },
        Some(Value::String(name)) => match names.iter().position(|p| p.eq_ignore_ascii_case(name)) {
            Some(k) => k as i64 + 1,
            None => return bad(format!("{} has no parameter named {}: it has {}", q(device_name), q(name), names.join(", "))),
        },
        _ => return bad(format!("add[{n}].param: a parameter's name or number")),
    };
    let min = unit_number(item, "min", n, "add", 0.0)?;
    let max = unit_number(item, "max", n, "add", 1.0)?;
    let scaling = scaling_of(item, n, "add", "Linear")?;
    Ok(Mapping {
        chain_type: (if list == "fx" { FX } else { MODULATION }).to_string(),
        chain: chain - 1,
        device: if list == "fx" { device - 1 } else { device },
        param,
        min,
        max,
        scaling,
    })
}

/// The edit made: the file's Instrument.xml with it, the macro's mappings before and after (each its element as written).
#[derive(Debug)]
pub struct Mapped {
    pub macro_: usize,
    pub xml: String,
    pub prior: Vec<String>,
    pub applied: Vec<String>,
    pub title: String,
    /// Kumi's words for the mappings before.
    pub words: Vec<Value>,
}

/// The mappings asked of a macro (`set`, `remove`, `add`), made on the file's Instrument.xml; `export` is what the tool
/// told of the instrument (its macros as the API read them, its chains).
pub fn edit(input: &Value, text: &str, export: &Value) -> Result<Mapped, Refusal> {
    let m = macro_number(input)?;
    let chains = export.get("chains").unwrap_or(&Value::Null);
    let label = label_of(export);
    let mut document = parse(text)?;
    let prior = mappings_in(macro_element(&document, m)?, m)?;
    agree(&prior, export, m)?;
    let count = prior.len();
    let mut kept: Vec<Option<(Mapping, Element)>> = prior.iter().cloned().map(Some).collect();
    let mut said = vec![];
    for (n, item) in input.get("set").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let j = item.get("mapping").and_then(Value::as_i64).unwrap_or(0);
        if j < 1 || j as usize > count {
            return bad(format!("set[{n}].mapping is a number from 1 to {count} (the macro's mappings)"));
        }
        let Some((mapping, element)) = kept[j as usize - 1].as_mut() else { unreachable!("set before remove") };
        mapping.min = unit_number(item, "min", n, "set", mapping.min)?;
        mapping.max = unit_number(item, "max", n, "set", mapping.max)?;
        mapping.scaling = scaling_of(item, n, "set", &mapping.scaling.clone())?;
        let before = &prior[j as usize - 1].0;
        if !mapping.same(before) {
            // (only what changed: the rest keeps Renoise's text)
            if (mapping.min - before.min).abs() > 1e-6 {
                element.set_child_text("Min", &xml::number_text(mapping.min));
            }
            if (mapping.max - before.max).abs() > 1e-6 {
                element.set_child_text("Max", &xml::number_text(mapping.max));
            }
            if mapping.scaling != before.scaling {
                element.set_child_text("Scaling", &mapping.scaling);
            }
            said.push(format!("set {} to {}–{} {}", named(mapping, chains), mapping.min, mapping.max, scaling_word(&mapping.scaling)));
        }
    }
    let mut removed = vec![];
    for item in input.get("remove").and_then(Value::as_array).into_iter().flatten() {
        let j = item.as_i64().unwrap_or(0);
        if j < 1 || j as usize > count {
            return bad(format!("remove: mapping numbers from 1 to {count} (the macro's mappings)"));
        }
        match kept[j as usize - 1].take() {
            Some((mapping, _)) => removed.push(named(&mapping, chains)),
            None => return bad(format!("remove: mapping {j} is named twice")),
        }
    }
    if !removed.is_empty() {
        said.push(format!("unmapped {}", removed.join(", ")));
    }
    let mut wanted: Vec<(Mapping, Element)> = kept.into_iter().flatten().collect();
    let mut added = vec![];
    for (n, item) in input.get("add").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let mapping = asked(item, n, chains)?;
        if wanted.iter().any(|(other, _)| other.same_target(&mapping)) {
            return bad(format!("macro {m} maps {} already", named(&mapping, chains)));
        }
        added.push(named(&mapping, chains));
        let element = mapping.element();
        wanted.push((mapping, element));
    }
    if !added.is_empty() {
        said.push(format!("mapped to {}", added.join(", ")));
    }
    if wanted.len() > MOST_MAPPINGS {
        return bad(format!("a macro takes at most {MOST_MAPPINGS} mappings from Kumi"));
    }
    let words_before = prior.iter().map(|(mapping, _)| words(mapping, chains)).collect();
    let prior_xml = prior.iter().map(|(_, element)| element.to_xml()).collect();
    let applied_xml: Vec<String> = wanted.iter().map(|(_, element)| element.to_xml()).collect();
    set_mappings(macro_element_mut(&mut document, m)?, wanted.into_iter().map(|(_, element)| element).collect());
    let title = if said.is_empty() {
        format!("{label} macro {m}: its mappings as they are")
    } else {
        format!("{label} macro {m}: {}", said.join("; "))
    };
    Ok(Mapped { macro_: m, xml: document.to_xml(), prior: prior_xml, applied: applied_xml, title, words: words_before })
}

fn elements_of(raw: &[String]) -> Result<Vec<(Mapping, Element)>, Refusal> {
    raw.iter()
        .map(|text| {
            let element =
                xml::parse(text).map(|document| document.root).map_err(|_| Refusal::Bad("the change's mappings don't read".into()))?;
            let mapping = Mapping::read(&element).ok_or_else(|| Refusal::Bad("the change's mappings don't read".into()))?;
            Ok((mapping, element))
        })
        .collect()
}

fn texts(value: &Value) -> Vec<String> {
    value.as_array().map(|items| items.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// Kumi's undo of a mapping change, on the instrument's file as it is now: the macro's mappings before (`prior`) put back
/// byte for byte when they are as the change left them (`applied`) and the chains are as they were (`chains_then`).
/// None when they are back already.
pub fn undo(
    m: usize,
    prior: &[String],
    applied: &[String],
    chains_then: &str,
    text: &str,
    export: &Value,
) -> Result<Option<String>, Refusal> {
    let label = label_of(export);
    let mut document = parse(text)?;
    let now: Vec<Mapping> = mappings_in(macro_element(&document, m)?, m)?.into_iter().map(|(mapping, _)| mapping).collect();
    let prior = elements_of(prior)?;
    let applied: Vec<Mapping> = elements_of(applied)?.into_iter().map(|(mapping, _)| mapping).collect();
    let before: Vec<Mapping> = prior.iter().map(|(mapping, _)| mapping.clone()).collect();
    if same_list(&now, &before) {
        return Ok(None);
    }
    if !same_list(&now, &applied) {
        return changed(format!(
            "{}'s macro {m} mappings changed in Renoise since Kumi mapped them, so Kumi won't put them back",
            q(&label)
        ));
    }
    if chains_print(export.get("chains").unwrap_or(&Value::Null)) != chains_then {
        return changed(format!(
            "{}'s modulation sets or sample FX chains changed since, and its mappings point into them by number: Kumi won't put them back",
            q(&label)
        ));
    }
    set_mappings(macro_element_mut(&mut document, m)?, prior.into_iter().map(|(_, element)| element).collect());
    Ok(Some(document.to_xml()))
}

fn failed(message: &str) -> Value {
    json!({"error": message, "code": "failed"})
}

/// The instrument file the host wrote for the tool to load ([`write_beside`]), as what the bridge's `change_apply`
/// (`input.file`) or `change_restore` (`material.write.file`) is sent names it: the host removes it once the bridge has
/// answered. The tool removes it too, but with Lua's `os.remove`, which on Windows can't reach a folder whose name
/// isn't in the system's code page (#399, N-N9).
pub(crate) fn host_file(params: &Value) -> Option<std::path::PathBuf> {
    if params.get("kind").and_then(Value::as_str) != Some(KIND) {
        return None;
    }
    let file = params.pointer("/input/file").or_else(|| params.pointer("/material/write/file")).and_then(Value::as_str)?;
    file.ends_with(".kumi.xrni").then(|| file.into())
}

/// How old an instrument file in Kumi's work folder is before the host takes it for one left behind: one in use lasts a
/// change's few seconds (the tool's own sweep waits as long).
const STALE_WORK: std::time::Duration = std::time::Duration::from_secs(3600);

/// Removes the instrument files (`*.xrni`) left in Kumi's work folder (`<bridge root>/work`) before `now` by
/// [`STALE_WORK`] or more: a host or Renoise that quit mid-change, or a tool that couldn't remove its own (#399, N-N9).
/// Anything else in the folder stays. How many it removed.
pub fn sweep_work(dir: &std::path::Path, now: std::time::SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension.eq_ignore_ascii_case("xrni")))
        .filter(|entry| {
            let modified = entry.metadata().ok().filter(std::fs::Metadata::is_file).and_then(|metadata| metadata.modified().ok());
            modified.is_some_and(|modified| now.duration_since(modified).is_ok_and(|age| age >= STALE_WORK))
        })
        .filter(|entry| std::fs::remove_file(entry.path()).is_ok())
        .count()
}

/// The edited Instrument.xml packed with the saved file's other entries, written beside it for the tool to load.
fn write_beside(export: &Value, bytes: &[u8], text: &str) -> Result<String, Value> {
    let file = export.get("file").and_then(Value::as_str).ok_or_else(|| failed("the bridge didn't say where it saved the instrument"))?;
    let archive =
        crate::renoise::zip::Archive::read(bytes).map_err(|e| failed(&format!("the instrument Renoise saved doesn't read ({e})")))?;
    let packed = archive
        .with_replaced("Instrument.xml", text.as_bytes())
        .map_err(|e| failed(&format!("Kumi couldn't pack the instrument ({e})")))?;
    let path = std::path::Path::new(file).with_extension("kumi.xrni");
    std::fs::write(&path, packed).map_err(|e| failed(&format!("Kumi couldn't write the instrument file ({e})")))?;
    Ok(path.to_string_lossy().into_owned())
}

impl crate::host::Host {
    /// A Renoise instrument read by ref: its macros' mappings as Renoise's API reads them (`macroMappings`, the macros
    /// that have any: each mapping's number, parameter, range and scaling), which mapping changes name by number.
    pub(crate) async fn renoise_macro_mappings(&self, answer: &mut Value, pick: &crate::host::Pick) {
        let Some(row) = answer.get("items").and_then(Value::as_array).and_then(|items| items.first()) else { return };
        let Some(reference) = row.get("ref").and_then(Value::as_str).filter(|r| r.contains(":instrument:")).map(str::to_owned) else {
            return;
        };
        let mut params = json!({"ref": reference});
        crate::changes::place(&mut params, answer.get("epoch").and_then(Value::as_str), answer.get("layout").and_then(Value::as_u64));
        let Ok(read) = self.call("macro_read", params, pick, crate::changes::CHANGE_TIMEOUT).await else { return };
        let mapped: Vec<Value> = read
            .get("macros")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m.get("mappings").and_then(Value::as_array).is_some_and(|list| !list.is_empty()))
            .map(|m| json!({"macro": m["macro"], "name": m["name"], "mappings": m["mappings"]}))
            .collect();
        answer["items"][0]["macroMappings"] = mapped.into();
    }

    /// The instrument saved by the tool into Kumi's work folder and read (the file removed): what the tool told of it, the
    /// file's bytes and its Instrument.xml.
    async fn instrument_file(
        &self,
        mut params: Value,
        epoch: Option<&str>,
        layout: Option<u64>,
        pick: &crate::host::Pick,
    ) -> Result<(Value, Vec<u8>, String), Value> {
        crate::changes::place(&mut params, epoch, layout);
        let export = self.call("instrument_export", params, pick, crate::changes::CHANGE_TIMEOUT).await.map_err(|e| self.call_error(&e))?;
        let file =
            export.get("file").and_then(Value::as_str).ok_or_else(|| failed("the bridge didn't say where it saved the instrument"))?;
        let bytes = std::fs::read(file);
        let _ = std::fs::remove_file(file);
        let bytes = bytes.map_err(|e| failed(&format!("Kumi couldn't read the instrument Renoise saved ({e})")))?;
        let archive =
            crate::renoise::zip::Archive::read(&bytes).map_err(|e| failed(&format!("the instrument Renoise saved doesn't read ({e})")))?;
        let entry = archive.entry("Instrument.xml").ok_or_else(|| failed("the instrument Renoise saved has no Instrument.xml"))?;
        let contents = archive.contents(entry).map_err(|e| failed(&format!("its Instrument.xml doesn't read ({e})")))?;
        let text = String::from_utf8(contents).map_err(|_| failed("its Instrument.xml isn't UTF-8"))?;
        Ok((export, bytes, text))
    }

    /// `change_preview` of mappings added or removed: the instrument saved and its file edited (nothing loaded), the record
    /// kept with the macro's mappings as they were.
    pub(crate) async fn mapping_preview(
        &self,
        pick: &crate::host::Pick,
        input: &Value,
        epoch: Option<&str>,
        layout: Option<u64>,
    ) -> (Value, bool) {
        if !input.get("ref").is_some_and(Value::is_string) {
            return (json!({"error": "ref: the instrument", "code": "bad-params"}), true);
        }
        let (export, _, text) = match self.instrument_file(json!({"ref": input["ref"]}), epoch, layout, pick).await {
            Ok(found) => found,
            Err(answer) => return (answer, true),
        };
        let mapped = match edit(input, &text, &export) {
            Ok(mapped) => mapped,
            Err(refusal) => return (refusal_answer(&refusal), true),
        };
        let chains = export.get("chains").unwrap_or(&Value::Null);
        let mut display = json!({"title": mapped.title});
        if mapped.prior == mapped.applied {
            display["unchanged"] = true.into();
        }
        let answer = json!({
            "epoch": export.get("epoch").cloned().unwrap_or(Value::Null),
            "layout": export.get("layout").cloned().unwrap_or(Value::Null),
            "target": export.get("target").cloned().unwrap_or(Value::Null),
            "prior": {"macro": mapped.macro_, "mapped": mapped.words, "xml": mapped.prior},
            "fence": {"print": export.get("print").cloned().unwrap_or(Value::Null), "chains": chains_print(chains)},
            "display": display,
        });
        match self.journal().previewed(KIND, input, &answer, layout, pick.clone()) {
            Ok(record) => {
                let mut prior = record.prior.clone();
                if let Some(fields) = prior.as_object_mut() {
                    fields.remove("xml");
                }
                let mut reply = json!({"transactionId": record.id, "confirmation": record.confirmation, "prior": prior, "display": record.display, "target": record.target});
                crate::changes::place(&mut reply, record.epoch.as_deref(), record.layout);
                (reply, false)
            }
            Err(why) => (json!({"error": why.message, "code": why.code}), true),
        }
    }

    /// What the bridge's `change_apply` is sent for mappings added or removed: the instrument saved again, the edit made on
    /// it again (refused `changed`, nothing written, when its macros or chains moved since the preview), and the file
    /// written beside it for the tool to load.
    pub(crate) async fn mapping_apply_params(&self, record: &crate::changes::Record) -> Result<Value, Value> {
        let (export, bytes, text) = self
            .instrument_file(json!({"ref": record.input["ref"]}), record.epoch.as_deref(), record.input_layout, &record.instance)
            .await?;
        let chains = chains_print(export.get("chains").unwrap_or(&Value::Null));
        if export.get("print") != record.fence.get("print") || record.fence.get("chains").and_then(Value::as_str) != Some(chains.as_str()) {
            return Err(
                json!({"error": format!("{}'s macros or chains changed in Renoise since the preview: preview it again", q(&label_of(&export))), "code": "changed"}),
            );
        }
        let mapped = edit(&record.input, &text, &export).map_err(|r| refusal_answer(&r))?;
        if mapped.prior != texts(&record.prior["xml"]) {
            return Err(
                json!({"error": format!("{}'s macro mappings changed in Renoise since the preview: preview it again", q(&label_of(&export))), "code": "changed"}),
            );
        }
        let file = write_beside(&export, &bytes, &mapped.xml)?;
        let mut apply = json!({
            "kind": KIND,
            "input": {
                "ref": record.input["ref"], "macro": mapped.macro_, "file": file, "print": export.get("print").cloned().unwrap_or(Value::Null),
                "display": {"title": mapped.title}, "edit": {"xml": mapped.applied, "chains": chains},
            },
            "fence": {},
            "desc": record.desc,
        });
        crate::changes::place(&mut apply, record.epoch.as_deref(), record.input_layout);
        Ok(apply)
    }

    /// What the bridge's `change_restore` is sent for mappings added or removed: the instrument saved as it is now (found
    /// by the record's place) with the macro's mappings put back, written beside it. Ok(Err(answer)) when they are back
    /// already (nothing to write).
    pub(crate) async fn mapping_restore_params(&self, record: &crate::changes::Record, desc: &str) -> Result<Result<Value, Value>, Value> {
        let material = record.material.clone().unwrap_or(Value::Null);
        let (export, bytes, text) = self
            .instrument_file(
                json!({"at": material.get("at").cloned().unwrap_or(Value::Null)}),
                record.epoch.as_deref(),
                record.layout,
                &record.instance,
            )
            .await?;
        let m = macro_number(&record.input).map_err(|r| refusal_answer(&r))?;
        let edit = record.applied.as_ref().and_then(|applied| applied.get("edit")).cloned().unwrap_or(Value::Null);
        let chains_then = edit.get("chains").and_then(Value::as_str).unwrap_or_default();
        let label = label_of(&export);
        match undo(m, &texts(&record.prior["xml"]), &texts(&edit["xml"]), chains_then, &text, &export) {
            Err(refusal) => Err(refusal_answer(&refusal)),
            Ok(None) => Ok(Err(json!({
                "restored": {},
                "display": {"title": format!("{label} macro {m}'s mappings are as they were already"), "note": "It was back already: nothing was written."},
            }))),
            Ok(Some(xml)) => {
                let file = write_beside(&export, &bytes, &xml)?;
                let mut material = material;
                material["write"] = json!({"file": file, "print": export.get("print").cloned().unwrap_or(Value::Null), "display": {"title": format!("{label} macro {m}'s mappings put back")}});
                let mut params =
                    json!({"kind": KIND, "target": record.target, "prior": {}, "applied": {}, "material": material, "desc": desc});
                crate::changes::place(&mut params, record.epoch.as_deref(), record.layout);
                Ok(Ok(params))
            }
        }
    }
}

#[cfg(test)]
mod tests;
