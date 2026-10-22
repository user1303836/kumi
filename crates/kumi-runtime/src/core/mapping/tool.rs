//! The map_samples tool, the same in every DAW: plan a set of samples across the keys (files, a folder, or an instrument's own
//! samples), say what was decided and ask only what can't be told, then make it as one change per instrument; or look over the
//! song's instruments for a tidy. The DAW's backend reads, hears and writes.
use super::backend::{Held, MappingBackend};
use super::{heard::hear_file, names, plan, Asked, Layout, Plan, Sample, MOST_SAMPLES};
use crate::core::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use crate::core::errors::RuntimeError;
use async_trait::async_trait;
use kumi_common::abort::{Signal, SignalExt};
use kumi_common::js::json::stringify;
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

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("plan");
        match action {
            "tidy" => return self.tidy(&signal).await,
            "plan" | "apply" => {}
            _ => return Err("action is plan (the default), apply or tidy".into()),
        }
        let instrument = input.get("instrumentRef").and_then(Value::as_str).map(str::to_owned);
        let samples = match &instrument {
            Some(reference) => {
                if input.contains_key("files") || input.contains_key("folder") {
                    return Err("give files, a folder or instrumentRef, one of them".into());
                }
                let (_, held) = self.backend.samples_of(reference, &signal).await?;
                self.hear_held(held, &signal).await?
            }
            None => self.hear_files(&files_of(&input)?, &signal).await?,
        };
        let asked = self.asked(&input, &samples)?;
        let plan = plan(&samples, &asked, &|midi| self.backend.note_name(midi));
        let mut reply = self.plan_json(&plan);
        if action == "plan" {
            reply["next"] = json!(if plan.instruments.is_empty() {
                "nothing to make yet: give what leftOut asks".to_string()
            } else {
                format!("action apply makes {} as planned, one change each", count(plan.instruments.len(), self.backend.words().instrument))
            });
            return Ok(reply);
        }
        if plan.instruments.is_empty() {
            return Err(format!("nothing to map: {}", stringify(&reply["leftOut"])));
        }
        let drums = plan.layout == Layout::Drums;
        let mut made = Vec::new();
        match &instrument {
            Some(reference) => {
                let part = input.get("part").and_then(Value::as_str);
                let planned = match (plan.instruments.len(), part) {
                    (1, _) => &plan.instruments[0],
                    (_, Some(part)) => plan
                        .instruments
                        .iter()
                        .find(|planned| planned.part.as_deref() == Some(part))
                        .ok_or_else(|| format!("part is one of {}", parts(&plan)))?,
                    _ => {
                        return Err(format!(
                            "these samples are {} articulations ({}): in one {} they'd play over each other; say which to lay out (part), or make an {} of each from their files",
                            plan.instruments.len(),
                            parts(&plan),
                            self.backend.words().instrument,
                            self.backend.words().instrument
                        ))
                    }
                };
                made.push(self.backend.remap(reference, planned, &signal).await?);
            }
            None => {
                let base = input.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| common_name(&samples));
                for planned in &plan.instruments {
                    let name = match &planned.part {
                        Some(part) => format!("{base} \u{b7} {part}"),
                        None => base.clone(),
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

    /// Each file heard (a file that can't be is mapped by its name alone, and said).
    async fn hear_files(&self, paths: &[String], signal: &Signal) -> Result<Vec<Sample>, String> {
        let mut samples = Vec::new();
        for path in paths {
            signal.check().map_err(|_| "stopped".to_string())?;
            let stem = Path::new(path).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
            let heard = hear_file(path, signal).await.ok();
            samples.push(Sample::new(path.clone(), stem, heard));
        }
        fix_dynamics(&mut samples);
        Ok(samples)
    }

    async fn hear_held(&self, held: Vec<Held>, signal: &Signal) -> Result<Vec<Sample>, String> {
        if held.is_empty() {
            return Err(format!("that {} has no samples", self.backend.words().instrument));
        }
        if held.len() > MOST_SAMPLES {
            return Err(format!("Kumi maps at most {MOST_SAMPLES} samples at once"));
        }
        let mut samples = Vec::new();
        for sample in held {
            signal.check().map_err(|_| "stopped".to_string())?;
            let heard = match &sample.file {
                Ok(file) => hear_file(file, signal).await.ok(),
                Err(_) => None,
            };
            samples.push(Sample::new(sample.reference, sample.name, heard));
        }
        fix_dynamics(&mut samples);
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
        let c4_is_60 = match input.get("middleC").and_then(Value::as_str) {
            None => None,
            Some("C4") => Some(true),
            Some("C3") => Some(false),
            Some(_) => return Err("middleC is C3 or C4: where the names put middle C".into()),
        };
        let mut roots = BTreeMap::new();
        for given in input.get("roots").and_then(Value::as_array).into_iter().flatten() {
            let named = given.get("sample").and_then(Value::as_str).unwrap_or("");
            let sample = samples
                .iter()
                .find(|s| s.name == named || s.id == named || Path::new(&s.id).file_name().is_some_and(|f| f.to_string_lossy() == named))
                .ok_or_else(|| format!("roots: no sample is {named}"))?;
            let note =
                given.get("note").and_then(|note| self.backend.note_of(note)).ok_or_else(|| format!("roots: a note is {}", words.notes))?;
            roots.insert(sample.id.clone(), note);
        }
        Ok(Asked {
            layout,
            c4_is_60,
            roots,
            gm: input.get("gm").and_then(Value::as_bool).unwrap_or(false),
            lowest: words.lowest,
            highest: words.highest,
            first_key: words.first_key,
        })
    }

    fn plan_json(&self, plan: &Plan) -> Value {
        let note = |midi: i32| self.backend.note_name(midi);
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
            reply["middleC"] = json!(format!("{} ({})", if convention.c4_is_60 { "C4" } else { "C3" }, convention.how));
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

/// What the song's instruments (as the backend lists them: `ref`, `name`, `notes` (None when the song was too big to count),
/// `plugin`, `samples` [{name, print}]) could lose or share.
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
    let empty = |row: &Value| prints_of(row).is_empty() && row.get("plugin").and_then(Value::as_bool) != Some(true);
    let mut unused = Vec::new();
    let mut empties = Vec::new();
    for row in &rows {
        if empty(row) {
            if row.get("notes").and_then(Value::as_u64) == Some(0) && !row.get("name").and_then(Value::as_str).unwrap_or("").is_empty() {
                empties.push(label(row));
            }
            continue;
        }
        if counted && row.get("notes").and_then(Value::as_u64) == Some(0) {
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
    // Instruments that hold the same samples (copies), or nearly (four in five shared).
    let mut copies = Vec::new();
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
            if shared * 5 >= union * 4 {
                let words = if ours == theirs { "the same samples" } else { "nearly the same samples" };
                copies.push(format!("{} and {}: {words} ({shared} of {union})", label(first), label(second)));
                paired.insert(label(second));
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
        reply["notCounted"] =
            json!(format!("the song is too big to count every note in one look: which {instrument}s no note plays isn't known"));
    }
    if !empties.is_empty() {
        reply["emptyButNamed"] = json!(empties);
    }
    if !copies.is_empty() {
        reply["copies"] = json!(copies);
    }
    if !shared.is_empty() {
        reply["sharedSamples"] = json!(shared);
    }
    reply["next"] = json!(if unused.is_empty() && copies.is_empty() && shared.is_empty() && empties.is_empty() {
        format!("nothing to tidy: every {instrument} is played and none is a copy")
    } else {
        format!("propose what to delete or merge, and change only what the producer accepts (a deleted {instrument} comes back with undo_change)")
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

/// The name a new instrument gets: what the samples' names begin with ("Piano" of "Piano_C3_v1", "Piano_E3_v2"), else the first.
pub fn common_name(samples: &[Sample]) -> String {
    let Some(first) = samples.first() else { return "Samples".into() };
    let mut prefix: Vec<char> = first.name.chars().collect();
    for sample in &samples[1..] {
        let shared = prefix.iter().zip(sample.name.chars()).take_while(|(a, b)| **a == *b).count();
        prefix.truncate(shared);
    }
    let text: String = prefix.into_iter().collect();
    let trimmed = text.trim_end_matches(|c: char| !c.is_alphanumeric()).to_string();
    // (a prefix that ends inside a word, "Pad C" of "Pad C3", "Pad C4", goes back to the word before)
    let trimmed = if samples.len() > 1 && trimmed.len() < first.name.len() {
        let next = first.name[trimmed.len()..].chars().next();
        if next.is_some_and(char::is_alphanumeric) {
            trimmed.rsplit_once(|c: char| !c.is_alphanumeric()).map_or(trimmed.clone(), |(head, _)| head.to_string())
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let trimmed = trimmed.trim_end_matches(|c: char| !c.is_alphanumeric()).to_string();
    if trimmed.chars().count() >= 2 {
        trimmed
    } else {
        first.name.clone()
    }
}

/// The files to map: `files`, or the sounds in `folder` (in name order), at most 120.
fn files_of(input: &JsonObject) -> Result<Vec<String>, String> {
    let mut files: Vec<String> = Vec::new();
    if let Some(list) = input.get("files").and_then(Value::as_array) {
        files.extend(list.iter().filter_map(Value::as_str).map(crate::audio::audio_path));
    }
    if let Some(folder) = input.get("folder").and_then(Value::as_str) {
        let folder = crate::audio::audio_path(folder);
        let entries = std::fs::read_dir(&folder).map_err(|_| format!("Kumi can't read the folder {folder}"))?;
        let mut found: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .filter(|path| {
                let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
                crate::audio::AUDIO_EXTENSIONS.contains(&extension.as_str())
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
        "Lay samples out across the keys as one {instrument}, from sound files (files, or every sound in folder) or an {instrument}'s own samples (instrumentRef). Kumi reads each name's note (C3, A#2, Db4, C-4, 060), velocity (v1 to v16, vel64, pp to fff) and round robin (rr2), hears each sample's pitch and loudness, settles whether the names put middle C at C3 or C4 by the pitch it hears, and plans: roots, zones reaching halfway to the next root, velocity layers in loudness order, round robins that take turns, an {instrument} per articulation; a drum kit (a key per sound, its layers sharing it; gm: true puts kick, snare and hats on their General MIDI keys) when most samples have no pitch. action plan (the default) only plans: tell the producer what Kumi decided and ask only what its ask lists; apply makes it, one change per {instrument} (undo_change takes it back). roots sets a sample's note by hand ({words}). action tidy looks over the song's {instrument}s instead: ones no note plays, copies, samples two hold; propose, and change only what the producer accepts.",
        words = words.notes
    )
}

fn schema(words: &super::backend::Words) -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["plan", "apply", "tidy"]},
            "files": {"type": "array", "items": {"type": "string", "minLength": 1}, "minItems": 1, "maxItems": MOST_SAMPLES},
            "folder": {"type": "string", "minLength": 1, "description": "A folder whose sound files are mapped (not its subfolders)"},
            "instrumentRef": {"type": "string", "description": format!("An {} whose samples are laid out", words.instrument)},
            "layout": {"type": "string", "enum": ["keys", "drumkit"]},
            "middleC": {"type": "string", "enum": ["C3", "C4"], "description": "Where the names put middle C, when Kumi can't hear it"},
            "gm": {"type": "boolean"},
            "roots": {"type": "array", "maxItems": MOST_SAMPLES, "items": {"type": "object", "additionalProperties": false, "required": ["sample", "note"], "properties": {
                "sample": {"type": "string", "description": "Its name, file name or path"},
                "note": {"type": ["string", "integer"], "description": words.notes}
            }}},
            "part": {"type": "string", "description": "The articulation to lay out, when an instrument's samples hold several"},
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
    fn a_new_instrument_is_named_for_what_its_samples_share() {
        let named = |names: &[&str]| common_name(&names.iter().map(|n| Sample::new(*n, *n, None)).collect::<Vec<_>>());
        assert_eq!(named(&["Piano_C3_v1", "Piano_E3_v2"]), "Piano");
        assert_eq!(named(&["Pad C3", "Pad C4"]), "Pad");
        assert_eq!(named(&["Kick", "Snare"]), "Kick");
        assert_eq!(named(&["Rhodes Mk1 C2", "Rhodes Mk1 C3"]), "Rhodes Mk1");
    }

    #[test]
    fn the_tidy_finds_unplayed_instruments_copies_and_shared_samples() {
        let listed = json!({"counted": true, "instruments": [
            {"ref": "in:0", "name": "Kick", "notes": 40, "samples": [{"name": "kick", "print": "a"}]},
            {"ref": "in:1", "name": "Kick copy", "notes": 0, "samples": [{"name": "kick", "print": "a"}]},
            {"ref": "in:2", "name": "Keys", "notes": 12, "samples": [{"name": "C3", "print": "c"}, {"name": "kick", "print": "a2"}]},
            {"ref": "in:3", "name": "Perc", "notes": 3, "samples": [{"name": "kick", "print": "a2"}]},
            {"ref": "in:4", "name": "", "notes": 0, "samples": []}
        ]});
        let tidy = tidy_of(&listed, "instrument");
        assert_eq!(tidy["noNotesPlay"], json!(["in:1 “Kick copy”"]));
        assert_eq!(tidy["copies"], json!(["in:0 “Kick” and in:1 “Kick copy”: the same samples (1 of 1)"]));
        // A copy's shared sample isn't said twice; another instrument's is.
        assert_eq!(tidy["sharedSamples"], json!(["“kick” is in in:2 “Keys”, in:3 “Perc”"]));
        assert!(tidy.get("emptyButNamed").is_none(), "an empty slot isn't the producer's");
    }

    #[test]
    fn a_song_too_big_to_count_says_so_instead_of_calling_instruments_unplayed() {
        let listed =
            json!({"counted": false, "instruments": [{"ref": "in:0", "name": "Kick", "samples": [{"name": "kick", "print": "a"}]}]});
        let tidy = tidy_of(&listed, "instrument");
        assert!(tidy.get("noNotesPlay").is_none());
        assert!(tidy["notCounted"].as_str().unwrap().contains("too big"));
    }
}
