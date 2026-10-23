//! (M6) A sample into Logic's Sampler on a new track, as the bridge's `sample-load` makes it over the real Logic
//! (`crates/logic-kumi/src/changes/sample.rs`, its `change-sample-load` scene): the audio file imported onto an audio
//! track at the playhead (File > Import > Audio File…, which Logic names "Insert" in its history), then Track > Convert
//! Regions to New Sampler Track… on the region Logic selected. Logic makes a software instrument track below the audio
//! track, named after the instrument, with the Sampler and a MIDI region of its trigger note a whole bar or more long
//! where the audio region was, and leaves the audio region muted. The epoch moves twice (the region, then the track).
//! The restore is Logic's own undo of the conversion while it is on top, then of the import right under it. Logic
//! copies the file into the project's package; the simulated Logic only checks it is there.
use super::mixer::sampler_channel;
use super::{bad, changed, failed, quoted, Answer, Fault, Region, State, Track, UndoStep, BEATS_PER_BAR, TICKS_PER_BEAT};
use serde_json::{json, Value};
use std::path::Path;

/// Logic's names for the two steps a load makes, the later first.
const STEP: &str = "Convert Regions to New Sampler Track";
const IMPORT_STEP: &str = "Insert";
/// The files the import takes, by extension (the bridge's `AUDIO`).
const AUDIO: [&str; 8] = ["wav", "wave", "aif", "aiff", "aifc", "caf", "mp3", "m4a"];
/// The note the conversion's zone plays on (C3, Logic's middle C by default).
const TRIGGER: i64 = 60;
/// Why Quick Sampler isn't one of the channels, as the bridge says it.
const QUICK_SAMPLER: &str = "Logic Pro's bridge can't load a sample into Quick Sampler: Logic converts audio regions into a Sampler, Alchemy or Drum Machine Designer track (whose pad holds the sample in a Quick Sampler, inside a Track Stack), and Quick Sampler takes a sample by a drag onto its window; ask for kind sampler";

/// What a request asks, checked as the bridge's `plan` checks it.
struct Plan {
    file: String,
    /// The audio track the file comes in on (from 0), and its label.
    track: usize,
    label: String,
    position: Option<f64>,
    name: String,
}

/// The file's name without its extension, as Logic names the region and the instrument.
fn stem(file: &str) -> String {
    Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// How long a WAV file plays, from its header: its data's bytes over its byte rate. None for another file, or one
/// whose header can't be read.
fn seconds_of(file: &str) -> Option<f64> {
    let bytes = std::fs::read(file).ok()?;
    let tag = |at: usize| bytes.get(at..at + 4);
    if tag(0)? != b"RIFF" || tag(8)? != b"WAVE" {
        return None;
    }
    let (mut at, mut byte_rate) = (12usize, 0u32);
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        if tag(at)? == b"fmt " {
            byte_rate = u32::from_le_bytes(bytes.get(at + 16..at + 20)?.try_into().ok()?);
        }
        if tag(at)? == b"data" {
            return (byte_rate > 0).then(|| size as f64 / f64::from(byte_rate));
        }
        at += 8 + size + size % 2;
    }
    None
}

impl State {
    fn sample_plan(&self, input: &Value) -> Result<Plan, Fault> {
        match input.get("kind").and_then(Value::as_str) {
            Some("sampler") => {}
            Some("quick-sampler") => return Err(bad(QUICK_SAMPLER)),
            _ => return Err(bad("kind: sampler (Logic Pro's Sampler, on a new track) or quick-sampler")),
        }
        let file = input
            .get("file")
            .and_then(Value::as_str)
            .filter(|f| Path::new(f).is_absolute() && !f.chars().any(char::is_control))
            .ok_or_else(|| bad("file: the whole path of an audio file"))?
            .to_string();
        let extension = Path::new(&file).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !AUDIO.contains(&extension.as_str()) {
            return Err(bad(format!("file must be an audio file Logic Pro imports ({})", AUDIO.join(", "))));
        }
        if !Path::new(&file).is_file() {
            return Err(bad(format!(
                "{file} isn't there, or Kumi can't see it (macOS keeps ~/Music from a process without the Files and Folders permission)"
            )));
        }
        let position = match input.get("positionBeats") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_f64()
                    .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                    .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start"))?,
            ),
        };
        let name = match input.get("name") {
            None | Some(Value::Null) => stem(&file),
            Some(Value::String(name)) if !name.trim().is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control) => {
                name.clone()
            }
            Some(_) => return Err(bad("name must be one line of at most 64 characters")),
        };
        let audio = |at: usize| self.tracks[at].kind == "audio";
        let track = match input.get("ref").filter(|r| !r.is_null()) {
            Some(reference) => {
                let at = self.track_at(reference.as_str(), false)?;
                if !audio(at) {
                    return Err(bad(format!(
                        "{} isn't an audio track: Logic Pro converts an audio region into a Sampler, so the sample comes in on an audio track",
                        quoted(&self.track_label(at))
                    )));
                }
                at
            }
            None => (0..self.tracks.len())
                .find(|at| audio(*at) && self.tracks[*at].row["selected"] == true)
                .or_else(|| (0..self.tracks.len()).find(|at| audio(*at)))
                .ok_or_else(|| failed("the project has no audio track for the sample to come in on before Logic Pro converts it"))?,
        };
        Ok(Plan { file, label: self.track_label(track), track, position, name })
    }

    /// A track as the bridge's titles name it: its name, or its place when it has none.
    fn track_label(&self, at: usize) -> String {
        match self.tracks[at].name.as_str() {
            "" => format!("Track {}", at + 1),
            name => name.to_string(),
        }
    }

    fn sample_fence(&self) -> Value {
        let names: Vec<String> = self.tracks.iter().map(|track| track.name.clone()).collect();
        json!({"tracks": self.tracks.len(), "names": names, "regions": self.regions.len()})
    }

    /// `sample-load {kind: "sampler", file, ref?, positionBeats?, name?}`.
    pub(super) fn sample_load(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.sample_restore(params);
        }
        let plan = self.sample_plan(&params["input"]).map_err(|fault| {
            if method == "change_apply" && fault.0 == "failed" {
                changed(fault.1)
            } else {
                fault
            }
        })?;
        let fence = self.sample_fence();
        let title = |name: &str| format!("Loaded {} into a new Sampler track", quoted(name));
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": params["input"].get("ref").cloned().unwrap_or(Value::Null), "kind": "track"},
                "prior": {"tracks": fence["tracks"], "regions": fence["regions"]},
                "fence": fence,
                "display": {
                    "title": title(&plan.name),
                    "track": plan.label,
                    "note": format!("Logic Pro brings the file in on {} and converts it, leaving that audio region muted; it copies the file into the project's folder. Its own undo takes the load back while it is its latest step.", quoted(&plan.label)),
                },
            }));
        }
        if params["fence"] != fence {
            return Err(changed("the tracks or regions changed since the preview"));
        }
        let desc = Self::desc(params)?;
        // The file comes in at the playhead's bar and beat (or the beat asked), as long as it plays at the tempo.
        let start = plan.position.unwrap_or(self.playhead).floor();
        let beats = seconds_of(&plan.file).map_or(BEATS_PER_BAR, |seconds| seconds * self.tempo / 60.0);
        let source_end = ((start + beats) * 1e6).round() / 1e6;
        let (source_track, source_id) = (self.tracks[plan.track].id, self.next_region);
        self.next_region += 1;
        self.regions.push(Region {
            id: source_id,
            track: source_track,
            name: format!("{}, muted", stem(&plan.file)),
            kind: "audio".into(),
            start,
            end: source_end,
            looping: false,
            selected: false,
            notes: vec![],
        });
        self.select_region(source_id);
        self.sort_regions();
        self.bump_epoch();
        // The conversion: a software instrument track below the audio track, named after the instrument, playing the
        // Sampler; its MIDI region holds the trigger note, a whole bar or more long, where the audio region was.
        let mut track = Track::made(self.next_track, &plan.name, "instrument");
        track.channel = sampler_channel(&plan.name);
        let track_id = track.id;
        self.next_track += 1;
        self.tracks.insert(plan.track + 1, track);
        let end = start + (beats / BEATS_PER_BAR).ceil().max(1.0) * BEATS_PER_BAR;
        let trigger_id = self.next_region;
        self.next_region += 1;
        self.regions.push(Region {
            id: trigger_id,
            track: track_id,
            name: plan.name.clone(),
            kind: "midi".into(),
            start,
            end,
            looping: false,
            selected: false,
            notes: vec![json!({"beat": 0.0, "channel": 1, "lengthTicks": ((end - start) * TICKS_PER_BEAT).round() as i64, "pitch": TRIGGER, "velocity": 100})],
        });
        self.select_region(trigger_id);
        self.sort_regions();
        self.bump_epoch();
        let undo = self.applied_steps(&[IMPORT_STEP, STEP], &desc)?;
        let place = plan.track + 2;
        let region_ref = |state: &State, id: u64| state.region_index(id).map(|at| state.region_ref(at + 1));
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {
                "file": plan.file, "kind": "sampler", "track": place, "name": plan.name, "instrument": "Sampler",
                "startBeats": start, "endBeats": end, "sourceTrack": plan.track + 1, "answered": [],
            },
            "created": {
                "ref": format!("{}:track:{place}", self.epoch), "kind": "track",
                "region": region_ref(self, trigger_id), "source": region_ref(self, source_id),
            },
            "display": {
                "title": title(&plan.name),
                "note": format!("Logic Pro left the audio region it converted muted on {} and copied the file into the project's folder.", quoted(&plan.label)),
            },
            "undo": undo,
        }))
    }

    /// Logic's own undo of the conversion while it is on top, then of the import right under it: the track and both
    /// regions go. A track that went already is said so; one that holds more than its trigger region now is `changed`.
    fn sample_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let created = params.get("created").filter(|created| !created.is_null()).ok_or_else(|| bad("created: the track the load made"))?;
        let name = params["applied"]["name"].as_str().unwrap_or_default().to_string();
        let there = self.track_at(created["ref"].as_str(), true).ok().filter(|at| self.tracks[*at].name == name);
        let Some(at) = there else {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"deleted": created["ref"]},
                "display": {"title": format!("{} is gone already", quoted(&name))},
                "undo": {"desc": desc, "entry": false},
            }));
        };
        let track_id = self.tracks[at].id;
        let held = self.regions.iter().filter(|region| region.track == track_id).count();
        if params["discard"] != true && held > 1 {
            return Err(changed(format!("the Sampler track Kumi made ({}) holds {held} regions now", quoted(&name))));
        }
        let key = Self::key_of(params);
        let mut pressed = vec![self.undo_in_logic(STEP, |top| top.key.as_deref() == Some(key.as_str()))?.title];
        // (the import right under it is Kumi's too: nothing came between them)
        let import_on_top =
            self.undo.last().is_some_and(|top| top.title == format!("Undo {IMPORT_STEP}") && top.key.as_deref() == Some(key.as_str()));
        if import_on_top {
            let UndoStep { title, .. } = self.undo.pop().expect("the import's step");
            pressed.push(title);
        }
        if self.restore_unanswered() {
            return Err(super::uncertain(format!("Logic Pro's {:?} was pressed, but the Sampler track is still there", pressed[0])));
        }
        let source_track = params["applied"]["sourceTrack"].as_u64().unwrap_or(0) as usize;
        let source_id = source_track.checked_sub(1).and_then(|place| self.tracks.get(place)).map(|track| track.id);
        let stem_of = stem(params["applied"]["file"].as_str().unwrap_or_default());
        self.regions.retain(|region| region.track != track_id);
        if import_on_top {
            // (the audio region the sample came in on, muted where the import left it)
            if let Some(index) = self
                .regions
                .iter()
                .rposition(|region| Some(region.track) == source_id && region.kind == "audio" && region.name.starts_with(&stem_of))
            {
                self.regions.remove(index);
            }
        }
        self.tracks.remove(at);
        self.sort_regions();
        self.bump_epoch();
        let mut note = "Put back with Logic Pro's own undo; the copies of the file stay in the project's folder.".to_string();
        if !import_on_top {
            note = format!("{note} The audio region the sample came in on stays: Logic Pro's Edit menu says {:?}.", self.undo_title());
        }
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"deleted": created["ref"], "pressed": pressed},
            "display": {"title": format!("{} taken back", quoted(&name)), "note": note},
            "undo": {"desc": desc, "entry": false, "how": "undo"},
        }))
    }
}
