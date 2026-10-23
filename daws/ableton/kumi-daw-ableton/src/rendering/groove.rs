//! Live's side of the groove judge: a clip's notes as Live's bridge gives them, how a part on a Drum Rack falls into
//! lanes, the clip read again so this answer may change it, the note edits HISTORY holds for a clip and the change that
//! moves notes. What the judge makes of them is `Listening`'s.
use super::super::notes::{clip_and_track, drum_rack, read_notes, track_name};
use super::*;
use kumi_listening::{
    notes::{Kit, Note},
    renderer::{ClipNotes, NotePatch},
};

/// The tools that change a clip's notes: what a groove round takes back, on its clip, when it isn't kept.
const NOTE_TOOLS: [&str; 4] = ["change_notes", "delete_notes", "edit_notes", "transform_midi"];

impl LiveRenderer {
    /// A clip's notes as Live has them: the ones it plays (not those before its start or past its end, which a split or
    /// shortened clip keeps), each with its id.
    pub(super) async fn clip_notes_of(&self, clip: &str, signal: Signal) -> Result<ClipNotes, String> {
        let input = object(json!({"clipRef": clip, "format": "json"}));
        let read = read_notes(&input, self.connection(), self.observer.tempo.get(), signal).await;
        let done: Value = serde_json::from_str(&read.text).map_err(|_| "Kumi couldn't read the clip's notes.".to_string())?;
        let clip = &done["clips"][0];
        if let Some(error) = clip["error"].as_str() {
            return Err(format!("{}: {error}", clip["clip"].as_str().unwrap_or("the clip")));
        }
        let (from, to, end) = window(clip);
        let notes: Vec<(i64, Note)> = clip["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["mute"].as_bool() != Some(true))
            .filter_map(|row| {
                Some((
                    row["id"].as_i64().unwrap_or(-1),
                    Note {
                        start: row["start"].as_f64()?,
                        length: row["duration"].as_f64().unwrap_or(0.25),
                        pitch: row["pitch"].as_i64()? as i32,
                        velocity: row["velocity"].as_f64().unwrap_or(100.),
                    },
                ))
            })
            .filter(|(_, note)| note.start >= from - 1e-9 && note.start < to - 1e-9)
            .collect();
        if notes.is_empty() {
            return Err(format!("{} has no notes to measure.", clip["name"].as_str().unwrap_or("The clip")));
        }
        let placement = &clip["placement"];
        let song_zero = placement["start"].as_f64().map(|start| start - placement["startMarker"].as_f64().unwrap_or(0.));
        Ok(ClipNotes { name: clip["name"].as_str().unwrap_or("the clip").to_string(), notes, song_zero, to, end })
    }

    /// The run's clip read again from Live (its row, by its slot or track), so this answer may change it.
    pub(super) async fn read_again(&self, clip: &str, signal: Signal) -> Result<(), RuntimeError> {
        let Some((long, track)) = clip_and_track(self.connection(), clip) else { return Ok(()) };
        let (kind, parent) = if long.contains(":arrangement_clip:") {
            ("arrangement-clip", track)
        } else {
            ("session-clip", long.replacen(":clip:", ":clip_slot:", 1))
        };
        self.rows(kind, json!({"parent": parent, "fields": ["name"]}), signal).await.map(|_| ())
    }

    /// How a clip's notes fall into lanes: by its track's Drum Rack, when it has one.
    pub(super) async fn lanes_of(&self, clip: &str, signal: Signal) -> Kit {
        let Some((_, track)) = clip_and_track(self.connection(), clip) else { return Kit::pitched() };
        drum_rack(self.connection(), &track, &signal).await.map_or_else(Kit::pitched, |pads| Kit::rack(&pads))
    }

    /// The changes applied since a checkpoint, split into the note edits on the run's clip (by its long ref) and the
    /// rest: what the producer changed in between, or a judge round kept, isn't a groove round's to take back.
    pub(super) fn note_changes_since(&self, checkpoint: &[String], clip: Option<&str>) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let entries = self.history.entries.borrow();
        self.applied_since(checkpoint).into_iter().partition(|(id, _)| {
            entries.get(id).is_some_and(|entry| {
                let entry = entry.borrow();
                entry.tool.as_deref().is_some_and(|tool| NOTE_TOOLS.contains(&tool)) && clip.is_some() && entry.clip.as_deref() == clip
            })
        })
    }

    /// The clip's long ref, from the ref the model has (a short one, or long).
    pub(super) fn clip_long(&self, clip: &str) -> Option<String> {
        clip_and_track(self.connection(), clip).map(|(long, _)| long)
    }

    /// The drum stem's track, by name, from the stem's Arrangement clip (its clipRef).
    pub(super) async fn stem_track_name(&self, audio: &str, signal: Signal) -> Result<String, String> {
        // The model has the stem's short ref: its long one says its track.
        let Some((_, track)) = clip_and_track(self.connection(), audio).filter(|(long, _)| long.contains(":arrangement_clip:")) else {
            return Err("audio is the drum stem's Arrangement clip (its clipRef).".into());
        };
        track_name(self.connection(), &track, signal).await.ok_or("Kumi couldn't find the drum stem's track.".to_string())
    }

    /// Notes moved in one change, so Live takes all of it or none, and HISTORY takes it back as one.
    pub(super) async fn change_note_patches(&self, clip: &str, patches: &[NotePatch], signal: Signal) -> Result<(), RuntimeError> {
        let patches: Vec<Value> = patches
            .iter()
            .map(|patch| {
                let mut row = json!({"id": patch.id, "start": patch.start, "velocity": patch.velocity});
                if let Some(duration) = patch.duration {
                    row["duration"] = json!(duration);
                }
                row
            })
            .collect();
        self.step("change_notes", json!({"clipRef": clip, "notes": patches}), signal).await?;
        Ok(())
    }
}

/// The part of a clip's own time Live plays (its notes there are heard), and where its notes have to end: the latest
/// of its length, loop end and end marker, as Live's bridge holds them. Unbounded where Live didn't say.
fn window(clip: &Value) -> (f64, f64, f64) {
    let (info, session) = match clip.get("loop") {
        Some(info) => (info, true),
        None => (&clip["placement"], false),
    };
    let number = |key: &str| info[key].as_f64().filter(|value| value.is_finite());
    let end = ["length", "loopEnd", "endMarker"].iter().filter_map(|key| number(key)).fold(f64::NEG_INFINITY, f64::max);
    let end = if end.is_finite() { end } else { f64::INFINITY };
    let looped = match (number("loopStart"), number("loopEnd")) {
        (Some(from), Some(to)) if info["looping"].as_bool() == Some(true) && to > from => Some((from, to)),
        _ => None,
    };
    let marker = number("startMarker").or(number("loopStart"));
    let unbounded = (f64::NEG_INFINITY, f64::INFINITY, end);
    if session {
        // A Session clip plays from its start marker: its loop over and over, or on to its end marker.
        return match (looped, marker) {
            (Some((from, to)), marker) => (marker.unwrap_or(from).min(from), to, end),
            (None, marker) => (marker.unwrap_or(f64::NEG_INFINITY), number("endMarker").unwrap_or(f64::INFINITY), end),
        };
    }
    // An Arrangement clip plays for its span from its start marker, its loop repeating once reached.
    let span = number("endTime").zip(number("start")).map(|(to, from)| to - from).or(number("length"));
    match (marker, span, looped) {
        (Some(marker), Some(span), Some((from, to))) if span > to - marker => (marker.min(from), to, end),
        (Some(marker), Some(span), _) => (marker, marker + span, end),
        _ => unbounded,
    }
}
