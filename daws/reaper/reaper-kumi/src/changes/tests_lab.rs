//! The fake REAPER lab the M4 track, routing, project and action tests share (r-tracks): a project, the changes and reads
//! over it, the events the watch sends, and a change made as Kumi's host makes it.

use std::cell::RefCell;

use serde_json::{json, Value};

use super::*;
use crate::reaper::fake;

#[derive(Default)]
pub struct Events(RefCell<Vec<(String, Value)>>);

impl Outbox for Events {
    fn event(&self, name: &str, data: Value) {
        self.0.borrow_mut().push((name.to_string(), data));
    }
    fn heartbeat(&self) {}
}

impl Events {
    pub fn take(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    /// The descriptions an `undone_in_daw` event named for `how`, over every event since the last take.
    pub fn told(&self, how: &str) -> Vec<String> {
        self.take()
            .into_iter()
            .filter(|(name, data)| name == "undone_in_daw" && data["how"] == how)
            .flat_map(|(_, data)| data["descs"].as_array().cloned().unwrap_or_default())
            .filter_map(|desc| desc.as_str().map(str::to_string))
            .collect()
    }
}

pub struct Lab {
    pub changes: Changes,
    pub reads: Reads,
    pub events: Events,
}

impl Lab {
    pub fn new() -> Lab {
        fake::lab_project();
        let lab = Lab { changes: Changes::default(), reads: Reads::default(), events: Events::default() };
        lab.tick();
        lab
    }

    pub fn call(&self, method: &str, mut params: Value) -> Answer {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        if params.get("epoch").is_none() && !matches!(method, "transport_stop" | "transport_play" | "undo_step_begin" | "undo_step_end") {
            params["epoch"] = self.reads.epoch().into();
        }
        match method {
            "change_preview" => self.changes.preview(&reaper, &self.reads, &params),
            "change_apply" => self.changes.apply(&reaper, &self.reads, &params),
            "change_restore" => self.changes.restore(&reaper, &self.reads, &params),
            "undo_step_begin" => self.changes.step_begin(&reaper, &self.reads, &params),
            "undo_step_end" => self.changes.step_end(&reaper, &self.reads, &params),
            "read" => self.reads.read(&reaper, &params),
            "project_save" => self.changes.project_save(&reaper, &self.reads, &params),
            "track_template_save" => self.changes.track_template_save(&reaper, &self.reads, &params),
            other => panic!("{other}"),
        }
    }

    pub fn tick(&self) {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        self.changes.tick(&reaper, &self.reads, reaper.current_project().as_ref(), &self.events);
    }

    pub fn preview(&self, kind: &str, input: Value) -> Answer {
        self.call("change_preview", json!({"kind": kind, "input": input}))
    }

    pub fn apply(&self, kind: &str, input: &Value, preview: &Value, desc: &str) -> Answer {
        self.call("change_apply", json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": desc}))
    }

    /// A change as Kumi's host makes it: preview, then apply with the preview's fence and a description.
    pub fn make(&self, kind: &str, input: Value, desc: &str) -> (Value, Value) {
        let preview = self.preview(kind, input.clone()).unwrap_or_else(|e| panic!("preview {kind}: {e:?}"));
        let applied = self.apply(kind, &input, &preview, desc).unwrap_or_else(|e| panic!("apply {kind}: {e:?}"));
        (preview, applied)
    }

    /// A change whose apply is refused: its code and message.
    pub fn make_err(&self, kind: &str, input: Value) -> (String, String) {
        let preview = self.preview(kind, input.clone()).unwrap_or_else(|e| panic!("preview {kind}: {e:?}"));
        self.apply(kind, &input, &preview, &format!("Kumi: {kind} (kerr0)")).expect_err("refused")
    }

    fn restore_params(kind: &str, preview: &Value, applied: &Value, desc: &str) -> Value {
        let mut params =
            json!({"kind": kind, "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": desc});
        for key in ["material", "created"] {
            if let Some(value) = applied.get(key) {
                params[key] = value.clone();
            }
        }
        params
    }

    pub fn undo(&self, kind: &str, preview: &Value, applied: &Value, desc: &str) -> Answer {
        self.call("change_restore", Lab::restore_params(kind, preview, applied, desc))
    }

    pub fn undo_anyway(&self, kind: &str, preview: &Value, applied: &Value, desc: &str) -> Answer {
        let mut params = Lab::restore_params(kind, preview, applied, desc);
        params["discard"] = true.into();
        self.call("change_restore", params)
    }
}

pub fn state<T>(f: impl FnOnce(&mut fake::State) -> T) -> T {
    fake::with(f)
}

/// The description of the point REAPER's Undo would take back.
pub fn top() -> Option<String> {
    let api = fake::api();
    let reaper = Reaper::new(&api);
    reaper.undo_can_undo(reaper.current_project().unwrap().ptr)
}

/// How many points REAPER's history has.
pub fn entries() -> i32 {
    state(|s| s.undo_entries)
}

pub const DRUMS: &str = "r:track:{AAAA}";
pub const KICK: &str = "r:track:{BBBB}";
pub const SNARE: &str = "r:track:{CCCC}";
pub const REVERB: &str = "r:track:{DDDD}";

/// The lab project with Drums inside a folder of its own: Rhythm › Drums › Kick, Snare (closing both); then Reverb, outside
/// them. A track at the end of Drums closes two folders.
pub fn drums_in_rhythm() {
    state(|s| {
        s.tracks.insert(0, fake::FakeTrack::new("{RRRR}", "Rhythm"));
        s.tracks[0].folder = 1;
        for track in s.tracks.iter_mut() {
            for send in track.sends.iter_mut() {
                send.dest += 1;
            }
        }
        s.tracks[3].folder = -2;
    });
}

/// Every track's folder depth, in order.
pub fn folder_depths() -> Vec<i32> {
    state(|s| s.tracks.iter().map(|t| t.folder).collect())
}
