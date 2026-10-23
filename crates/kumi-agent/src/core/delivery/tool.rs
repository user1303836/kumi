//! The render_files tool, the same in every DAW: plan assets from the timeline's items (named in the project's own convention),
//! make their regions in one change, check what a render would write (the DAW's own dry run, against the names planned), and
//! render it, the DAW's render settings put back after. The DAW's backend reads, adds regions, lists and renders.
use super::assets::{assets, bridging, Asset};
use super::backend::{DeliveryBackend, NewRegion, Region, Targets};
use super::naming::{convention, numbered, problems};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::errors::RuntimeError;
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;

pub const TOOL: &str = "render_files";

/// Items closer than this are one sound, by default (seconds).
const GAP: f64 = 0.25;
/// The most assets one plan makes.
const MOST_ASSETS: usize = 500;
/// A region already there this close to an asset's stretch is the asset's (seconds).
const SAME_SPAN: f64 = 0.05;
/// What a tail stops short of: the next asset's start (seconds).
const BEFORE_NEXT: f64 = 0.01;

/// The keys a render's settings take, passed to the DAW as given. (No tail: tailMs makes the regions longer, up to the next
/// sound; a render's own tail would run on past each region's end into the next one.)
const SETTINGS: [&str; 11] =
    ["source", "bounds", "trackRefs", "format", "bits", "sampleRate", "channels", "pattern", "folder", "overwrite", "dialog"];

pub struct Delivery<B: DeliveryBackend> {
    backend: B,
    description: String,
    schema: JsonObject,
}

/// An asset planned: its name, its stretch (with the tail), its items' tracks (names, and refs: what its region renders), and
/// the region there already, if any.
struct Planned {
    name: String,
    start: f64,
    end: f64,
    tracks: Vec<String>,
    renders: Vec<String>,
    layers: usize,
    there: Option<String>,
}

/// The assets planned, how they're named, what to say of them, and the names the DAW's regions have now.
struct Assets {
    planned: Vec<Planned>,
    naming: String,
    said: Vec<String>,
    taken: Vec<String>,
}

impl<B: DeliveryBackend + 'static> Delivery<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let words = backend.words();
        Rc::new(Delivery { description: description(words), schema: schema(words), backend })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(DeliveryTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        match input.get("action").and_then(Value::as_str).unwrap_or("plan") {
            "plan" => self.plan(&input, false, &signal).await,
            "regions" => self.plan(&input, true, &signal).await,
            "check" => self.check(&input, &signal).await.map(|(reply, _, _)| reply),
            "render" => self.render(&input, &signal).await,
            _ => Err("action is plan (the default), regions, check or render".into()),
        }
    }

    /// The assets of the items asked for, named: an asset with its region by the region's name, the new ones numbered in a
    /// row as asked (name), else in the convention the project's regions follow.
    async fn assets(&self, input: &JsonObject, signal: &Signal) -> Result<Assets, String> {
        let region = self.backend.words().region;
        let tracks: Vec<String> = strings(input.get("onTracks"));
        let selected = input.get("selectedOnly").and_then(Value::as_bool).unwrap_or(false);
        let items = self.backend.items(&tracks, selected, signal).await?;
        if items.is_empty() {
            return Err(if selected {
                "no items are selected: select the sounds, or say selectedOnly: false".into()
            } else {
                "there are no items to make assets of on those tracks".into()
            });
        }
        let gap = input.get("gap").and_then(Value::as_f64).unwrap_or(GAP).clamp(0., 10.);
        let found = assets(&items, gap);
        if found.len() > MOST_ASSETS {
            return Err(format!("that's {} assets: Kumi plans at most {MOST_ASSETS} at once (select fewer, or a longer gap)", found.len()));
        }
        let regions = self.backend.regions(signal).await?;
        let names: Vec<String> = regions.iter().map(|r| r.name.clone()).collect();
        let (stem, separator, pad, first, naming) = match input.get("name").and_then(Value::as_str).filter(|name| !name.trim().is_empty()) {
            Some(stem) => {
                let separator = input.get("separator").and_then(Value::as_str).unwrap_or("_").to_string();
                let used = convention(&names).filter(|found| found.stem == stem && found.separator == separator);
                let pad = input.get("pad").and_then(Value::as_u64).map(|pad| pad.clamp(1, 6) as usize).or(used.as_ref().map(|c| c.pad)).unwrap_or(2);
                let first = input.get("first").and_then(Value::as_u64).map(|n| n as usize).unwrap_or_else(|| used.as_ref().map_or(1, |c| c.highest + 1));
                let words = match &used {
                    Some(used) => format!("{stem}{separator}{} on from the {region}s there (the highest {})", "N".repeat(pad), used.highest),
                    None => format!("{stem}{separator}{} as asked", "N".repeat(pad)),
                };
                (stem.to_string(), separator, pad, first, words)
            }
            None => match convention(&names) {
                Some(found) => {
                    let first = input.get("first").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(found.highest + 1);
                    let words = format!(
                        "{}{}{}, the convention the project's {region}s follow, from {}",
                        found.stem,
                        found.separator,
                        "N".repeat(found.pad),
                        numbered(&found.stem, &found.separator, found.pad, first)
                    );
                    (found.stem, found.separator, found.pad, first, words)
                }
                None => return Err(format!("name: the stem the files are named for (\"Footstep_Gravel\"): the project's {region}s follow no numbered convention to go on from")),
            },
        };
        let tail = input.get("tailMs").and_then(Value::as_f64).unwrap_or(0.).clamp(0., 30_000.) / 1000.;
        let mut said = Vec::new();
        let mut planned = Vec::new();
        let mut fresh = 0;
        for (k, asset) in found.iter().enumerate() {
            let next = found.get(k + 1).map(|next: &Asset| next.start);
            let mut end = asset.end + tail;
            if let Some(next) = next.filter(|next| end > next - BEFORE_NEXT) {
                end = (next - BEFORE_NEXT).max(asset.end);
                said.push(format!("the tail of asset {} stops before the next sound starts", k + 1));
            }
            let (mut tracks, mut renders): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
            for &at in &asset.items {
                if !tracks.contains(&items[at].track_name) {
                    tracks.push(items[at].track_name.clone());
                }
                if !renders.contains(&items[at].track) {
                    renders.push(items[at].track.clone());
                }
            }
            if let Some((bed, apart)) = bridging(&items, asset, gap) {
                let bed = &items[bed];
                said.push(format!(
                    "asset {} is one only because {} on {} ({:.1} s) runs under {apart} sounds: onTracks without {} keeps them apart",
                    k + 1,
                    if bed.name.is_empty() { "an item".to_string() } else { quoted(&bed.name) },
                    quoted(&bed.track_name),
                    bed.end - bed.start,
                    quoted(&bed.track_name)
                ));
            }
            let there = regions
                .iter()
                .find(|region: &&Region| (region.start - asset.start).abs() <= SAME_SPAN && (region.end - end).abs() <= SAME_SPAN.max(tail))
                .map(|region| region.name.clone());
            // (an asset with its region is that region's file: only the new ones are numbered, in a row from first)
            let name = match &there {
                Some(name) => name.clone(),
                None => {
                    fresh += 1;
                    numbered(&stem, &separator, pad, first + fresh - 1)
                }
            };
            planned.push(Planned { name, start: asset.start, end, tracks, renders, layers: asset.items.len(), there });
        }
        Ok(Assets { planned, naming, said, taken: names })
    }

    async fn plan(&self, input: &JsonObject, make: bool, signal: &Signal) -> Result<Value, String> {
        let words = self.backend.words();
        let region = words.region;
        let Assets { planned, naming, mut said, taken } = self.assets(input, signal).await?;
        // (the new names only: those there are the regions' own, as the producer named them)
        let names: Vec<String> = planned.iter().filter(|p| p.there.is_none()).map(|p| p.name.clone()).collect();
        let mut found = problems(&names);
        if let Some(twice) = names.iter().find(|name| taken.iter().any(|old| old.to_lowercase() == name.to_lowercase())) {
            found.push(format!(
                "{} is a {region}'s name already: two files of one name (first: a number past the highest there)",
                quoted(twice)
            ));
        }
        let rows: Vec<Value> = planned
            .iter()
            .map(|p| {
                let mut row = json!({"name": p.name, "from": round(p.start), "to": round(p.end), "layers": p.layers, "tracks": p.tracks});
                if p.there.is_some() {
                    row["region"] = json!("there already: its file has its name");
                }
                row
            })
            .collect();
        let mut reply = json!({"assets": rows, "naming": naming});
        if !found.is_empty() {
            reply["problems"] = json!(found);
        }
        if !make {
            if !said.is_empty() {
                reply["said"] = json!(said);
            }
            reply["next"] = json!(format!(
                "action regions makes a {region} for each one not there yet (one change), then check lists the files a render would write, by these names"
            ));
            return Ok(reply);
        }
        if !found.is_empty() {
            return Err(format!("these names won't do: {}; give name (and separator) for others", found.join("; ")));
        }
        let renders = strings(input.get("renders"));
        let new: Vec<NewRegion> = planned
            .iter()
            .filter(|p| p.there.is_none())
            .map(|p| NewRegion {
                start: p.start,
                end: p.end,
                name: p.name.clone(),
                renders: if renders.is_empty() { p.renders.clone() } else { renders.clone() },
            })
            .collect();
        if new.is_empty() {
            return Err(format!("each asset has its {region} already: check lists the files a render would write"));
        }
        let skipped = planned.len() - new.len();
        if skipped > 0 {
            said.push(format!("{skipped} asset{} had {region}s already", if skipped == 1 { "" } else { "s" }));
        }
        reply["made"] = self.backend.add_regions(&new, signal).await?;
        if !said.is_empty() {
            reply["said"] = json!(said);
        }
        reply["next"] = json!(format!(
            "check lists the files a render would write (names: these, and only their {region}s render; source {}, the default: {}), then render writes them",
            words.source, words.source_said
        ));
        Ok(reply)
    }

    /// The DAW's dry run, measured against the names expected: what's missing, what's extra, clashes and names that won't do.
    async fn check(&self, input: &JsonObject, signal: &Signal) -> Result<(Value, Targets, bool), String> {
        let settings = self.settings(input, signal).await?;
        let targets = self.backend.targets(&settings, signal).await?;
        let stems: Vec<String> = targets.files.iter().map(|file| stem_of(file)).collect();
        let expected = strings(input.get("names"));
        let missing: Vec<&String> = expected.iter().filter(|name| !stems.contains(name)).collect();
        let extra: Vec<&String> =
            if expected.is_empty() { Vec::new() } else { stems.iter().filter(|stem| !expected.contains(stem)).collect() };
        // (two targets of one name: the DAW numbers the second "-001", a clash nobody asked for)
        static CLASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*)-[0-9]{3}$").unwrap());
        let clashes: Vec<&String> =
            stems.iter().filter(|stem| CLASH.captures(stem).is_some_and(|caps| stems.contains(&caps[1].to_string()))).collect();
        let found = problems(&stems);
        let blocking = !missing.is_empty() || !extra.is_empty() || !clashes.is_empty() || found.iter().any(|p| !p.contains("spaces"));
        let mut reply = json!({"files": stems, "folder": targets.folder, "count": targets.files.len(), "ok": !blocking});
        if !missing.is_empty() {
            reply["missing"] = json!(missing);
        }
        if !extra.is_empty() {
            reply["extra"] = json!(extra);
        }
        if !clashes.is_empty() {
            reply["clashes"] = json!(clashes);
        }
        if !found.is_empty() {
            reply["problems"] = json!(found);
        }
        if !targets.there.is_empty() {
            reply["there"] = json!(targets.there.iter().map(|file| stem_of(file)).collect::<Vec<_>>());
        }
        if !targets.notes.is_empty() {
            reply["notes"] = json!(targets.notes);
        }
        reply["next"] = json!(if blocking {
            "fix what the check found before rendering (names, regions, the pattern)".to_string()
        } else if !targets.there.is_empty() {
            "these files are there already: render with overwrite: true writes over them, only when the producer says".to_string()
        } else {
            "render writes exactly these files".to_string()
        });
        Ok((reply, targets, blocking))
    }

    async fn render(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let (checked, targets, blocking) = self.check(input, signal).await?;
        if blocking {
            return Err(format!("nothing was rendered: {}", stringify(&checked)));
        }
        let overwrite = input.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        if !targets.there.is_empty() && !overwrite {
            return Err(format!(
                "nothing was rendered: {} of the files are there already ({}…); overwrite: true writes over them, only when the producer says",
                targets.there.len(),
                stem_of(&targets.there[0])
            ));
        }
        let settings = self.settings(input, signal).await?;
        let done = self.backend.render(&settings, &targets.files, signal).await?;
        let mut reply = json!({"rendered": done.files, "folder": targets.folder, "count": targets.files.len(), "said": format!("{} rendered with the settings asked; every render setting is back as it was", self.backend.words().app)});
        // (what the check said of these settings, then what the render said)
        let mut notes = targets.notes.clone();
        notes.extend(done.notes.into_iter().filter(|note| !targets.notes.contains(note)));
        if !notes.is_empty() {
            reply["notes"] = json!(notes);
        }
        Ok(reply)
    }

    /// The render's settings from the input: the keys the DAW takes, by default a file per region from the DAW's delivery
    /// source; and the regions that render (`regionRefs`): those named in regions, else, from the default source over regions,
    /// those the names name when each is a region's (a delivery's own, not every region the project has).
    async fn settings(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let words = self.backend.words();
        let mut settings = serde_json::Map::new();
        for key in SETTINGS {
            if let Some(value) = input.get(key).filter(|value| !value.is_null()) {
                settings.insert(key.into(), value.clone());
            }
        }
        settings.entry("pattern").or_insert(json!(words.region_pattern));
        let asked = strings(input.get("regions"));
        let names = strings(input.get("names"));
        let by_default = !settings.contains_key("source") && input.get("bounds").is_none_or(|bounds| *bounds == "regions");
        settings.entry("source").or_insert(json!(words.source));
        if asked.is_empty() && (names.is_empty() || !by_default) {
            return Ok(Value::Object(settings));
        }
        let regions = self.backend.regions(signal).await?;
        let named = |name: &String| -> Vec<String> { regions.iter().filter(|r| r.name == *name).map(|r| r.reference.clone()).collect() };
        let mut picked = Vec::new();
        for name in &asked {
            let found = named(name);
            if found.is_empty() {
                return Err(format!("no {} is named {}: action regions makes the assets' {}s", words.region, quoted(name), words.region));
            }
            picked.extend(found);
        }
        if asked.is_empty() && names.iter().all(|name| !named(name).is_empty()) {
            picked = names.iter().flat_map(named).collect();
        }
        if !picked.is_empty() {
            settings.insert("regionRefs".into(), json!(picked));
        }
        Ok(Value::Object(settings))
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

fn stem_of(file: &str) -> String {
    Path::new(file).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| file.to_string())
}

fn round(seconds: f64) -> f64 {
    (seconds * 1000.).round() / 1000.
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

fn description(words: &super::backend::Words) -> String {
    let region = words.region;
    format!(
        "Deliver files from {app}: one per sound, named for the game or the client. action plan (the default) groups the items into assets (items stacked or within gap seconds, 0.25 by default, are one sound: three layers of a footstep make one) and names them in the convention the project's {region}s follow (Footstep_Gravel_07 goes on to _08), or as name asks (separator _ and pad 2 by default); an asset with its {region} already keeps the {region}'s name, and only the new ones are numbered. tailMs makes each {region} run on past its sound for reverbs to ring out (never into the next sound). action regions makes a {region} for each asset not there yet, in one change, each rendering its asset's own tracks (renders: other tracks for all of them). action check lists the files a render would write, from {app}'s own dry run, without rendering: give names (the assets' names) and it says what's missing, extra, clashing or there already; only the {region}s the names name render (regions: name others). action render checks again and writes exactly those files, never over one that's there unless overwrite: true, and puts every render setting back. Settings: pattern ({wildcards}; {pattern} by default: a file per {region}), source ({sources}), bounds ({bounds}), format (wav with bits 16, 24 or 32 float, or project: the format set in {app}'s render dialog, for MP3 or FLAC), sampleRate, channels, folder ({app}'s render folder for the project by default), dialog (true keeps the render dialog's normalizing, limiter, fades, dither and second format, off for a delivery by default). Don't stop to ask for what has a default: with none asked, the files are WAV 24-bit at the project's rate in that folder; say what was used, and render again as the game takes it when the producer says.",
        app = words.app,
        wildcards = words.wildcards,
        pattern = words.region_pattern,
        sources = words.sources,
        bounds = words.bounds
    )
}

fn schema(words: &super::backend::Words) -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["plan", "regions", "check", "render"]},
            "onTracks": {"type": "array", "items": {"type": "string"}, "description": "Only the items on these tracks"},
            "selectedOnly": {"type": "boolean"},
            "gap": {"type": "number", "minimum": 0, "maximum": 10},
            "name": {"type": "string", "maxLength": 60, "description": "The stem the assets are named for"},
            "separator": {"type": "string", "maxLength": 2},
            "pad": {"type": "integer", "minimum": 1, "maximum": 6},
            "first": {"type": "integer", "minimum": 0},
            "tailMs": {"type": "number", "minimum": 0, "maximum": 30000, "description": format!("How far each {} runs on past its sound", words.region)},
            "renders": {"type": "array", "items": {"type": "string"}, "description": format!("The tracks each new {} renders (its asset's own by default)", words.region)},
            "names": {"type": "array", "items": {"type": "string"}, "maxItems": 2000, "description": "The names the files should have"},
            "regions": {"type": "array", "items": {"type": "string"}, "maxItems": 2000, "description": format!("The {}s that render, by name (by default those the names name)", words.region)},
            "dialog": {"type": "boolean", "description": "Keep the render dialog's normalizing, limiter, fades, dither and second format"},
            "pattern": {"type": "string", "minLength": 1, "maxLength": 512},
            "source": {"type": "string", "enum": ["master", "matrix", "stems"]},
            "bounds": {"description": "regions, selected-regions, project, time-selection, or {start, end} in seconds", "anyOf": [
                {"type": "string", "enum": ["regions", "selected-regions", "project", "time-selection"]},
                {"type": "object", "additionalProperties": false, "required": ["start", "end"], "properties": {"start": {"type": "number", "minimum": 0}, "end": {"type": "number", "minimum": 0}}}
            ]},
            "trackRefs": {"type": "array", "items": {"type": "string"}, "description": "A stems render's tracks"},
            "format": {"type": "string", "enum": ["wav", "project"]},
            "bits": {"type": "integer", "enum": [16, 24, 32]},
            "sampleRate": {"anyOf": [{"type": "integer", "enum": [22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000]}, {"type": "string", "enum": ["project"]}]},
            "channels": {"type": "integer", "enum": [1, 2]},
            "folder": {"type": "string", "minLength": 1},
            "overwrite": {"type": "boolean"}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct DeliveryTool<B: DeliveryBackend> {
    owner: Rc<Delivery<B>>,
}

#[async_trait(?Send)]
impl<B: DeliveryBackend + 'static> KernelTool for DeliveryTool<B> {
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
