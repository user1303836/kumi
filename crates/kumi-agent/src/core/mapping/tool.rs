//! The map_samples tool, the same in every DAW: plan a set of samples across the keys (files, a folder, or an instrument's own
//! samples), say what was decided and ask only what can't be told, then make it as one change per instrument; or look over the
//! song's instruments for a tidy. Notes are named and read as the DAW's profile writes them; the DAW's backend reads, hears and
//! writes.
use super::backend::{Held, MappingBackend};
use super::{heard::hear_file, names, plan, Asked, Layout, Plan, Planned, Sample, MIDDLE_CS, MOST_SAMPLES, PLAIN};
use async_trait::async_trait;
use kumi_common::abort::{Signal, SignalExt};
use kumi_common::js::json::stringify;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::daw::DawProfile;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::rc::Rc;

pub const TOOL: &str = "map_samples";

pub struct Mapping<B: MappingBackend> {
    backend: B,
    description: String,
    schema: JsonObject,
}

impl<B: MappingBackend + 'static> Mapping<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let words = backend.words();
        Rc::new(Mapping { description: description(words), schema: schema(words), backend })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(MappingTool { owner: self.clone() })
    }

    fn profile(&self) -> &'static DawProfile {
        self.backend.words().profile
    }

    fn note(&self, midi: i32) -> String {
        self.profile().note_name(midi.clamp(0, 127) as u8)
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("plan");
        match action {
            "tidy" => return self.tidy(&signal).await,
            "plan" | "apply" => {}
            _ => return Err("action is plan (the default), apply or tidy".into()),
        }
        let instrument = input.get("instrumentRef").and_then(Value::as_str).map(str::to_owned);
        let mut unheard = Vec::new();
        let (samples, held, folder) = match &instrument {
            Some(reference) => {
                if input.contains_key("files") || input.contains_key("folder") {
                    return Err("give files, a folder or instrumentRef, one of them".into());
                }
                let (_, held) = self.backend.samples_of(reference, MOST_SAMPLES, &signal).await?;
                (self.hear_held(&held, &mut unheard, &signal).await?, held, None)
            }
            None => {
                let files = files_of(&input)?;
                (
                    self.hear_files(&files, &mut unheard, &signal).await?,
                    Vec::new(),
                    input.get("folder").and_then(Value::as_str).map(str::to_owned),
                )
            }
        };
        let asked = self.asked(&input, &samples)?;
        let mut plan = plan(&samples, &asked, &|midi| self.note(midi));
        plan.findings.extend(unheard);
        let id = plan_id(&plan);
        let mut reply = self.plan_json(&plan);
        reply["planId"] = json!(id);
        if action == "plan" {
            reply["next"] = json!(if plan.instruments.is_empty() {
                "nothing to make yet: give what leftOut asks".to_string()
            } else {
                format!(
                    "action apply with plan \"{id}\" makes {} as planned, one change each",
                    count(plan.instruments.len(), self.backend.words().instrument)
                )
            });
            return Ok(reply);
        }
        // (what the producer accepted is the plan they saw: one that came out otherwise isn't made)
        if let Some(seen) = input.get("plan").and_then(Value::as_str) {
            if seen != id {
                return Err(format!(
                    "this plan isn't the one shown ({seen}): something it was planned from changed. Show this one: {}",
                    stringify(&reply)
                ));
            }
        }
        if plan.instruments.is_empty() {
            return Err(format!("nothing to map: {}", stringify(&reply["leftOut"])));
        }
        let drums = plan.layout == Layout::Drums;
        let mut made = Vec::new();
        match &instrument {
            Some(reference) => {
                let planned = self.planned_for_remap(&plan, input.get("part").and_then(Value::as_str))?;
                made.push(self.backend.remap(reference, planned, &signal).await?);
                let still = self.still_sounding(&held, planned);
                if !still.is_empty() {
                    reply["stillSounding"] = json!(still);
                    reply["note"] = json!(format!(
                        "these samples weren't laid out and still play inside the new zones ({}): propose moving them aside or deleting them",
                        if planned.cycle { "as round robins of the new ones" } else { "on top of the new ones" }
                    ));
                }
                if plan.instruments.len() > 1 {
                    reply["otherParts"] = json!(format!(
                        "{} stay as they were: make each its own {} first (a copy of this one), then lay it out with its part",
                        plan.instruments
                            .iter()
                            .filter(|other| !std::ptr::eq(*other, planned))
                            .filter_map(|other| other.part.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                        self.backend.words().instrument
                    ));
                }
            }
            None => {
                let base = match input.get("name").and_then(Value::as_str) {
                    Some(name) => name.to_owned(),
                    None => common_name(&samples, folder.as_deref(), drums),
                };
                for planned in &plan.instruments {
                    let name = match &planned.part {
                        Some(part) if part != PLAIN => format!("{base} \u{b7} {part}"),
                        _ => base.clone(),
                    };
                    match self.backend.make(planned, &name, drums, &signal).await {
                        Ok(answer) => made.push(answer),
                        Err(why) if made.is_empty() => return Err(why),
                        Err(why) => {
                            made.push(json!({"error": why}));
                            break;
                        }
                    }
                }
            }
        }
        reply["made"] = Value::Array(made);
        Ok(reply)
    }

    /// The planned instrument an instrument's own samples become: the one there is, or the part asked for.
    fn planned_for_remap<'a>(&self, plan: &'a Plan, part: Option<&str>) -> Result<&'a Planned, String> {
        let instrument = self.backend.words().instrument;
        match (plan.instruments.len(), part) {
            (1, _) => Ok(&plan.instruments[0]),
            (_, Some(part)) => plan.instruments.iter().find(|planned| planned.part.as_deref() == Some(part)).ok_or_else(|| format!("part is one of {}", parts(plan))),
            _ => Err(format!(
                "these samples are {} parts ({}): in one {instrument} they'd play over each other; say which to lay out (part), and make the others {instrument}s of their own",
                plan.instruments.len(),
                parts(plan)
            )),
        }
    }

    /// The instrument's samples a remap leaves where they were that still sound in the new zones (keys and velocities).
    fn still_sounding(&self, held: &[Held], planned: &Planned) -> Vec<String> {
        let laid: BTreeSet<&str> = planned.samples.iter().map(|placed| placed.id.as_str()).collect();
        // (a sample on the note-off layer plays as a key lets go, never with the new zones' notes: it was left on purpose)
        held.iter()
            .filter(|sample| !laid.contains(sample.reference.as_str()) && !sample.note_off)
            .filter(|sample| {
                planned.samples.iter().any(|placed| {
                    sample.zone.0 <= placed.zone.1
                        && placed.zone.0 <= sample.zone.1
                        && sample.velocity.0 <= placed.velocity.1
                        && placed.velocity.0 <= sample.velocity.1
                })
            })
            .map(|sample| format!("{} ({} to {})", quoted(&sample.name), self.note(sample.zone.0), self.note(sample.zone.1)))
            .collect()
    }

    /// Each file heard (a file that can't be is mapped by its name alone, and said).
    async fn hear_files(&self, paths: &[String], unheard: &mut Vec<String>, signal: &Signal) -> Result<Vec<Sample>, String> {
        let mut samples = Vec::new();
        for path in paths {
            signal.check().map_err(|_| "stopped".to_string())?;
            let stem = Path::new(path).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
            let heard = match hear_file(path, signal).await {
                Ok(heard) => Some(heard),
                Err(why) => {
                    unheard.push(format!("{} couldn't be heard ({why}): placed by its name alone", quoted(&stem)));
                    None
                }
            };
            samples.push(Sample::new(path.clone(), stem, heard));
        }
        fix_dynamics(&mut samples);
        Ok(samples)
    }

    async fn hear_held(&self, held: &[Held], unheard: &mut Vec<String>, signal: &Signal) -> Result<Vec<Sample>, String> {
        if held.is_empty() {
            return Err(format!("that {} has no samples to lay out (slices follow their markers)", self.backend.words().instrument));
        }
        let mut samples = Vec::new();
        for sample in held {
            signal.check().map_err(|_| "stopped".to_string())?;
            let heard = match &sample.file {
                Ok(file) => match hear_file(file, signal).await {
                    Ok(heard) => Some(heard),
                    Err(why) => {
                        unheard.push(format!("{} couldn't be heard ({why}): placed by its name alone", quoted(&sample.name)));
                        None
                    }
                },
                Err(why) => {
                    unheard.push(format!("{} couldn't be heard ({why}): placed by its name alone", quoted(&sample.name)));
                    None
                }
            };
            let mut planned = Sample::new(sample.reference.clone(), sample.name.clone(), heard);
            planned.transpose = sample.transpose;
            planned.note_off = sample.note_off;
            samples.push(planned);
        }
        fix_dynamics(&mut samples);
        if let Some(moved) = samples.iter().find(|sample| sample.transpose != 0) {
            unheard.push(format!(
                "{} {} transposed: base notes allow for it, so each sounds as recorded at its root",
                quoted(&moved.name),
                if samples.iter().filter(|sample| sample.transpose != 0).count() > 1 { "and others are" } else { "is" }
            ));
        }
        Ok(samples)
    }

    fn asked(&self, input: &JsonObject, samples: &[Sample]) -> Result<Asked, String> {
        let words = self.backend.words();
        let layout = match input.get("layout").and_then(Value::as_str) {
            None => None,
            Some("keys") => Some(Layout::Keys),
            Some("drumkit") => Some(Layout::Drums),
            Some(_) => return Err("layout is keys (a multi-sample) or drumkit (a key per sound)".into()),
        };
        let middle_c = match input.get("middleC").and_then(Value::as_str) {
            None => None,
            Some(text) => Some(
                text.strip_prefix('C')
                    .and_then(|octave| octave.parse::<i32>().ok())
                    .filter(|octave| MIDDLE_CS.contains(octave))
                    .ok_or("middleC is C3, C4 or C5: the octave the names give middle C")?,
            ),
        };
        let mut roots = BTreeMap::new();
        for given in input.get("roots").and_then(Value::as_array).into_iter().flatten() {
            let named = given.get("sample").and_then(Value::as_str).unwrap_or("");
            let sample = samples
                .iter()
                .find(|s| s.name == named || s.id == named || Path::new(&s.id).file_name().is_some_and(|f| f.to_string_lossy() == named))
                .ok_or_else(|| format!("roots: no sample is {named}"))?;
            let note = given
                .get("note")
                .and_then(|note| note_of(note, words.profile))
                .ok_or_else(|| format!("roots: a note is {}", notes_words(words.profile)))?;
            roots.insert(sample.id.clone(), note);
        }
        Ok(Asked {
            layout,
            middle_c,
            roots,
            gm: input.get("gm").and_then(Value::as_bool).unwrap_or(false),
            lowest: words.lowest,
            highest: words.highest,
            first_key: words.first_key,
        })
    }

    fn plan_json(&self, plan: &Plan) -> Value {
        let note = |midi: i32| self.note(midi);
        let instruments: Vec<Value> = plan
            .instruments
            .iter()
            .map(|planned| {
                let samples: Vec<Value> = planned
                    .samples
                    .iter()
                    .map(|placed| {
                        let mut row = json!({"sample": placed.name, "root": note(placed.root), "by": placed.found.word()});
                        if placed.zone.0 != placed.zone.1 {
                            row["zone"] = json!([note(placed.zone.0), note(placed.zone.1)]);
                        }
                        if placed.velocity != (0, 127) {
                            row["velocity"] = json!([placed.velocity.0, placed.velocity.1]);
                        }
                        if let Some(robin) = placed.round_robin {
                            row["roundRobin"] = json!(robin);
                        }
                        row
                    })
                    .collect();
                let mut row = json!({"samples": samples});
                if let Some(part) = &planned.part {
                    row["part"] = json!(part);
                }
                if planned.cycle {
                    row["overlap"] = json!("cycle: round robins take turns");
                }
                row
            })
            .collect();
        let mut reply = json!({
            "layout": match plan.layout { Layout::Keys => "keys", Layout::Drums => "drumkit" },
            "instruments": instruments,
        });
        if let Some(convention) = &plan.convention {
            reply["middleC"] = json!(format!("C{} ({})", convention.middle_c, convention.how));
        }
        if !plan.findings.is_empty() {
            reply["decided"] = json!(plan.findings);
        }
        if !plan.questions.is_empty() {
            reply["ask"] = json!(plan.questions);
        }
        if !plan.left_out.is_empty() {
            reply["leftOut"] = Value::Array(plan.left_out.iter().map(|(sample, why)| json!({"sample": sample, "why": why})).collect());
        }
        reply
    }

    /// The song's instruments looked over: ones no note plays, samples two instruments hold, and instruments that are copies.
    async fn tidy(&self, signal: &Signal) -> Result<Value, String> {
        let listed = self.backend.instruments(signal).await?;
        Ok(tidy_of(&listed, self.backend.words().instrument))
    }
}

/// A short id of a plan: its layout, and each planned instrument's part, overlap and samples' places. The same input planned
/// from the same samples gives the same id.
fn plan_id(plan: &Plan) -> String {
    let mut text = format!("{:?}|", plan.layout);
    for planned in &plan.instruments {
        text.push_str(&format!("{:?}{};", planned.part, planned.cycle));
        for placed in &planned.samples {
            text.push_str(&format!(
                "{}:{}:{}:{}-{}:{}-{}:{:?},",
                placed.id, placed.root, placed.base, placed.zone.0, placed.zone.1, placed.velocity.0, placed.velocity.1, placed.round_robin
            ));
        }
    }
    let sum = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |sum, byte| (sum ^ byte as u64).wrapping_mul(0x0100_0000_01b3));
    format!("m{:08x}", sum >> 32)
}

/// A note the model wrote, as MIDI: a number (the DAW's own, middle C 60), or a name as the DAW writes it ("C-5", "C#4",
/// "Db4"), its octave read as the profile numbers octaves.
pub fn note_of(value: &Value, profile: &DawProfile) -> Option<i32> {
    use regex::Regex;
    use std::sync::LazyLock;
    static NOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Ga-g])(#|b|-)?(-?[0-9]{1,2})$").unwrap());
    match value {
        Value::Number(n) => n.as_i64().filter(|n| (0..=127).contains(n)).map(|n| n as i32),
        Value::String(text) => {
            let caps = NOTE.captures(text.trim())?;
            let natural: i32 = match caps[1].to_ascii_uppercase().as_str() {
                "C" => 0,
                "D" => 2,
                "E" => 4,
                "F" => 5,
                "G" => 7,
                "A" => 9,
                _ => 11,
            };
            let moved = match caps.get(2).map(|m| m.as_str()) {
                Some("#") => 1,
                Some("b") => -1,
                _ => 0,
            };
            let octave: i32 = caps[3].parse().ok()?;
            let midi = 12 * (octave + 5 - profile.vocab.middle_c_octave as i32) + natural + moved;
            (0..=127).contains(&midi).then_some(midi)
        }
        _ => None,
    }
}

/// How the model writes a note, in the DAW's words.
fn notes_words(profile: &DawProfile) -> String {
    format!("a note as the DAW writes it ({} is middle C) or its number (60)", profile.note_name(60))
}

/// What the song's instruments (as the backend lists them: `ref`, `name`, `notes`, `plugin`, `settings`, `samples` [{name,
/// print}], with `counted`, or `countedTo` of `patterns` when the count stopped short) could lose or share.
pub fn tidy_of(listed: &Value, instrument: &str) -> Value {
    let rows = listed.get("instruments").and_then(Value::as_array).cloned().unwrap_or_default();
    let counted = listed.get("counted").and_then(Value::as_bool).unwrap_or(true);
    let label = |row: &Value| {
        format!(
            "{} {}",
            row.get("ref").and_then(Value::as_str).unwrap_or("?"),
            quoted(row.get("name").and_then(Value::as_str).unwrap_or(""))
        )
    };
    let prints_of = |row: &Value| -> BTreeSet<String> {
        row.get("samples")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("print").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    };
    // (played: some note plays it; unplayed only when every note was counted)
    let played = |row: &Value| match row.get("notes").and_then(Value::as_u64) {
        Some(notes) if notes > 0 => Some(true),
        Some(_) if counted => Some(false),
        _ => None,
    };
    let empty = |row: &Value| prints_of(row).is_empty() && row.get("plugin").and_then(Value::as_bool) != Some(true);
    let mut unused = Vec::new();
    let mut empties = Vec::new();
    for row in &rows {
        if empty(row) {
            if played(row) == Some(false) && !row.get("name").and_then(Value::as_str).unwrap_or("").is_empty() {
                empties.push(label(row));
            }
            continue;
        }
        if played(row) == Some(false) {
            unused.push(label(row));
        }
    }
    // Samples two or more instruments hold (the same audio, by its print).
    let mut holders: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for row in &rows {
        for sample in row.get("samples").and_then(Value::as_array).into_iter().flatten() {
            if let Some(print) = sample.get("print").and_then(Value::as_str) {
                let entry = holders.entry(print.to_owned()).or_default();
                let who = label(row);
                if !entry.iter().any(|(holder, _)| *holder == who) {
                    entry.push((who, sample.get("name").and_then(Value::as_str).unwrap_or("").to_owned()));
                }
            }
        }
    }
    // Instruments that hold the same samples (copies), or nearly (four in five shared): one can go when it isn't played or
    // when both play the same way; a played copy with settings of its own is the producer's on purpose.
    let mut copies = Vec::new();
    let mut alike = Vec::new();
    let mut paired = BTreeSet::new();
    for (a, first) in rows.iter().enumerate() {
        let ours = prints_of(first);
        if ours.is_empty() {
            continue;
        }
        for second in rows.iter().skip(a + 1) {
            let theirs = prints_of(second);
            if theirs.is_empty() {
                continue;
            }
            let shared = ours.intersection(&theirs).count();
            let union = ours.union(&theirs).count();
            if shared * 5 < union * 4 {
                continue;
            }
            let what = if ours == theirs { "the same samples" } else { "nearly the same samples" };
            let same_settings = first.get("settings").is_some() && first.get("settings") == second.get("settings");
            let settings = if same_settings { "the same settings" } else { "settings of their own" };
            let pair = format!("{} and {}: {what} ({shared} of {union})", label(first), label(second));
            paired.insert(label(second));
            match (played(first), played(second)) {
                (Some(false), _) => copies.push(format!("{pair}; {} isn't played: it can go", label(first))),
                (_, Some(false)) => copies.push(format!("{pair}; {} isn't played: it can go", label(second))),
                _ if same_settings => copies.push(format!("{pair}, {settings}, both played: one can play the other's notes")),
                (Some(true), Some(true)) => alike.push(format!("{pair}, both played, with {settings}: keep both")),
                _ => alike.push(format!("{pair}, with {settings}; not every note was counted")),
            }
        }
    }
    let shared: Vec<String> = holders
        .into_iter()
        .filter(|(_, holders)| holders.len() > 1)
        .filter(|(_, holders)| !holders.iter().skip(1).all(|(holder, _)| paired.contains(holder)))
        .map(|(_, holders)| {
            let (_, sample) = &holders[0];
            let who: Vec<&str> = holders.iter().map(|(holder, _)| holder.as_str()).collect();
            format!("{} is in {}", quoted(sample), who.join(", "))
        })
        .collect();
    let mut reply = json!({"instruments": rows.len()});
    if !unused.is_empty() {
        reply["noNotesPlay"] = json!(unused);
    }
    if !counted {
        let to = listed.get("countedTo").and_then(Value::as_u64).unwrap_or(0);
        let of = listed.get("patterns").and_then(Value::as_u64).unwrap_or(0);
        reply["notCounted"] = json!(format!(
            "the notes of {to} of the song's {of} patterns were counted (it's too big to count in one look): an {instrument} with none there may still play later"
        ));
    }
    if !empties.is_empty() {
        reply["emptyButNamed"] = json!(empties);
    }
    if !copies.is_empty() {
        reply["copies"] = json!(copies);
    }
    if !alike.is_empty() {
        reply["alike"] = json!(alike);
    }
    if !shared.is_empty() {
        reply["sharedSamples"] = json!(shared);
    }
    reply["next"] = json!(if unused.is_empty() && copies.is_empty() && shared.is_empty() && empties.is_empty() {
        format!("nothing to tidy: every {instrument} is played and none is a spare copy")
    } else {
        format!(
            "propose what to delete or merge, and change only what the producer accepts (a deleted {instrument} comes back with undo_change); copies under alike are the producer's on purpose"
        )
    });
    reply
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("the {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn parts(plan: &Plan) -> String {
    plan.instruments.iter().filter_map(|planned| planned.part.clone()).collect::<Vec<_>>().join(", ")
}

/// A single-letter dynamic counts only beside longer ones.
fn fix_dynamics(samples: &mut [Sample]) {
    let stems: Vec<String> = samples.iter().map(|s| s.name.clone()).collect();
    let stems: Vec<&str> = stems.iter().map(String::as_str).collect();
    let mut hints: Vec<names::Hints> = samples.iter().map(|s| s.hints.clone()).collect();
    names::dynamics_alone(&stems, &mut hints);
    for (sample, hint) in samples.iter_mut().zip(hints) {
        sample.hints = hint;
    }
}

/// The name a new instrument gets: what the samples' names begin with ("Piano" of "Piano_C3_v1", "Piano_E3_v2"), else the
/// folder's name, else "Kit" for drums or "Samples".
pub fn common_name(samples: &[Sample], folder: Option<&str>, drums: bool) -> String {
    let fallback = || {
        folder
            .and_then(|folder| Path::new(folder.trim_end_matches(['/', '\\'])).file_name().map(|name| name.to_string_lossy().into_owned()))
            .filter(|name| name.chars().count() >= 2)
            .unwrap_or_else(|| if drums { "Kit" } else { "Samples" }.to_string())
    };
    let Some(first) = samples.first() else { return fallback() };
    if samples.len() == 1 {
        return first.name.clone();
    }
    let mut prefix: Vec<char> = first.name.chars().collect();
    for sample in &samples[1..] {
        let shared = prefix.iter().zip(sample.name.chars()).take_while(|(a, b)| **a == *b).count();
        prefix.truncate(shared);
    }
    let text: String = prefix.into_iter().collect();
    let trimmed = text.trim_end_matches(|c: char| !c.is_alphanumeric()).to_string();
    // (a prefix that ends inside a word, "Pad C" of "Pad C3", "Pad C4", goes back to the word before)
    let trimmed = if trimmed.len() < first.name.len() && first.name[trimmed.len()..].chars().next().is_some_and(char::is_alphanumeric) {
        trimmed.rsplit_once(|c: char| !c.is_alphanumeric()).map_or(String::new(), |(head, _)| head.to_string())
    } else {
        trimmed
    };
    let trimmed = trimmed.trim_end_matches(|c: char| !c.is_alphanumeric()).to_string();
    if trimmed.chars().count() >= 2 {
        trimmed
    } else {
        fallback()
    }
}

/// The files to map: `files`, or the sounds in `folder` (in name order; hidden files and the "._" copies macOS leaves on
/// other drives are none), at most 120.
fn files_of(input: &JsonObject) -> Result<Vec<String>, String> {
    let mut files: Vec<String> = Vec::new();
    if let Some(list) = input.get("files").and_then(Value::as_array) {
        files.extend(list.iter().filter_map(Value::as_str).map(kumi_media::audio::audio_path));
    }
    if let Some(folder) = input.get("folder").and_then(Value::as_str) {
        let folder = kumi_media::audio::audio_path(folder);
        let entries = std::fs::read_dir(&folder).map_err(|_| format!("Kumi can't read the folder {folder}"))?;
        let mut found: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .filter(|path| !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')))
            .filter(|path| {
                let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
                kumi_media::audio::AUDIO_EXTENSIONS.contains(&extension.as_str())
            })
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        found.sort();
        if found.is_empty() {
            return Err(format!("there are no sound files in {folder}"));
        }
        files.extend(found);
    }
    if files.is_empty() {
        return Err("give files (sound files' paths), a folder of them, or instrumentRef".into());
    }
    if files.len() > MOST_SAMPLES {
        return Err(format!("that's {} sounds: Kumi maps at most {MOST_SAMPLES} at once", files.len()));
    }
    let mut seen = BTreeSet::new();
    files.retain(|file| seen.insert(file.clone()));
    Ok(files)
}

fn description(words: &super::backend::Words) -> String {
    let instrument = words.instrument;
    format!(
        "Lay samples out across the keys as one {instrument}, from sound files (files, or every sound in folder) or an {instrument}'s own samples (instrumentRef). Kumi reads each name's note (C3, A#2, Db4, C-4), velocity (v1 to v16, vel64, pp to fff) and round robin (rr2), hears each sample's pitch and loudness, settles whether the names put middle C at C3, C4 or C5 by the pitch it hears, and plans: roots, zones reaching halfway to the next root, velocity layers in loudness order, round robins that take turns, an {instrument} per articulation (untagged samples a part of their own); a drum kit (a key per sound, its layers sharing it; gm: true puts kick, snare and hats on their General MIDI keys) when most samples have no pitch. action plan (the default) only plans: tell the producer what Kumi decided and ask only what its ask lists; apply with the plan's id makes it, one change per {instrument} (undo_change takes it back). roots sets a sample's note by hand ({notes}). action tidy looks over the song's {instrument}s instead: ones no note plays, spare copies, samples two hold; propose, and change only what the producer accepts.",
        notes = notes_words(words.profile)
    )
}

fn schema(words: &super::backend::Words) -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["plan", "apply", "tidy"]},
            "plan": {"type": "string", "description": "The planId plan answered: apply makes that plan, or says it changed"},
            "files": {"type": "array", "items": {"type": "string", "minLength": 1}, "minItems": 1, "maxItems": MOST_SAMPLES},
            "folder": {"type": "string", "minLength": 1, "description": "A folder whose sound files are mapped (not its subfolders)"},
            "instrumentRef": {"type": "string", "description": format!("An {} whose samples are laid out", words.instrument)},
            "layout": {"type": "string", "enum": ["keys", "drumkit"]},
            "middleC": {"type": "string", "enum": ["C3", "C4", "C5"], "description": "Where the names put middle C, when Kumi can't hear it"},
            "gm": {"type": "boolean"},
            "roots": {"type": "array", "maxItems": MOST_SAMPLES, "items": {"type": "object", "additionalProperties": false, "required": ["sample", "note"], "properties": {
                "sample": {"type": "string", "description": "Its name, file name or path"},
                "note": {"type": ["string", "integer"], "description": notes_words(words.profile)}
            }}},
            "part": {"type": "string", "description": format!("The part to lay out (an articulation, or {PLAIN}), when an instrument's samples hold several")},
            "name": {"type": "string", "maxLength": 64}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct MappingTool<B: MappingBackend> {
    owner: Rc<Mapping<B>>,
}

#[async_trait(?Send)]
impl<B: MappingBackend + 'static> KernelTool for MappingTool<B> {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        &self.owner.description
    }

    fn input_schema(&self) -> JsonObject {
        self.owner.schema.clone()
    }

    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.owner.run(input, signal).await {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }

    fn reach(&self) -> Reach {
        Reach::DawWrite
    }

    fn deferred(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_instrument_is_named_for_what_its_samples_share_or_its_folder() {
        let named = |names: &[&str], folder: Option<&str>, drums: bool| {
            common_name(&names.iter().map(|n| Sample::new(*n, *n, None)).collect::<Vec<_>>(), folder, drums)
        };
        assert_eq!(named(&["Piano_C3_v1", "Piano_E3_v2"], None, false), "Piano");
        assert_eq!(named(&["Pad C3", "Pad C4"], None, false), "Pad");
        assert_eq!(named(&["Rhodes Mk1 C2", "Rhodes Mk1 C3"], None, false), "Rhodes Mk1");
        assert_eq!(named(&["Kick", "Snare"], Some("/sounds/808 Kit/"), true), "808 Kit");
        assert_eq!(named(&["Kick", "Snare"], None, true), "Kit");
        assert_eq!(named(&["Pad", "Bell"], None, false), "Samples");
    }

    #[test]
    fn notes_are_read_as_the_daw_writes_them() {
        let renoise = &DawProfile::RENOISE;
        assert_eq!(note_of(&json!("C-5"), renoise), Some(60), "Renoise's C-5 is middle C");
        assert_eq!(note_of(&json!("C#4"), renoise), Some(49));
        assert_eq!(note_of(&json!(36), renoise), Some(36));
        assert_eq!(note_of(&json!("C4"), &DawProfile::REAPER), Some(60));
        assert_eq!(note_of(&json!("H4"), renoise), None);
        assert_eq!(note_of(&json!(130), renoise), None);
    }

    #[test]
    fn the_tidy_finds_unplayed_instruments_spare_copies_and_shared_samples() {
        let listed = json!({"counted": true, "instruments": [
            {"ref": "in:0", "name": "Kick", "notes": 40, "settings": "s1", "samples": [{"name": "kick", "print": "a"}]},
            {"ref": "in:1", "name": "Kick copy", "notes": 0, "settings": "s1", "samples": [{"name": "kick", "print": "a"}]},
            {"ref": "in:2", "name": "Keys", "notes": 12, "settings": "s2", "samples": [{"name": "C3", "print": "c"}, {"name": "kick", "print": "a2"}]},
            {"ref": "in:3", "name": "Perc", "notes": 3, "settings": "s3", "samples": [{"name": "kick", "print": "a2"}]},
            {"ref": "in:4", "name": "", "notes": 0, "samples": []},
            {"ref": "in:5", "name": "Pad", "notes": 9, "settings": "s5", "samples": [{"name": "pad", "print": "p"}]},
            {"ref": "in:6", "name": "Pad wet", "notes": 4, "settings": "s6", "samples": [{"name": "pad", "print": "p"}]},
            {"ref": "in:7", "name": "Lead", "notes": 9, "settings": "s7", "samples": [{"name": "lead", "print": "l"}]},
            {"ref": "in:8", "name": "Lead too", "notes": 2, "settings": "s7", "samples": [{"name": "lead", "print": "l"}]}
        ]});
        let tidy = tidy_of(&listed, "instrument");
        assert_eq!(tidy["noNotesPlay"], json!(["in:1 “Kick copy”"]));
        assert_eq!(
            tidy["copies"],
            json!([
                "in:0 “Kick” and in:1 “Kick copy”: the same samples (1 of 1); in:1 “Kick copy” isn't played: it can go",
                "in:7 “Lead” and in:8 “Lead too”: the same samples (1 of 1), the same settings, both played: one can play the other's notes"
            ])
        );
        assert_eq!(
            tidy["alike"],
            json!(["in:5 “Pad” and in:6 “Pad wet”: the same samples (1 of 1), both played, with settings of their own: keep both"])
        );
        // A copy's shared sample isn't said twice; another instrument's is.
        assert_eq!(tidy["sharedSamples"], json!(["“kick” is in in:2 “Keys”, in:3 “Perc”"]));
        assert!(tidy.get("emptyButNamed").is_none(), "an empty slot isn't the producer's");
    }

    #[test]
    fn a_song_too_big_to_count_says_how_far_the_count_got_and_calls_nothing_unplayed() {
        let listed = json!({"counted": false, "countedTo": 40, "patterns": 120, "instruments": [
            {"ref": "in:0", "name": "Kick", "notes": 0, "samples": [{"name": "kick", "print": "a"}]},
            {"ref": "in:1", "name": "Kick 2", "notes": 0, "settings": "x", "samples": [{"name": "kick", "print": "a"}]}
        ]});
        let tidy = tidy_of(&listed, "instrument");
        assert!(tidy.get("noNotesPlay").is_none());
        assert!(tidy.get("copies").is_none(), "neither is known unplayed: {tidy}");
        assert!(tidy["notCounted"].as_str().unwrap().contains("40 of the song's 120 patterns"));
    }

    #[test]
    fn a_plans_id_follows_what_it_places() {
        let samples = vec![Sample::new("/a.wav", "Pad C4", None), Sample::new("/b.wav", "Pad C5", None)];
        let asked = Asked { lowest: 0, highest: 127, first_key: 48, ..Asked::default() };
        let name = |midi: i32| midi.to_string();
        let first = plan_id(&plan(&samples, &asked, &name));
        assert_eq!(first, plan_id(&plan(&samples, &asked, &name)));
        let moved = Asked { middle_c: Some(3), ..asked };
        assert_ne!(first, plan_id(&plan(&samples, &moved, &name)), "another convention places them elsewhere");
    }
}
