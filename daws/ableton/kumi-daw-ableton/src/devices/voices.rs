//! Independent copies of a patched instrument, fed by the same pitch-owned [poly] allocator as code instruments.
//!
//! This is deliberately a local graph transformation: shared names, external abstractions, scripts and Live side
//! effects cannot safely become voices. Notes are allocated by pitch, not by MIDI channel (in particular, this is
//! not MPE). Channel messages retain their channel; system messages are outside this wrapper's MIDI contract.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::gen::MAX_VOICES;
use super::patch::catalog::catalog;
use super::patch::notation::{parse, Notation, What, MOST_BOXES};
use super::patch::reference::atoms;
use super::patch::{NoFiles, Patcher};
use super::spec::PatchSpec;

const MOST_CORDS: usize = 16_384;
const MOST_WORK: usize = 4 * 1024 * 1024;
const PREFIX: &str = "obj-kumi-voices-";

fn error(reason: &str) -> String {
    format!("voices: {reason}; use voices: 1 or keep the voice graph local to its MIDI input and audio output.")
}

/// Names the notation builder itself assigned, identified by their exact path,
/// box ID and source name. This entry is for freshly generated patches only;
/// arbitrary scripting names and Live parameters remain unsupported.
pub(crate) fn instrument_from_patch(document: Value, voices: u32, spec: &PatchSpec) -> Result<Value, String> {
    if voices == 1 {
        return Ok(document);
    }
    fn names(notation: &Notation, path: &str, controls: &[&str], owned: &mut HashMap<String, String>) {
        for (index, item) in notation.boxes.iter().enumerate() {
            let path = format!("{path}obj-{}", index + 1);
            if let Some(name) = item.name.as_ref().filter(|name| !controls.contains(&name.as_str())) {
                owned.insert(path.clone(), name.clone());
            }
            if let What::Patcher { inner, .. } = &item.what {
                // A nested patcher's notation has no outside face controls.
                names(inner, &format!("{path}/"), &[], owned);
            }
        }
    }
    let (notation, _) = parse(&spec.patch);
    let mut owned = HashMap::new();
    let controls = spec.controls.iter().map(|control| control.name()).collect::<Vec<_>>();
    names(&notation, "", &controls, &mut owned);
    instrument_with_names(document, voices, &owned)
}

#[cfg(test)]
fn instrument(document: Value, voices: u32) -> Result<Value, String> {
    instrument_with_names(document, voices, &HashMap::new())
}

/// One voice is exactly the original document. More voices share the face and guarded output, but no user state.
fn instrument_with_names(mut document: Value, voices: u32, owned: &HashMap<String, String>) -> Result<Value, String> {
    if voices == 1 {
        return Ok(document);
    }
    if !(2..=MAX_VOICES).contains(&voices) {
        return Err(error("the voice count must be between 1 and 32"));
    }
    let mut work = 0;
    charge(&document, 0, &mut work)?;
    let patcher = document.get_mut("patcher").and_then(Value::as_object_mut).ok_or_else(|| error("missing device patcher"))?;
    let boxes = patcher.get("boxes").and_then(Value::as_array).ok_or_else(|| error("missing boxes"))?;
    let lines = patcher.get("lines").and_then(Value::as_array).ok_or_else(|| error("missing cords"))?;
    let mut ids = HashSet::new();
    let mut user = HashSet::new();
    let mut totals = (10usize, 17usize); // The shared MIDI router, before its per-voice branches.
    let mut copied_work = 0;
    let mut extra_input_work = 0;
    for entry in boxes {
        let item = &entry["box"];
        let id = item["id"].as_str().ok_or_else(|| error("a box has no ID"))?;
        if !ids.insert(id) || id.starts_with(PREFIX) {
            return Err(error("box IDs must be unique and leave Kumi's voice IDs free"));
        }
        if id.starts_with("obj-lane-") || matches!(id, "obj-playhead" | "obj-mod") {
            return Err(error("step lanes, playhead and mappings are shared side effects"));
        }
        let cost = size(item, 0)?;
        if numeric_id(id) {
            local_box(item, false, id, owned)?;
            user.insert(id);
            totals.0 += voices as usize * cost.0;
            totals.1 += voices as usize * cost.1;
            charge(item, 0, &mut copied_work)?;
        } else {
            totals.0 += cost.0;
            totals.1 += cost.1;
        }
    }
    if !ids.contains("obj-in") || !ids.contains("obj-out") || !ids.contains("obj-plugout") {
        return Err(error("the instrument's MIDI input and guarded output are required"));
    }
    if user.is_empty() {
        return Err(error("the instrument has no voice graph"));
    }
    totals.0 += voices as usize * 6;
    totals.1 += voices as usize * 8;
    for entry in lines {
        let (from, outlet) = end(entry, "source")?;
        let (to, inlet) = end(entry, "destination")?;
        if !ids.contains(from) || !ids.contains(to) {
            return Err(error("a cord refers to a missing box"));
        }
        match (user.contains(from), user.contains(to)) {
            (true, true) => totals.1 += voices as usize,
            (false, true) if from == "obj-in" && outlet == 0 => totals.1 += 2 * voices as usize,
            (false, true) if control_id(from) => totals.1 += voices as usize,
            (true, false) if to == "obj-out" && inlet < 2 => totals.1 += voices as usize,
            (false, false) if from != "obj-in" && to != "obj-in" => totals.1 += 1,
            _ => return Err(error("a voice writes to shared controls or reaches an unsupported shared input")),
        }
        if user.contains(from) || user.contains(to) {
            charge(entry, 0, &mut copied_work)?;
            if from == "obj-in" {
                charge(entry, 0, &mut extra_input_work)?;
            }
        }
    }
    // Reserve space for generated boxes/cords and longer IDs before making copies. The final charge below also
    // checks the exact result; this conservative preflight prevents creating an oversized intermediate graph.
    let expanded_work =
        work + copied_work * (voices as usize - 1) + extra_input_work * voices as usize + 256 * totals.1 + voices as usize * 4096 + 8192;
    if totals.0 > MOST_BOXES || totals.1 > MOST_CORDS || expanded_work > MOST_WORK {
        return Err(error("the expanded graph exceeds 2000 boxes, 16384 cords or the 4 MiB construction budget"));
    }

    let rename = |id: &str, voice: u32| format!("{PREFIX}{voice}-{id}");
    let mut expanded_boxes: Vec<Value> =
        boxes.iter().filter(|entry| !user.contains(entry["box"]["id"].as_str().unwrap())).cloned().collect();
    for voice in 1..=voices {
        for entry in boxes.iter().filter(|entry| user.contains(entry["box"]["id"].as_str().unwrap())) {
            let mut copy = entry.clone();
            namespace_names(&mut copy["box"], voice, entry["box"]["id"].as_str().unwrap(), owned);
            copy["box"]["id"] = json!(rename(entry["box"]["id"].as_str().unwrap(), voice));
            expanded_boxes.push(copy);
        }
    }
    let mut expanded_lines = Vec::with_capacity(totals.1);
    // Honour explicit original cord order before adding distinct orders to each new fan-out.
    let mut original: Vec<_> = lines.iter().collect();
    original.sort_by_key(|line| line["patchline"]["order"].as_u64().unwrap_or(u64::MAX));
    for entry in original {
        let (from, _) = end(entry, "source")?;
        let (to, _) = end(entry, "destination")?;
        if !user.contains(from) && !user.contains(to) {
            expanded_lines.push(entry.clone());
            continue;
        }
        for voice in 1..=voices {
            let mut copy = entry.clone();
            if user.contains(to) {
                copy["patchline"]["destination"][0] = json!(rename(to, voice));
            }
            if user.contains(from) {
                copy["patchline"]["source"][0] = json!(rename(from, voice));
            } else if from == "obj-in" {
                copy["patchline"]["source"][0] = json!(format!("{PREFIX}{voice}-format"));
                let mut broadcast = copy.clone();
                broadcast["patchline"]["source"][0] = json!(format!("{PREFIX}broadcast"));
                expanded_lines.push(broadcast);
            }
            expanded_lines.push(copy);
        }
    }
    router(&mut expanded_boxes, &mut expanded_lines, voices);
    let mut orders: HashMap<(String, u64), u64> = HashMap::new();
    for entry in &mut expanded_lines {
        let (id, outlet) = end(entry, "source")?;
        let order = orders.entry((id.to_string(), outlet)).or_default();
        entry["patchline"]["order"] = json!(*order);
        *order += 1;
    }
    debug_assert!(expanded_boxes.len() <= totals.0 && expanded_lines.len() <= totals.1);
    patcher.insert("boxes".into(), json!(expanded_boxes));
    patcher.insert("lines".into(), json!(expanded_lines));
    let mut final_work = 0;
    charge(&document, 0, &mut final_work)?;
    let mut laid = Patcher::read(&document, &NoFiles);
    super::patch::layout::arrange(&mut laid);
    let document = laid.to_document();
    charge(&document, 0, &mut 0)?;
    Ok(document)
}

fn numeric_id(id: &str) -> bool {
    id.strip_prefix("obj-").is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit()))
}

fn control_id(id: &str) -> bool {
    id.strip_prefix("obj-control-").is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit()))
}

fn namespace_names(item: &mut Value, voice: u32, path: &str, owned: &HashMap<String, String>) {
    if let Some(name) = owned.get(path).filter(|name| item["varname"].as_str() == Some(name.as_str())) {
        item["varname"] = json!(format!("{PREFIX}{voice}-{name}"));
    }
    fn within(patcher: &mut Value, voice: u32, path: &str, owned: &HashMap<String, String>) {
        if let Some(boxes) = patcher["boxes"].as_array_mut() {
            for entry in boxes {
                if let Some(id) = entry["box"]["id"].as_str().map(str::to_owned) {
                    namespace_names(&mut entry["box"], voice, &format!("{path}/{id}"), owned);
                }
            }
        }
    }
    if let Some(patcher) = item.get_mut("patcher") {
        within(patcher, voice, path, owned);
    }
    if let Some(patcher) = item.get_mut("data").and_then(|data| data.get_mut("patcher")) {
        within(patcher, voice, path, owned);
    }
}

fn end<'a>(entry: &'a Value, which: &str) -> Result<(&'a str, u64), String> {
    let end = &entry["patchline"][which];
    match (end[0].as_str(), end[1].as_u64()) {
        (Some(id), Some(port)) => Ok((id, port)),
        _ => Err(error("an unreadable cord cannot be copied")),
    }
}

/// Charge all JSON, including code and nested metadata, before multiplying it. Recursion and work are bounded even
/// for malformed input, not just valid patchers; this also bounds later recursive validation and allocation.
fn charge(value: &Value, depth: usize, work: &mut usize) -> Result<(), String> {
    *work += 1;
    if depth > 64 || *work > MOST_WORK {
        return Err(error("the graph exceeds the construction budget"));
    }
    match value {
        Value::String(text) => *work += text.len(),
        Value::Array(items) => {
            for item in items {
                charge(item, depth + 1, work)?;
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                *work += key.len();
                charge(value, depth + 1, work)?;
            }
        }
        _ => {}
    }
    if *work > MOST_WORK {
        Err(error("the graph exceeds the construction budget"))
    } else {
        Ok(())
    }
}

fn size(item: &Value, depth: usize) -> Result<(usize, usize), String> {
    if depth > 16 {
        return Err(error("the voice graph is nested too deeply"));
    }
    let mut total = (1, 0);
    for inner in [item.get("patcher"), item.get("data").and_then(|data| data.get("patcher"))].into_iter().flatten() {
        let boxes = inner["boxes"].as_array().ok_or_else(|| error("an embedded patcher has no boxes"))?;
        total.1 += inner["lines"].as_array().ok_or_else(|| error("an embedded patcher has no cords"))?.len();
        for entry in boxes {
            let cost = size(&entry["box"], depth + 1)?;
            total.0 += cost.0;
            total.1 += cost.1;
            if total.0 > MOST_BOXES || total.1 > MOST_CORDS {
                return Err(error("an embedded graph exceeds the expansion budget"));
            }
        }
    }
    Ok(total)
}

/// This allowlist is intentionally narrower than Max's object reference. An unknown object may be an abstraction
/// with shared state or side effects; absence from the named-object catalog alone does not prove it is local.
fn local_box(item: &Value, gen: bool, path: &str, owned: &HashMap<String, String>) -> Result<(), String> {
    let name_is_foreign = match item.get("varname") {
        None => false,
        Some(Value::String(name)) => !name.is_empty() && owned.get(path) != Some(name),
        Some(_) => true,
    };
    if name_is_foreign || item.get("parameter_enable").is_some_and(|enabled| enabled.as_f64() != Some(0.0)) {
        return Err(error("a voice has a scripting name or its own Live parameter"));
    }
    let maxclass = item["maxclass"].as_str().unwrap_or_default();
    if gen && maxclass == "codebox" {
        return local_gen(item["code"].as_str().unwrap_or_default());
    }
    if !gen && maxclass == "message" {
        let text = item["text"].as_str().unwrap_or_default();
        return if text.contains(';') || names_receiver(text) {
            Err(error("a message can address shared named receivers"))
        } else {
            Ok(())
        };
    }
    if !gen && matches!(maxclass, "inlet" | "outlet" | "comment") {
        return Ok(());
    }
    if maxclass != "newobj" {
        return Err(error(&format!("[{maxclass}] is not a supported local voice object")));
    }
    let text = item["text"].as_str().unwrap_or_default();
    let args = atoms(text);
    let class = args.first().map(String::as_str).unwrap_or_default();
    let class = catalog().canonical(class);
    if names_receiver(text) || args.iter().any(|arg| matches!(arg.as_str(), "@varname" | "@parameter_enable" | "@bindto" | "@name")) {
        return Err(error("a voice object declares a shared name or parameter"));
    }
    if gen {
        return if matches!(class, "in" | "out") { Ok(()) } else { Err(error("only inline GenExpr is supported inside Gen voices")) };
    }
    if matches!(class, "patcher" | "gen" | "gen~" | "mc.gen~" | "mcs.gen~") {
        // A filename/abstraction argument is not an embedded local graph, even when a patcher is also present.
        if (class != "patcher" && args.len() != 1)
            || (class == "patcher" && (args.len() > 2 || args.iter().any(|arg| arg.starts_with('@'))))
        {
            return Err(error("a Gen voice names an external abstraction"));
        }
        let inner = item
            .get("patcher")
            .or_else(|| item.get("data").and_then(|data| data.get("patcher")))
            .ok_or_else(|| error("a voice names an external patcher or Gen abstraction"))?;
        let inner_gen = class != "patcher";
        for entry in inner["boxes"].as_array().ok_or_else(|| error("an embedded voice has no boxes"))? {
            let id = entry["box"]["id"].as_str().ok_or_else(|| error("an embedded voice has no box ID"))?;
            local_box(&entry["box"], inner_gen, &format!("{path}/{id}"), owned)?;
        }
        return Ok(());
    }
    if catalog().names_something(class) || catalog().midi_out(class) || !LOCAL_CLASSES.split_whitespace().any(|allowed| allowed == class) {
        return Err(error(&format!("[{class}] may share state, run external code or perform a side effect")));
    }
    if item.get("patcher").is_some() || item.get("data").and_then(|data| data.get("patcher")).is_some() {
        return Err(error("an unsupported object carries an embedded patcher"));
    }
    // cycle~'s optional symbol argument names a shared buffer. Numeric oscillators remain independent.
    if class == "cycle~" && args.iter().skip(1).any(|arg| arg.parse::<f64>().is_err()) {
        return Err(error("[cycle~] must use its local numeric wavetable, without a named buffer or attributes"));
    }
    Ok(())
}

fn names_receiver(text: &str) -> bool {
    // Even an otherwise local [int] accepts "send receiver". Do not multiply explicit indirect sends either.
    atoms(text).iter().any(|atom| matches!(atom.as_str(), "send" | "forward" | "script"))
}

const LOCAL_CLASSES: &str = "
    abs acos acosh asin asinh atan atan2 atanh bangbang buddy cartopol change clip cos cosh counter dbtoa
    decode defer deferlow delay expr float funnel gate if int iter join line listfunnel loadbang loadmess
    match maximum minimum mtof ftom onebang pack pak peak poltocar pow prepend random round route routepass
    scale select sin sinh speedlim split sqrt stripnote switch tan tanh thresh timer togEdge
    trigger trunc unpack uzi zl zl.change zl.ecils zl.filter zl.group zl.iter zl.join zl.lace zl.len zl.lookup
    zl.mth zl.nth zl.queue zl.reg zl.rev zl.rot zl.scramble zl.sect zl.slice zl.sort zl.stack zl.stream zl.sum
    zl.thin zl.union zl.unique zlclear
    + - * / % !/ !- == != > < >= <= && || ! & | ^ << >>
    +~ -~ *~ /~ %~ !/~ !-~ ==~ !=~ >~ <~ >=~ <=~ abs~ acos~ acosh~ allpass~ asin~ asinh~ atan~ atan2~ atanh~
    biquad~ buffir~ cartopol~ change~ click~ clip~ comb~ cos~ cosh~ count~ cross~ cycle~ dbtoa~ delta~ edge~
    filtercoeff~ fourpole~ ftom~ hip~ kink~ line~ lores~ maximum~ minimum~ minmax~ mtof~ noise~ onepole~
    overdrive~ peakamp~ phasor~ pink~ poltocar~ pow~ rampsmooth~ rand~ reson~ round~ sah~ scale~ selector~
    sig~ sin~ sinh~ slide~ snapshot~ sqrt~ svf~ tan~ tanh~ thresh~ train~ trapezoid~ triangle~ trunc~ zerox~
    midiparse midiformat unpacknote makenote flush poly bag borax
";

fn local_gen(code: &str) -> Result<(), String> {
    // Strings are where Gen imports and externally named buffers enter. Local Data/History/Delay and ordinary
    // arithmetic have no strings. Refusing unfamiliar functions also prevents an implicit library dependency.
    if code.contains(['\'', '"']) {
        return Err(error("Gen voices cannot import code or name external buffers"));
    }
    let words: Vec<_> = code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).collect();
    if words.iter().any(|word| matches!(*word, "Buffer" | "buffer" | "require" | "include" | "import" | "export" | "setbuffer")) {
        return Err(error("Gen voices cannot import code or name external buffers"));
    }
    // GenExpr functions may be declared locally. A call is safe only if its name is a built-in or one of those
    // declarations; the notation's compiler remains responsible for syntax/type checking in Max.
    let call = regex::Regex::new(r"([A-Za-z_][A-Za-z_0-9]*)\s*\([^()]*\)\s*\{").unwrap();
    let mut declared: HashSet<_> = call.captures_iter(code).map(|capture| capture[1].to_string()).collect();
    declared.extend(super::gen::names(code).declared.into_keys());
    let call = regex::Regex::new(r"([A-Za-z_][A-Za-z_0-9]*)\s*\(").unwrap();
    for capture in call.captures_iter(code) {
        let name = &capture[1];
        if !GEN_FUNCTIONS.split_whitespace().any(|allowed| allowed == name) && !declared.contains(name) {
            return Err(error(&format!("Gen function [{name}] is not a verified local voice operation")));
        }
    }
    Ok(())
}

const GEN_FUNCTIONS: &str = "
    Param History Delay Data if for while abs acos acosh asin asinh atan atan2 atanh bool ceil clamp clip cos cosh
    cycle dcblock delta exp exp2 floor fold fract frequency ftom mtof log log2 log10 max min mix noise phasor
    pow round samplerate sin sinh smoothstep sqrt step tan tanh trunc triangle wrap peek poke splat sample
    read write reset size dim channels fixdenorm fixnan isnan isinf sign slide latch sah change counter accum
";

fn object(boxes: &mut Vec<Value>, suffix: &str, text: &str, ins: usize, outs: usize) {
    boxes.push(json!({"box": {"id": format!("{PREFIX}{suffix}"), "maxclass": "newobj", "text": text,
        "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs], "patching_rect": [40., 40., 120., 22.]}}));
}

fn cord(lines: &mut Vec<Value>, from: &str, outlet: usize, to: &str, inlet: usize) {
    lines.push(json!({"patchline": {"source": [format!("{PREFIX}{from}"), outlet], "destination": [format!("{PREFIX}{to}"), inlet]}}));
}

fn router(boxes: &mut Vec<Value>, lines: &mut Vec<Value>, voices: u32) {
    // C74's poly reference: nonzero second argument turns off the oldest voice before putting its new note there.
    // Its right-to-left outputs feed cold pack elements before the voice number triggers the completed list.
    object(boxes, "raw-order", "t i i", 1, 2);
    object(boxes, "status", "split 128 239", 3, 2);
    object(boxes, "channel-mod", "% 16", 2, 1);
    object(boxes, "channel", "+ 1", 2, 1);
    object(boxes, "parse", "midiparse @hires 2", 1, 8);
    object(boxes, "poly", &format!("poly {voices} 1"), 2, 3);
    object(boxes, "pack", "pack 0 0 0", 3, 1);
    object(
        boxes,
        "route",
        &format!("route {}", (1..=voices).map(|voice| voice.to_string()).collect::<Vec<_>>().join(" ")),
        1,
        voices as usize + 1,
    );
    object(boxes, "pressure", "zl.rev", 2, 2);
    object(boxes, "broadcast", "midiformat @hires 2", 7, 2);
    lines.push(json!({"patchline": {"source": ["obj-in", 0], "destination": [format!("{PREFIX}raw-order"), 0]}}));
    cord(lines, "raw-order", 1, "status", 0);
    cord(lines, "raw-order", 0, "parse", 0);
    cord(lines, "status", 0, "channel-mod", 0);
    cord(lines, "channel-mod", 0, "channel", 0);
    cord(lines, "channel", 0, "broadcast", 6);
    cord(lines, "parse", 0, "poly", 0);
    for port in 0..3 {
        cord(lines, "poly", port, "pack", port);
    }
    cord(lines, "pack", 0, "route", 0);
    // midiparse emits key,pressure; midiformat expects pressure,key. Other channel messages have matching ports.
    cord(lines, "parse", 1, "pressure", 0);
    cord(lines, "pressure", 0, "broadcast", 1);
    for port in 2..6 {
        cord(lines, "parse", port, "broadcast", port);
    }
    for voice in 1..=voices {
        let id = |tail: &str| format!("{voice}-{tail}");
        object(boxes, &id("order"), "t l l", 1, 2);
        object(boxes, &id("unpack"), "unpack 0 0", 1, 2);
        object(boxes, &id("positive"), "> 0", 2, 1);
        object(boxes, &id("played"), "sel 1", 2, 2);
        object(boxes, &id("channel"), "i 1", 2, 1);
        object(boxes, &id("format"), "midiformat @hires 2", 7, 2);
        cord(lines, "channel", 0, &id("channel"), 1);
        cord(lines, "route", voice as usize - 1, &id("order"), 0);
        cord(lines, &id("order"), 1, &id("unpack"), 0);
        cord(lines, &id("unpack"), 1, &id("positive"), 0);
        cord(lines, &id("positive"), 0, &id("played"), 0);
        cord(lines, &id("played"), 0, &id("channel"), 0);
        cord(lines, &id("channel"), 0, &id("format"), 6);
        cord(lines, &id("order"), 0, &id("format"), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_assigned_by_the_fresh_notation_builder_are_accepted_and_scoped_per_voice() {
        use super::super::{
            patched::patched_device,
            spec::{check_spec, DeviceSpec},
        };
        let patch = "in -> [midiparse] -> pair = [unpack 0 0]\npair -> (note $1) -> sound = [gen~] { Param note(60); Param velocity(0); out1 = cycle(mtof(note)) * velocity / 127; out2 = out1; }\npair.1 -> (velocity $1) -> sound\nsound -> out\nsound.1 -> out.1\n";
        let (DeviceSpec::Patch(spec), _) = check_spec(
            json!({ "type": "instrument", "name": "Named voice", "about": "One local voice.", "patch": patch, "voices": 2 })
                .as_object()
                .unwrap(),
        ) else {
            panic!("a patched instrument")
        };
        let (document, _) = patched_device(&spec, None);
        assert!(instrument(document.clone(), 2).is_err(), "an arbitrary named document stays refused without builder provenance");
        let poly = instrument_from_patch(document.clone(), 2, &spec).unwrap();
        for voice in 1..=2 {
            for name in ["pair", "sound"] {
                assert!(poly["patcher"]["boxes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["box"]["varname"] == format!("{PREFIX}{voice}-{name}")));
            }
        }
        for (field, value) in
            [("varname", json!("foreign-name")), ("parameter_enable", json!(1.0)), ("text", json!("unpack 0 0 @varname foreign-name"))]
        {
            let mut foreign = document.clone();
            let pair =
                foreign["patcher"]["boxes"].as_array_mut().unwrap().iter_mut().find(|entry| entry["box"]["varname"] == "pair").unwrap();
            pair["box"][field] = value;
            assert!(instrument_from_patch(foreign, 2, &spec).is_err(), "{field} must remain refused");
        }
        assert_eq!(instrument_from_patch(document.clone(), 1, &spec).unwrap(), document, "mono remains an exact identity");
    }

    fn box_(id: &str, text: &str) -> Value {
        json!({"box": {"id": id, "maxclass": "newobj", "text": text, "numinlets": 2,
            "numoutlets": 2, "patching_rect": [40., 40., 80., 22.]}})
    }

    fn wire(from: &str, outlet: usize, to: &str, inlet: usize) -> Value {
        json!({"patchline": {"source": [from, outlet], "destination": [to, inlet]}})
    }

    fn mono() -> Value {
        let guard =
            serde_json::to_value(super::super::gen::gen_box("obj-out", super::super::gen::OUTPUT_STAGE_INSTRUMENT, [40., 300., 300., 22.]))
                .unwrap();
        json!({"patcher": {"description": "one face", "boxes": [
            box_("obj-in", "midiin"), guard, box_("obj-plugout", "plugout~ 1 2"),
            {"box": {"id": "obj-control-1", "maxclass": "live.dial", "parameter_enable": 1, "varname": "Level"}},
            box_("obj-1", "midiparse"), box_("obj-2", "unpack 0 0"), box_("obj-3", "mtof"),
            box_("obj-4", "cycle~"), box_("obj-5", "*~ 0.1")
        ], "lines": [
            wire("obj-in", 0, "obj-1", 0), wire("obj-1", 0, "obj-2", 0),
            wire("obj-2", 0, "obj-3", 0), wire("obj-3", 0, "obj-4", 0), wire("obj-4", 0, "obj-5", 0),
            wire("obj-control-1", 0, "obj-5", 1), wire("obj-5", 0, "obj-out", 0), wire("obj-5", 0, "obj-out", 1),
            wire("obj-out", 0, "obj-plugout", 0), wire("obj-out", 1, "obj-plugout", 1)
        ]}})
    }

    fn item<'a>(document: &'a Value, id: &str) -> &'a Value {
        &document["patcher"]["boxes"].as_array().unwrap().iter().find(|entry| entry["box"]["id"] == id).unwrap()["box"]
    }

    fn wired(document: &Value, from: &str, outlet: usize, to: &str, inlet: usize) -> bool {
        document["patcher"]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["patchline"]["source"] == json!([from, outlet]) && entry["patchline"]["destination"] == json!([to, inlet]))
    }

    #[test]
    fn mono_is_the_exact_original_even_when_polyphony_would_refuse_it() {
        let original = json!({"an": ["opaque", "document"]});
        assert_eq!(instrument(original.clone(), 1).unwrap(), original);
        assert!(instrument(mono(), 0).is_err());
        assert!(instrument(mono(), MAX_VOICES + 1).is_err());
    }

    #[test]
    fn separate_voices_share_only_the_face_and_the_final_guard() {
        let original = mono();
        let poly = instrument(original.clone(), 3).unwrap();
        assert_eq!(item(&poly, "obj-out")["text"], item(&original, "obj-out")["text"]);
        assert_eq!(
            item(&poly, "obj-out")["patcher"]["boxes"][0]["box"]["code"],
            item(&original, "obj-out")["patcher"]["boxes"][0]["box"]["code"]
        );
        assert_eq!(item(&poly, "obj-control-1"), item(&original, "obj-control-1"));
        assert_eq!(item(&poly, &format!("{PREFIX}poly"))["text"], "poly 3 1");
        assert_eq!(item(&poly, &format!("{PREFIX}route"))["text"], "route 1 2 3");
        let boxes = poly["patcher"]["boxes"].as_array().unwrap();
        for id in ["obj-in", "obj-out", "obj-plugout", "obj-control-1"] {
            assert_eq!(boxes.iter().filter(|entry| entry["box"]["id"] == id).count(), 1);
        }
        for voice in 1..=3 {
            let id = |tail: &str| format!("{PREFIX}{voice}-{tail}");
            assert!(wired(&poly, &id("format"), 0, &id("obj-1"), 0));
            assert!(wired(&poly, &format!("{PREFIX}broadcast"), 0, &id("obj-1"), 0));
            assert!(!wired(&poly, "obj-in", 0, &id("obj-1"), 0));
            assert!(wired(&poly, &id("obj-4"), 0, &id("obj-5"), 0));
            assert!(wired(&poly, "obj-control-1", 0, &id("obj-5"), 1));
            assert!(wired(&poly, &id("obj-5"), 0, "obj-out", 0));
            assert!(wired(&poly, &id("obj-5"), 0, "obj-out", 1));
            for other in 1..=3 {
                if other != voice {
                    assert!(!wired(&poly, &id("obj-4"), 0, &format!("{PREFIX}{other}-obj-5"), 0));
                }
            }
        }
        assert!(wired(&poly, "obj-in", 0, &format!("{PREFIX}raw-order"), 0));
        assert!(wired(&poly, &format!("{PREFIX}raw-order"), 1, &format!("{PREFIX}status"), 0));
        assert!(wired(&poly, &format!("{PREFIX}raw-order"), 0, &format!("{PREFIX}parse"), 0));
        assert!(!wired(&poly, &format!("{PREFIX}parse"), 6, &format!("{PREFIX}broadcast"), 6));
        assert!(wired(&poly, &format!("{PREFIX}parse"), 1, &format!("{PREFIX}pressure"), 0));
        assert!(wired(&poly, &format!("{PREFIX}pressure"), 0, &format!("{PREFIX}broadcast"), 1));
        assert_eq!(item(&poly, &format!("{PREFIX}broadcast"))["text"], "midiformat @hires 2");
    }

    #[test]
    fn nested_inline_state_is_copied_but_named_state_and_external_code_are_refused() {
        let mut original = mono();
        original["patcher"]["boxes"][5]["box"] = json!({"id": "obj-2", "maxclass": "newobj", "text": "p local",
            "patcher": {"boxes": [serde_json::to_value(super::super::gen::gen_box("local-gen", "History phase(0); phase = wrap(phase + in1); out1 = phase; out2 = phase;", [0., 0., 80., 22.])).unwrap()], "lines": []}});
        let mut poly = instrument(original.clone(), 2).unwrap();
        let first = poly["patcher"]["boxes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["box"]["id"] == format!("{PREFIX}1-obj-2"))
            .unwrap();
        first["box"]["patcher"]["boxes"][0]["box"]["patcher"]["boxes"][0]["box"]["code"] = json!("changed");
        assert_eq!(
            item(&poly, &format!("{PREFIX}2-obj-2"))["patcher"]["boxes"][0]["box"]["patcher"]["boxes"][0]["box"]["code"],
            item(&original, "obj-2")["patcher"]["boxes"][0]["box"]["patcher"]["boxes"][0]["box"]["code"]
        );
        for text in
            ["s ---shared", "receive #0-shared", "v8", "live.object", "external-voice", "mpeparse", "gen~ external", "cycle~ shared-buffer"]
        {
            let mut unsupported = mono();
            unsupported["patcher"]["boxes"][4] = box_("obj-1", text);
            assert!(instrument(unsupported, 2).is_err(), "{text}");
        }
        original["patcher"]["boxes"][5]["box"]["patcher"]["boxes"] = json!([box_("nested", "send shared")]);
        assert!(instrument(original, 2).is_err(), "a name inside a p is shared too");
        for code in ["Buffer b(\"shared\"); out1 = peek(b, in1);", "require external; out1 = in1;", "out1 = external(in1);"] {
            assert!(local_gen(code).is_err(), "{code}");
        }
    }

    #[test]
    fn shared_side_effects_and_expansion_are_checked_before_copying() {
        let mut patch = mono();
        patch["patcher"]["lines"].as_array_mut().unwrap().push(wire("obj-2", 1, "obj-control-1", 0));
        assert!(instrument(patch, 2).is_err());
        let mut patch = mono();
        patch["patcher"]["boxes"].as_array_mut().unwrap().push(box_("obj-playhead", "p playhead"));
        assert!(instrument(patch, 2).is_err());
        let mut patch = mono();
        patch["patcher"]["boxes"][4]["box"] = json!({"id": "obj-1", "maxclass": "message", "text": "; shared bang"});
        assert!(instrument(patch, 2).is_err());
        let mut patch = mono();
        patch["patcher"]["boxes"][4]["box"] = json!({"id": "obj-1", "maxclass": "newobj", "text": "p local",
            "patcher": {"boxes": (0..100).map(|n| box_(&format!("nested-{n}"), "+ 1")).collect::<Vec<_>>(), "lines": []}});
        assert!(instrument(patch, 32).unwrap_err().contains("2000 boxes"), "nested boxes count towards the total");
        let mut patch = mono();
        patch["patcher"]["lines"] = json!((0..600).map(|_| wire("obj-2", 0, "obj-3", 0)).collect::<Vec<_>>());
        assert!(instrument(patch, 32).unwrap_err().contains("16384 cords"));
        let mut patch = mono();
        patch["patcher"]["boxes"][4]["box"] = json!({"id": "obj-1", "maxclass": "message", "text": "x".repeat(200_000)});
        assert!(instrument(patch, 32).unwrap_err().contains("construction budget"));
    }

    /// Exercise the *generated cords*, starting at poly's documented routed output, without pretending to run Max
    /// or deciding which repeated-pitch note poly chooses. Only the small channel-before-note branch is interpreted.
    struct Delivery {
        patch: Value,
        pending: HashMap<String, i64>,
        channels: HashMap<String, i64>,
        notes: Vec<(String, [i64; 3])>,
    }

    impl Delivery {
        fn send(&mut self, from: &str, outlet: usize, message: Vec<i64>) {
            let targets: Vec<_> = self.patch["patcher"]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["patchline"]["source"] == json!([from, outlet]))
                .map(|entry| {
                    (
                        entry["patchline"]["destination"][0].as_str().unwrap().to_string(),
                        entry["patchline"]["destination"][1].as_u64().unwrap() as usize,
                    )
                })
                .collect();
            for (to, inlet) in targets {
                self.receive(&to, inlet, message.clone());
            }
        }

        fn receive(&mut self, to: &str, inlet: usize, message: Vec<i64>) {
            let text = item(&self.patch, to)["text"].as_str().unwrap().to_string();
            match text.as_str() {
                "t l l" => {
                    self.send(to, 1, message.clone());
                    self.send(to, 0, message);
                }
                "unpack 0 0" => {
                    self.send(to, 1, vec![message[1]]);
                    self.send(to, 0, vec![message[0]]);
                }
                "> 0" => self.send(to, 0, vec![i64::from(message[0] > 0)]),
                "sel 1" if message == [1] => self.send(to, 0, vec![]),
                "sel 1" => {}
                "i 1" if inlet == 1 => {
                    self.pending.insert(to.into(), message[0]);
                }
                "i 1" => self.send(to, 0, vec![self.pending.get(to).copied().unwrap_or(1)]),
                "midiformat @hires 2" if inlet == 6 => {
                    self.channels.insert(to.into(), message[0]);
                }
                "midiformat @hires 2" if inlet == 0 => {
                    let channel = self.channels.get(to).copied().unwrap_or(1);
                    self.notes.push((to.into(), [0x90 + channel - 1, message[0], message[1]]));
                }
                "midiformat @hires 2" => {}
                _ => panic!("unhandled delivery object {text}"),
            }
        }
    }

    #[test]
    fn offs_keep_the_owning_voices_channel_and_steals_switch_channel_only_for_the_new_note() {
        let mut delivered =
            Delivery { patch: instrument(mono(), 2).unwrap(), pending: HashMap::new(), channels: HashMap::new(), notes: vec![] };
        let channel = format!("{PREFIX}channel");
        let route = format!("{PREFIX}route");
        delivered.send(&channel, 0, vec![2]);
        delivered.send(&route, 0, vec![60, 100]);
        delivered.send(&channel, 0, vec![5]);
        delivered.send(&route, 1, vec![60, 90]);
        // poly is pitch-owned: whatever voice it selects for an off, that voice must retain its note's channel.
        delivered.send(&route, 0, vec![60, 0]);
        // Its documented steal emits an old off before the new positive note on the same routed voice.
        delivered.send(&channel, 0, vec![10]);
        delivered.send(&route, 1, vec![60, 0]);
        delivered.send(&route, 1, vec![67, 80]);
        delivered.send(&channel, 0, vec![16]);
        delivered.send(&route, 1, vec![67, 0]);
        let notes: Vec<_> = delivered.notes.iter().map(|(_, bytes)| *bytes).collect();
        assert_eq!(notes, [[0x91, 60, 100], [0x94, 60, 90], [0x91, 60, 0], [0x94, 60, 0], [0x99, 67, 80], [0x99, 67, 0]]);
    }
}
