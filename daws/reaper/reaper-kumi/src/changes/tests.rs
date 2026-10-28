use std::cell::RefCell;
use std::time::Instant;

use serde_json::{json, Value};

use super::*;
use crate::reaper::fake::{self, FakeSend};

#[derive(Default)]
struct Events(RefCell<Vec<(String, Value)>>);

impl Outbox for Events {
    fn event(&self, name: &str, data: Value) {
        self.0.borrow_mut().push((name.to_string(), data));
    }
    fn heartbeat(&self) {}
}

impl Events {
    fn take(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

struct Lab {
    changes: Changes,
    reads: Reads,
    events: Events,
}

impl Lab {
    fn new() -> Lab {
        fake::lab_project();
        let lab = Lab { changes: Changes::default(), reads: Reads::default(), events: Events::default() };
        // The first look at the history: nothing to report.
        lab.tick();
        lab
    }

    fn call(&self, method: &str, mut params: Value) -> Answer {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        if params.get("epoch").is_none() && !matches!(method, "transport_stop" | "transport_play" | "undo_step_begin" | "undo_step_end") {
            params["epoch"] = self.reads.epoch().into();
        }
        match method {
            "change_preview" => self.changes.preview(&reaper, &self.reads, &params),
            "change_apply" => self.changes.apply(&reaper, &self.reads, &params),
            "change_restore" => self.changes.restore(&reaper, &self.reads, &params),
            "transport_stop" => self.changes.transport_stop(&reaper),
            "transport_play" => self.changes.transport_play(&reaper),
            "cursor_set" => self.changes.cursor_set(&reaper, &self.reads, &params),
            "undo_step_begin" => self.changes.step_begin(&reaper, &self.reads, &params),
            "undo_step_end" => self.changes.step_end(&reaper, &self.reads, &params),
            "read" => self.reads.read(&reaper, &params),
            other => panic!("{other}"),
        }
    }

    fn tick(&self) {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        self.changes.tick(&reaper, &self.reads, reaper.current_project().as_ref(), &self.events);
    }

    /// A change as Kumi's host makes it: preview, then apply with the preview's fence and a description.
    fn make(&self, kind: &str, input: Value, desc: &str) -> (Value, Value) {
        let preview = self.call("change_preview", json!({"kind": kind, "input": input})).unwrap();
        let applied = self
            .call("change_apply", json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": desc}))
            .unwrap_or_else(|e| panic!("{e:?}"));
        (preview, applied)
    }

    fn undo(&self, kind: &str, preview: &Value, applied: &Value, desc: &str) -> Answer {
        let mut params =
            json!({"kind": kind, "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": desc});
        if let Some(material) = applied.get("material") {
            params["material"] = material.clone();
        }
        if let Some(created) = applied.get("created") {
            params["created"] = created.clone();
        }
        self.call("change_restore", params)
    }
}

fn state<T>(f: impl FnOnce(&mut fake::State) -> T) -> T {
    fake::with(f)
}

fn top() -> Option<String> {
    let api = fake::api();
    let reaper = Reaper::new(&api);
    reaper.undo_can_undo(reaper.current_project().unwrap().ptr)
}

const KICK: &str = "r:track:{BBBB}";

#[test]
fn a_preview_writes_nothing_and_says_what_it_would_do() {
    let lab = Lab::new();
    let before = state(|s| (s.tracks[1].volume, s.undo_entries, s.state_count));
    let preview = lab.call("change_preview", json!({"kind": "track-volume", "input": {"ref": KICK, "volumeDb": -6.0}})).unwrap();
    assert_eq!(preview["epoch"], lab.reads.epoch());
    assert_eq!(preview["target"], json!({"ref": KICK, "kind": "track"}));
    assert_eq!(preview["prior"], json!({"volume": 1.0, "volumeDb": 0.0}));
    assert_eq!(preview["fence"], preview["prior"]);
    assert_eq!(preview["display"]["title"], "Kick volume −6 dB");
    assert_eq!(state(|s| (s.tracks[1].volume, s.undo_entries, s.state_count)), before);
}

#[test]
fn an_apply_is_one_named_undo_block_and_a_restore_puts_the_value_back_as_another() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), "Kumi: Kick volume −6 dB (k1a2b)");
    assert!((state(|s| s.tracks[1].volume) - 10f64.powf(-6.0 / 20.0)).abs() < 1e-12);
    assert_eq!(applied["applied"]["volumeDb"], -6.0);
    assert_eq!(applied["undo"]["desc"], "Kumi: Kick volume −6 dB (k1a2b)");
    assert_eq!(applied["undo"]["entry"], true);
    assert_eq!(applied["undo"]["stateCount"], state(|s| s.state_count));
    assert_eq!(top().as_deref(), Some("Kumi: Kick volume −6 dB (k1a2b)"));
    // The producer's fader hasn't moved since: Kumi's undo puts the volume back, as a point of its own.
    let restored = lab.undo("track-volume", &preview, &applied, "Kumi: undo Kick volume −6 dB (k9z9z)").unwrap();
    assert_eq!(restored["restored"], preview["prior"]);
    assert_eq!(state(|s| s.tracks[1].volume), 1.0);
    assert_eq!(top().as_deref(), Some("Kumi: undo Kick volume −6 dB (k9z9z)"));
    assert_eq!(restored["undo"]["entry"], true);
    // A retry of the restore finds it put back and writes nothing more.
    let again = lab.undo("track-volume", &preview, &applied, "Kumi: undo Kick volume −6 dB (k9z9z)").unwrap();
    assert_eq!(again["restored"], preview["prior"]);
}

#[test]
fn a_change_is_refused_when_what_it_fenced_has_moved_and_nothing_is_written() {
    let lab = Lab::new();
    let input = json!({"ref": KICK, "volumeDb": -6.0});
    let preview = lab.call("change_preview", json!({"kind": "track-volume", "input": input})).unwrap();
    // The producer moved the fader between the preview and the apply.
    state(|s| s.tracks[1].volume = 0.5);
    let entries = state(|s| s.undo_entries);
    let (code, message) = lab
        .call("change_apply", json!({"kind": "track-volume", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}))
        .unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("Kick") && message.contains("volume"), "{message}");
    assert_eq!((state(|s| s.tracks[1].volume), state(|s| s.undo_entries)), (0.5, entries));
    // A track that went between the preview and the apply has changed too.
    state(|s| s.tracks.remove(1));
    let (code, _) = lab
        .call("change_apply", json!({"kind": "track-volume", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}))
        .unwrap_err();
    assert_eq!(code, "changed");
}

#[test]
fn a_restore_is_refused_when_the_producer_changed_the_value_since_unless_discarded() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-pan", json!({"ref": KICK, "pan": -0.3}), "Kumi: Kick pan 30% left (k1111)");
    assert_eq!(state(|s| s.tracks[1].pan), -0.3);
    assert_eq!(preview["display"]["title"], "Kick pan 30% left");
    state(|s| s.tracks[1].pan = 0.8);
    let (code, message) = lab.undo("track-pan", &preview, &applied, "Kumi: undo (k2222)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("changed in REAPER since Kumi set it"), "{message}");
    assert_eq!(state(|s| s.tracks[1].pan), 0.8);
    let mut params = json!({"kind": "track-pan", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": "Kumi: undo (k2222)", "discard": true});
    params["epoch"] = lab.reads.epoch().into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(state(|s| s.tracks[1].pan), 0.0);
}

#[test]
fn every_attribute_is_applied_and_put_back_exactly() {
    let lab = Lab::new();
    state(|s| s.tracks[1].solo = 2);
    let cases = [
        ("track-mute", json!({"ref": KICK, "mute": true}), "Kick muted"),
        ("track-solo", json!({"ref": KICK, "solo": false}), "Kick unsoloed"),
        ("track-rename", json!({"ref": KICK, "name": "Kick 1"}), "Renamed track “Kick” → “Kick 1”"),
        ("track-color", json!({"ref": KICK, "color": "#ff0000"}), "Kick colour changed"),
        ("track-color", json!({"ref": KICK, "color": null}), "Kick colour cleared"),
    ];
    for (n, (kind, input, title)) in cases.into_iter().enumerate() {
        // Clearing a colour needs one to clear.
        if title == "Kick colour cleared" {
            state(|s| s.tracks[1].color = 0x0000ff | 0x100_0000);
        }
        let before = state(|s| (s.tracks[1].mute, s.tracks[1].solo, s.tracks[1].name.clone(), s.tracks[1].color));
        let (preview, applied) = lab.make(kind, input, &format!("Kumi: {title} (k{n}000)"));
        assert_eq!(preview["display"]["title"], title);
        let changed = state(|s| (s.tracks[1].mute, s.tracks[1].solo, s.tracks[1].name.clone(), s.tracks[1].color));
        assert_ne!(changed, before, "{kind} changed nothing");
        lab.undo(kind, &preview, &applied, &format!("Kumi: undo (k{n}999)")).unwrap();
        assert_eq!(state(|s| (s.tracks[1].mute, s.tracks[1].solo, s.tracks[1].name.clone(), s.tracks[1].color)), before, "{kind}");
    }
    // A colour is REAPER's own order with the flag that says it is set.
    lab.make("track-color", json!({"ref": KICK, "color": "#102030"}), "Kumi: c (k0001)");
    assert_eq!(state(|s| s.tracks[1].color), 0x10 | 0x20 << 8 | 0x30 << 16 | 0x100_0000);
}

#[test]
fn a_bad_request_is_refused_before_anything_is_written() {
    let lab = Lab::new();
    for (kind, input, code) in [
        ("track-volume", json!({"ref": KICK, "volumeDb": "loud"}), "bad-params"),
        ("track-volume", json!({"ref": KICK, "volumeDb": 40.0}), "bad-params"),
        ("track-pan", json!({"ref": KICK, "pan": 2.0}), "bad-params"),
        ("track-mute", json!({"ref": KICK}), "bad-params"),
        ("track-rename", json!({"ref": KICK, "name": "a\nb"}), "bad-params"),
        ("track-color", json!({"ref": KICK, "color": "red"}), "bad-params"),
        ("track-volume", json!({"ref": "r:item:{ITEM-K1}", "volumeDb": 0.0}), "bad-params"),
        ("track-volume", json!({"ref": "r:track:{NOPE}", "volumeDb": 0.0}), "failed"),
        ("track-rename", json!({"ref": "r:track:master", "name": "x"}), "bad-params"),
        ("track-solo", json!({"ref": "r:track:master", "solo": true}), "bad-params"),
        ("track-delete", json!({"ref": "r:track:master"}), "bad-params"),
        ("track-warp", json!({"ref": KICK}), "bad-params"),
    ] {
        let (got, message) = lab.call("change_preview", json!({"kind": kind, "input": input})).unwrap_err();
        assert_eq!(got, code, "{kind} {input}: {message}");
    }
    assert_eq!(lab.call("change_preview", json!({"kind": "track-volume"})).unwrap_err().0, "bad-params");
    // The master can be given a volume, a pan, a mute and a colour.
    for (kind, input) in
        [("track-volume", json!({"volumeDb": -3.0})), ("track-pan", json!({"pan": 0.5})), ("track-mute", json!({"mute": true}))]
    {
        let mut input = input;
        input["ref"] = "r:track:master".into();
        lab.make(kind, input, "Kumi: master (k1111)");
    }
    assert_eq!(state(|s| s.master.pan), 0.5);
}

#[test]
fn a_request_from_another_epoch_or_while_reaper_is_busy_is_refused() {
    let lab = Lab::new();
    let input = json!({"ref": KICK, "volumeDb": -6.0});
    let (code, message) = lab.call("change_preview", json!({"kind": "track-volume", "input": input, "epoch": "old-1"})).unwrap_err();
    assert_eq!(code, "stale");
    assert!(message.contains("old-1"), "{message}");
    assert_eq!(lab.call("change_preview", json!({"kind": "track-volume", "input": input, "epoch": null})).unwrap_err().0, "bad-params");
    let preview = lab.call("change_preview", json!({"kind": "track-volume", "input": input})).unwrap();
    // The tab changed between the preview and the apply.
    lab.reads.bump_epoch();
    let apply =
        json!({"kind": "track-volume", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)", "epoch": preview["epoch"]});
    assert_eq!(lab.call("change_apply", apply.clone()).unwrap_err().0, "stale");
    assert_eq!(state(|s| s.tracks[1].volume), 1.0);
    // And while REAPER loads, renders or has a dialog, nothing is written.
    for set in [|s: &mut fake::State| s.loading = true, |s: &mut fake::State| s.rendering = true] {
        state(set);
        let mut apply = apply.clone();
        apply["epoch"] = lab.reads.epoch().into();
        assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "busy");
        assert_eq!(state(|s| s.tracks[1].volume), 1.0);
        state(|s| {
            s.loading = false;
            s.rendering = false;
        });
    }
}

#[test]
fn a_change_that_refuses_leaves_no_undo_point() {
    let lab = Lab::new();
    let input = json!({"ref": KICK, "volumeDb": -6.0});
    let preview = lab.call("change_preview", json!({"kind": "track-volume", "input": input})).unwrap();
    state(|s| s.tracks[1].volume = 0.5);
    let entries = state(|s| s.undo_entries);
    let _ = lab.call("change_apply", json!({"kind": "track-volume", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}));
    assert_eq!(state(|s| s.undo_entries), entries);
    assert_eq!(state(|s| s.undo_depth), 0, "the block was closed");
}

// ----- deleting a track -----

fn sends_of(lab: &Lab, track: &str) -> Vec<Value> {
    let api = fake::api();
    let reads = &lab.reads;
    let answer = reads.read(&Reaper::new(&api), &json!({"kind": "send", "parent": track})).unwrap();
    answer["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| json!({"ref": row["ref"], "volumeDb": row["volumeDb"], "category": row["category"]}))
        .collect()
}

#[test]
fn deleting_a_track_keeps_what_undo_needs_to_make_it_again_with_its_sends_and_state() {
    let lab = Lab::new();
    let reverb = "r:track:{DDDD}";
    state(|s| {
        s.tracks[0].sends[0].volume = 0.25;
        s.tracks[0].sends[0].extra.insert("B_PHASE".into(), 1.0);
        s.tracks[3].selected = true;
    });
    let drums_before = sends_of(&lab, "r:track:{AAAA}");
    let reverb_before = sends_of(&lab, reverb);
    let fx_before = state(|s| s.tracks[3].fx.len());
    let (preview, applied) = lab.make("track-delete", json!({"ref": reverb}), "Kumi: Deleted track Reverb (kdel1)");
    assert_eq!(preview["display"]["title"], "Deleted track “Reverb”");
    assert_eq!(preview["prior"]["index"], 4);
    assert_eq!(state(|s| s.tracks.len()), 3);
    // The send that went to it went with it.
    assert!(state(|s| s.tracks[0].sends.is_empty()));
    let material = &applied["material"];
    assert_eq!((material["guid"].as_str(), material["index"].as_i64()), (Some("{DDDD}"), Some(3)));
    assert!(material["chunk"].as_str().unwrap().starts_with("<TRACK {DDDD}"), "{}", material["chunk"]);
    assert_eq!(material["routes"].as_array().unwrap().len(), 1);
    assert_eq!(material["routes"][0]["other"], "{AAAA}");
    assert_eq!(applied["applied"], json!({"deleted": true}));
    // Undo makes it again: the same id, the same place, its FX, its send with its values.
    let restored = lab.undo("track-delete", &preview, &applied, "Kumi: undo Deleted track Reverb (kund1)").unwrap();
    assert_eq!(restored["restored"]["ref"], reverb);
    assert_eq!(state(|s| s.tracks.len()), 4);
    assert_eq!(state(|s| s.tracks[3].guid.clone()), "{DDDD}");
    assert_eq!(state(|s| s.tracks[3].fx.len()), fx_before);
    assert_eq!(sends_of(&lab, "r:track:{AAAA}"), drums_before);
    assert_eq!(sends_of(&lab, reverb), reverb_before);
    assert_eq!(state(|s| s.tracks[0].sends[0].extra.get("B_PHASE").copied()), Some(1.0));
    assert!(state(|s| s.tracks[3].selected));
    assert_eq!(top().as_deref(), Some("Kumi: undo Deleted track Reverb (kund1)"));
    // The track can be read by the ref it had.
    let api = fake::api();
    let row = lab.reads.read(&Reaper::new(&api), &json!({"ref": reverb})).unwrap();
    assert_eq!(row["items"][0]["name"], "Reverb");
}

#[test]
fn undo_of_a_delete_is_refused_if_the_track_is_back_or_its_state_cant_be_put_back() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-delete", json!({"ref": KICK}), "Kumi: Deleted track Kick (kdel1)");
    // Put back once: a track with that id exists again, and nothing is made twice.
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    let (code, _) = lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund2)").unwrap_err();
    assert_eq!(code, "changed");
    assert_eq!(state(|s| s.tracks.len()), 4);
    // A chunk REAPER won't take: what was made is taken away again.
    state(|s| s.tracks.remove(1));
    let mut broken = applied.clone();
    broken["material"]["chunk"] = "<TRACK {BBBB}\nnot a state\n>".into();
    let (code, _) = lab.undo("track-delete", &preview, &broken, "Kumi: undo (kund3)").unwrap_err();
    assert_eq!(code, "failed");
    assert_eq!(state(|s| s.tracks.len()), 3, "the track made for it was removed again");
}

#[test]
fn a_delete_is_refused_when_the_track_changed_since_the_preview() {
    let lab = Lab::new();
    let preview = lab.call("change_preview", json!({"kind": "track-delete", "input": {"ref": KICK}})).unwrap();
    state(|s| s.tracks[1].items.pop());
    let (code, _) = lab
        .call("change_apply", json!({"kind": "track-delete", "input": {"ref": KICK}, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}))
        .unwrap_err();
    assert_eq!(code, "changed");
    assert_eq!(state(|s| s.tracks.len()), 4);
}

#[test]
fn the_sends_a_track_receives_are_taken_out_of_its_chunk_and_nothing_else() {
    let chunk = "<TRACK {AAAA}\n  NAME \"Reverb\"\n  VOLPAN 1 0 -1 -1 1\n  AUXRECV 0 0 1 0 0 0 0 0 0 -1:U\n  <AUXVOLENV\n    EGUID {1111}\n    PT 0 1 0\n  >\n  <FXCHAIN\n    <VST \"ReaVerb\"\n      AUXRECV not a send\n    >\n  >\n  HWOUT 0 0 1 0 0 0 0 -1:U\n>\n";
    let (stripped, dropped) = tracks::without_receives(chunk);
    assert_eq!(dropped, 1);
    assert!(!stripped.contains("AUXRECV 0") && !stripped.contains("AUXVOLENV") && !stripped.contains("EGUID"), "{stripped}");
    assert!(stripped.contains("VOLPAN") && stripped.contains("<FXCHAIN") && stripped.contains("HWOUT"), "{stripped}");
    // A line that only looks like one, deeper in, stays.
    assert!(stripped.contains("AUXRECV not a send"));
    assert_eq!(tracks::without_receives("no chunk at all"), ("no chunk at all".to_string(), 0));
}

// ----- adding and moving tracks -----

fn names() -> Vec<String> {
    state(|s| s.tracks.iter().map(|t| t.name.clone()).collect())
}

fn depths() -> Vec<i32> {
    state(|s| s.tracks.iter().map(|t| t.folder).collect())
}

#[test]
fn adding_a_track_makes_it_where_asked_and_undo_takes_it_away_by_its_id() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-add", json!({"name": "Pad", "color": "#102030", "index": 2}), "Kumi: Added track Pad (kadd1)");
    assert_eq!(preview["display"]["title"], "Added track “Pad”");
    assert_eq!(preview["target"], json!({"ref": null, "kind": "track", "index": 1}));
    assert_eq!(names(), ["Drums", "Pad", "Kick", "Snare", "Reverb"]);
    assert_eq!(state(|s| s.tracks[1].color), 0x10 | 0x20 << 8 | 0x30 << 16 | 0x100_0000);
    let created = applied["created"]["ref"].as_str().unwrap().to_string();
    assert!(created.starts_with("r:track:{NEW-"), "{created}");
    assert_eq!((applied["applied"]["index"].clone(), applied["applied"]["name"].clone()), (json!(1), json!("Pad")));
    // The sends that named a track by place still reach it: Drums sends to Reverb, now one further on.
    assert_eq!(state(|s| s.tracks[0].sends[0].dest), 4);
    // (undo of an add is told what was created)
    let without = json!({"kind": "track-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": "Kumi: undo (kund0)", "epoch": lab.reads.epoch()});
    let restored = lab.call("change_restore", without);
    assert!(restored.is_err(), "created is needed: {restored:?}");
    let mut params = json!({"kind": "track-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo Added track Pad (kund1)"});
    params["epoch"] = lab.reads.epoch().into();
    let restored = lab.call("change_restore", params.clone()).unwrap();
    assert_eq!(restored["restored"]["deleted"], created);
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
    assert_eq!(state(|s| s.tracks[0].sends[0].dest), 3);
    // Taken away already: gone.
    assert_eq!(lab.call("change_restore", params).unwrap_err().0, "gone");
}

#[test]
fn a_track_the_producer_filled_since_is_not_deleted_by_undo_unless_discarded() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-add", json!({"name": "Pad"}), "Kumi: Added track Pad (kadd1)");
    state(|s| s.tracks[4].fx.push(crate::reaper::fake::FakeFx::new("{FX-NEW}", "ReaEQ")));
    let mut params = json!({"kind": "track-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo (kund1)"});
    params["epoch"] = lab.reads.epoch().into();
    let (code, message) = lab.call("change_restore", params.clone()).unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("has items, FX or sends"), "{message}");
    assert_eq!(state(|s| s.tracks.len()), 5);
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(state(|s| s.tracks.len()), 4);
}

#[test]
fn a_folder_is_a_track_added_before_the_run_it_holds_and_undo_closes_the_run_as_it_was() {
    let lab = Lab::new();
    assert_eq!(depths(), [1, 0, -1, 0]);
    // Kick and Snare (Snare closes Drums' folder) become the children of a new folder.
    let (preview, applied) =
        lab.make("track-add", json!({"name": "Subgroup", "asFolderOf": [KICK, "r:track:{CCCC}"]}), "Kumi: Added folder Subgroup (kfol1)");
    assert_eq!(preview["display"]["title"], "Added folder “Subgroup”");
    assert_eq!(names(), ["Drums", "Subgroup", "Kick", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 1, 0, -2, 0], "the new folder opens, and the run closes it and Drums");
    assert_eq!(applied["applied"]["closedAt"], json!({"guid": "{CCCC}", "before": -1, "after": -2}));
    let mut params = json!({"kind": "track-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo (kund1)"});
    params["epoch"] = lab.reads.epoch().into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(depths(), [1, 0, -1, 0]);
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
}

/// Removes the track at `at` as REAPER's own Remove tracks does: the track before takes over its closing, and the sends
/// after it close up.
fn remove_as_reaper_does(at: usize) {
    state(|s| {
        let gone = s.tracks.remove(at);
        if gone.folder < 0 && at > 0 {
            s.tracks[at - 1].folder += gone.folder;
        }
        for track in s.tracks.iter_mut() {
            track.sends.retain(|send| send.dest != at);
            for send in track.sends.iter_mut().filter(|send| send.dest > at) {
                send.dest -= 1;
            }
        }
    });
}

#[test]
fn undoing_an_added_track_that_took_over_a_folders_closing_leaves_every_other_track_in_its_folder() {
    let lab = Lab::new();
    super::tests_lab::drums_in_rhythm();
    assert_eq!(depths(), [1, 1, 0, -2, 0]);
    // Kumi adds Perc before Snare, in Drums; the producer removes Snare, and REAPER hands its closing of both folders to Perc.
    let (preview, applied) = lab.make("track-add", json!({"name": "Perc", "index": 4}), "Kumi: Added track Perc (kadd7)");
    assert_eq!(names(), ["Rhythm", "Drums", "Kick", "Perc", "Snare", "Reverb"]);
    remove_as_reaper_does(4);
    assert_eq!(depths(), [1, 1, 0, -2, 0]);
    lab.undo("track-add", &preview, &applied, "Kumi: undo (kund7)").unwrap();
    assert_eq!(names(), ["Rhythm", "Drums", "Kick", "Reverb"]);
    assert_eq!(depths(), [1, 1, -2, 0], "Kick closes both folders now, and Reverb stays out of them");
}

#[test]
fn a_folder_kumi_made_is_taken_away_around_a_track_the_producer_put_in_it_only_when_told_to_discard() {
    let lab = Lab::new();
    let (preview, applied) =
        lab.make("track-add", json!({"name": "Subgroup", "asFolderOf": [KICK, "r:track:{CCCC}"]}), "Kumi: Added folder Subgroup (kfol2)");
    assert_eq!(depths(), [1, 1, 0, -2, 0]);
    // The producer puts a track of theirs between Kick and Snare, inside Kumi's folder.
    state(|s| {
        s.tracks.insert(3, crate::reaper::fake::FakeTrack::new("{MINE}", "Mine"));
        for track in s.tracks.iter_mut() {
            for send in track.sends.iter_mut().filter(|send| send.dest >= 3) {
                send.dest += 1;
            }
        }
    });
    let (code, message) = lab.undo("track-add", &preview, &applied, "Kumi: undo (kund8)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("“Mine”"), "{message}");
    assert_eq!(names(), ["Drums", "Subgroup", "Kick", "Mine", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 1, 0, 0, -2, 0]);
    // Told to undo anyway, the folder goes, and the producer's track moves up with Kick and Snare, still in Drums.
    let mut params = json!({"kind": "track-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo (kund9)", "discard": true});
    params["epoch"] = lab.reads.epoch().into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(names(), ["Drums", "Kick", "Mine", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 0, 0, -1, 0]);
}

#[test]
fn a_folder_is_made_only_of_tracks_of_one_folder_each_with_all_of_its_tracks() {
    let lab = Lab::new();
    assert_eq!(depths(), [1, 0, -1, 0]);
    for (refs, why) in [
        // Drums alone: its tracks would go into the new folder without it.
        (json!(["r:track:{AAAA}"]), "with all of its tracks"),
        // Snare and Reverb: past the end of Drums.
        (json!(["r:track:{CCCC}", "r:track:{DDDD}"]), "one folder"),
    ] {
        let (code, message) = lab.call("change_preview", json!({"kind": "track-add", "input": {"asFolderOf": refs}})).unwrap_err();
        assert_eq!(code, "bad-params", "{refs}");
        assert!(message.contains(why), "{refs}: {message}");
    }
    // Drums with its tracks, and Reverb after them: one folder's worth at the top.
    let (preview, applied) = lab.make(
        "track-add",
        json!({"name": "Bus", "asFolderOf": ["r:track:{AAAA}", KICK, "r:track:{CCCC}", "r:track:{DDDD}"]}),
        "Kumi: Added folder Bus (kfol3)",
    );
    assert_eq!(depths(), [1, 1, 0, -1, -1]);
    lab.undo("track-add", &preview, &applied, "Kumi: undo (kund9)").unwrap();
    assert_eq!(depths(), [1, 0, -1, 0]);
}

#[test]
fn a_bad_add_is_refused_and_one_whose_place_moved_is_changed() {
    let lab = Lab::new();
    for (input, why) in [
        (json!({"index": 9}), "index"),
        (json!({"index": 0}), "from 1"),
        (json!({"index": 6}), "from 1"),
        (json!({"index": -1}), "index"),
        (json!({"name": "a\nb"}), "name"),
        (json!({"color": "red"}), "color"),
        (json!({"asFolderOf": []}), "asFolderOf"),
        (json!({"asFolderOf": ["r:track:master"]}), "master"),
        (json!({"asFolderOf": [KICK, "r:track:{DDDD}"]}), "next to each other"),
        (json!({"asFolderOf": ["r:track:{CCCC}", KICK]}), "next to each other"),
        (json!({"asFolderOf": [KICK], "index": 1}), "not both"),
    ] {
        let (code, message) = lab.call("change_preview", json!({"kind": "track-add", "input": input})).unwrap_err();
        assert_eq!(code, "bad-params", "{input}");
        assert!(message.contains(why), "{input}: {message}");
    }
    let preview = lab.call("change_preview", json!({"kind": "track-add", "input": {"name": "Pad", "index": 2}})).unwrap();
    state(|s| s.tracks.insert(1, crate::reaper::fake::FakeTrack::new("{OTHER}", "Other")));
    let (code, _) = lab
        .call(
            "change_apply",
            json!({"kind": "track-add", "input": {"name": "Pad", "index": 2}, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}),
        )
        .unwrap_err();
    assert_eq!(code, "changed");
    assert_eq!(state(|s| s.tracks.len()), 5);
}

#[test]
fn moving_a_track_puts_it_there_keeps_the_selection_and_undo_puts_it_and_the_depths_back() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[3].selected = true;
    });
    let (preview, applied) = lab.make("track-move", json!({"ref": KICK, "index": 4}), "Kumi: Moved track Kick to position 4 (kmov1)");
    assert_eq!(preview["display"]["title"], "Moved track “Kick” to position 4");
    assert_eq!(names(), ["Drums", "Snare", "Reverb", "Kick"]);
    assert_eq!(applied["applied"]["index"], 3);
    // The producer's selection (Reverb) is as it was, and Kick isn't selected.
    assert_eq!(state(|s| s.tracks.iter().map(|t| t.selected).collect::<Vec<_>>()), [false, false, true, false]);
    // Drums' send to Reverb follows Reverb.
    assert_eq!(state(|s| s.tracks[0].sends[0].dest), 2);
    lab.undo("track-move", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 0, -1, 0]);
    assert_eq!(state(|s| s.tracks[0].sends[0].dest), 3);
    assert_eq!(state(|s| s.tracks.iter().map(|t| t.selected).collect::<Vec<_>>()), [false, false, false, true]);
}

#[test]
fn a_move_is_refused_when_the_tracks_around_it_changed_before_or_after() {
    let lab = Lab::new();
    let preview = lab.call("change_preview", json!({"kind": "track-move", "input": {"ref": KICK, "index": 3}})).unwrap();
    state(|s| s.tracks.swap(2, 3));
    let (code, _) = lab
        .call(
            "change_apply",
            json!({"kind": "track-move", "input": {"ref": KICK, "index": 3}, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}),
        )
        .unwrap_err();
    assert_eq!(code, "changed");
    state(|s| s.tracks.swap(2, 3));
    let (preview, applied) = lab.make("track-move", json!({"ref": KICK, "index": 3}), "Kumi: Moved (kmov1)");
    assert_eq!(names(), ["Drums", "Snare", "Kick", "Reverb"]);
    // The producer moved another track next to it since.
    state(|s| s.tracks.swap(0, 1));
    let (code, message) = lab.undo("track-move", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("moved or changed in REAPER since"), "{message}");
    assert_eq!(names(), ["Snare", "Drums", "Kick", "Reverb"]);
    // Index out of range, the master, and no ref are refused up front.
    for input in [
        json!({"ref": KICK, "index": 5}),
        json!({"ref": KICK, "index": 0}),
        json!({"ref": KICK}),
        json!({"ref": "r:track:master", "index": 1}),
        json!({"index": 1}),
    ] {
        assert_eq!(lab.call("change_preview", json!({"kind": "track-move", "input": input})).unwrap_err().0, "bad-params", "{input}");
    }
}

#[test]
fn moving_a_track_to_where_it_is_changes_nothing() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-move", json!({"ref": KICK, "index": 2}), "Kumi: Moved (kmov1)");
    assert_eq!(preview["display"]["unchanged"], true);
    assert_eq!(applied["undo"]["entry"], false, "nothing was written, so REAPER made no point");
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
}

// ----- sends -----

const DRUMS_TO_REVERB: &str = "r:send:{AAAA}:0:{DDDD}";

#[test]
fn a_sends_volume_pan_mute_and_mode_are_changed_and_put_back_exactly() {
    let lab = Lab::new();
    state(|s| s.tracks[0].sends[0].mode = 3);
    let cases = [
        ("send-volume", json!({"ref": DRUMS_TO_REVERB, "volumeDb": -12.0}), "Send “Drums” → “Reverb” −12 dB"),
        ("send-pan", json!({"ref": DRUMS_TO_REVERB, "pan": 0.5}), "Send “Drums” → “Reverb” pan 50% right"),
        ("send-mute", json!({"ref": DRUMS_TO_REVERB, "mute": true}), "Send “Drums” → “Reverb” muted"),
        ("send-mode", json!({"ref": DRUMS_TO_REVERB, "mode": "pre-fx"}), "Send “Drums” → “Reverb” pre-fx"),
    ];
    for (n, (kind, input, title)) in cases.into_iter().enumerate() {
        let before = state(|s| {
            let send = &s.tracks[0].sends[0];
            (send.volume, send.pan, send.mute, send.mode)
        });
        let (preview, applied) = lab.make(kind, input, &format!("Kumi: {title} (ks{n}00)"));
        assert_eq!(preview["target"], json!({"ref": DRUMS_TO_REVERB, "kind": "send"}));
        // (m4-tui) HISTORY's picture: the route, changed, with only the value that moved.
        let route = &applied["display"]["route"];
        assert_eq!(
            (route["from"].as_str(), route["to"].as_str(), route["change"].as_str()),
            (Some("Drums"), Some("Reverb"), Some("changed"))
        );
        assert_eq!(route["values"].as_array().map(Vec::len), Some(1), "{kind}: {route}");
        assert_eq!(preview["display"]["title"], title);
        assert_eq!(applied["undo"]["entry"], true);
        let changed = state(|s| {
            let send = &s.tracks[0].sends[0];
            (send.volume, send.pan, send.mute, send.mode)
        });
        assert_ne!(changed, before, "{kind}");
        lab.undo(kind, &preview, &applied, &format!("Kumi: undo (ks{n}99)")).unwrap();
        assert_eq!(
            state(|s| {
                let send = &s.tracks[0].sends[0];
                (send.volume, send.pan, send.mute, send.mode)
            }),
            before,
            "{kind}"
        );
    }
}

#[test]
fn a_send_change_is_refused_when_the_send_or_its_neighbours_moved() {
    let lab = Lab::new();
    let input = json!({"ref": DRUMS_TO_REVERB, "volumeDb": -6.0});
    let preview = lab.call("change_preview", json!({"kind": "send-volume", "input": input})).unwrap();
    // The producer added a second send to the same track: the ref names neither now.
    state(|s| s.tracks[0].sends.push(FakeSend::to(3)));
    let (code, message) = lab
        .call("change_apply", json!({"kind": "send-volume", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}))
        .unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("2 such send"), "{message}");
    assert!(state(|s| s.tracks[0].sends[0].volume == 1.0));
    // The value moved between the preview and the apply.
    state(|s| {
        s.tracks[0].sends.pop();
        s.tracks[0].sends[0].volume = 0.5;
    });
    assert_eq!(
        lab.call("change_apply", json!({"kind": "send-volume", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"}))
            .unwrap_err()
            .0,
        "changed"
    );
    // The ways to ask wrongly. (A receive's ref names its send since M4: see tests_routing.)
    for (kind, input) in [
        ("send-volume", json!({"ref": "r:send:{DDDD}:1:hw2", "volumeDb": 0.0})),
        ("send-volume", json!({"ref": "r:track:{AAAA}", "volumeDb": 0.0})),
        ("send-volume", json!({"ref": DRUMS_TO_REVERB, "volumeDb": 99.0})),
        ("send-mode", json!({"ref": DRUMS_TO_REVERB, "mode": "sideways"})),
        ("send-add", json!({"ref": "r:track:{AAAA}", "toRef": "r:track:{AAAA}"})),
        ("send-add", json!({"ref": "r:track:master", "toRef": "r:track:{AAAA}"})),
        ("send-add", json!({"ref": "r:track:{AAAA}"})),
    ] {
        assert_eq!(lab.call("change_preview", json!({"kind": kind, "input": input})).unwrap_err().0, "bad-params", "{kind} {input}");
    }
    assert_eq!(
        lab.call("change_preview", json!({"kind": "send-volume", "input": {"ref": "r:send:{AAAA}:0:{NOPE}", "volumeDb": 0.0}}))
            .unwrap_err()
            .0,
        "failed"
    );
}

#[test]
fn adding_a_send_makes_it_with_its_settings_and_undo_removes_exactly_that_one() {
    let lab = Lab::new();
    let input = json!({"ref": KICK, "toRef": "r:track:{DDDD}", "volumeDb": -9.0, "mode": "pre-fader"});
    let (preview, applied) = lab.make("send-add", input, "Kumi: Added send Kick → Reverb (ksa01)");
    assert_eq!(preview["display"]["title"], "Added send “Kick” → “Reverb”");
    // (m4-tui) What HISTORY draws: the route's ends, made, with its volume and taps.
    let route = &applied["display"]["route"];
    assert_eq!((route["from"].as_str(), route["to"].as_str(), route["change"].as_str()), (Some("Kick"), Some("Reverb"), Some("made")));
    let names: Vec<&str> = route["values"].as_array().unwrap().iter().filter_map(|value| value["name"].as_str()).collect();
    assert_eq!(names[..3], ["Volume", "Pan", "Taps"]);
    assert_eq!(route["values"][0]["toText"], "−9 dB");
    assert_eq!(applied["created"]["ref"], "r:send:{BBBB}:0:{DDDD}");
    assert_eq!(applied["created"]["kind"], "send");
    assert!(applied["created"]["stamp"].as_str().is_some_and(|stamp| !stamp.is_empty()), "Kumi's own send carries its mark");
    assert_eq!(state(|s| s.tracks[1].sends.len()), 1);
    assert!((state(|s| s.tracks[1].sends[0].volume) - 10f64.powf(-9.0 / 20.0)).abs() < 1e-12);
    assert_eq!(state(|s| s.tracks[1].sends[0].mode), 3);
    let mut params = json!({"kind": "send-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo (kund1)"});
    params["epoch"] = lab.reads.epoch().into();
    lab.call("change_restore", params.clone()).unwrap();
    assert!(state(|s| s.tracks[1].sends.is_empty()));
    assert_eq!(lab.call("change_restore", params).unwrap_err().0, "gone");
}

#[test]
fn a_second_send_to_the_same_track_is_told_apart_and_undone_alone() {
    let lab = Lab::new();
    let (preview, applied) = lab.make(
        "send-add",
        json!({"ref": "r:track:{AAAA}", "toRef": "r:track:{DDDD}", "volumeDb": -6.0}),
        "Kumi: Added send Drums → Reverb (ksa01)",
    );
    // One of two sends to Reverb: its ref carries the state of both, as a read would give it.
    assert_eq!(applied["created"]["ref"].as_str(), Some(send_refs(0)[1].as_str()));
    assert!(send_refs(0)[1].starts_with("r:send:{AAAA}:0:{DDDD}:2of2."), "{}", send_refs(0)[1]);
    assert_eq!(preview["display"]["note"], "There are 1 send(s) between them already.");
    // The old send's ref is now 1of2, so a ref made before reads nothing; the new one is told.
    let mut params = json!({"kind": "send-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo (kund1)"});
    params["epoch"] = lab.reads.epoch().into();
    // The producer changed the new send since.
    state(|s| s.tracks[0].sends[1].volume = 0.9);
    let (code, _) = lab.call("change_restore", params.clone()).unwrap_err();
    assert_eq!(code, "changed");
    state(|s| s.tracks[0].sends[1].volume = 10f64.powf(-6.0 / 20.0));
    lab.call("change_restore", params).unwrap();
    assert_eq!(state(|s| s.tracks[0].sends.len()), 1);
    assert_eq!(state(|s| s.tracks[0].sends[0].volume), 1.0);
}

#[test]
fn removing_a_send_keeps_its_settings_and_undo_makes_it_again() {
    let lab = Lab::new();
    state(|s| {
        let send = &mut s.tracks[0].sends[0];
        send.volume = 0.3;
        send.pan = -0.2;
        send.mode = 1;
        send.extra.insert("B_PHASE".into(), 1.0);
    });
    let (preview, applied) = lab.make("send-remove", json!({"ref": DRUMS_TO_REVERB}), "Kumi: Removed send Drums → Reverb (ksr01)");
    assert_eq!(preview["display"]["title"], "Removed send “Drums” → “Reverb”");
    assert!(state(|s| s.tracks[0].sends.is_empty()));
    assert_eq!(applied["material"]["receiver"], "{DDDD}");
    lab.undo("send-remove", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    let (volume, pan, mode, phase) = state(|s| {
        let send = &s.tracks[0].sends[0];
        (send.volume, send.pan, send.mode, send.extra.get("B_PHASE").copied())
    });
    assert_eq!((volume, pan, mode, phase), (0.3, -0.2, 1, Some(1.0)));
    // Undone twice: it is back, and nothing is made again.
    lab.undo("send-remove", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(state(|s| s.tracks[0].sends.len()), 1);
    // The receiver went: the send can't be made again.
    let (preview, applied) = lab.make("send-remove", json!({"ref": DRUMS_TO_REVERB}), "Kumi: Removed send Drums → Reverb (ksr02)");
    state(|s| {
        s.tracks.remove(3);
    });
    assert_eq!(lab.undo("send-remove", &preview, &applied, "Kumi: undo (kund3)").unwrap_err().0, "gone");
}

// ----- markers and regions -----

fn markers() -> Vec<(String, f64, f64, String, i32)> {
    state(|s| s.markers.iter().map(|m| (m.guid.clone(), m.start, m.end, m.name.clone(), m.number)).collect())
}

#[test]
fn a_marker_is_added_where_asked_in_beats_or_seconds_and_undo_takes_that_one_away() {
    let lab = Lab::new();
    let before = markers();
    let (preview, applied) =
        lab.make("marker-add", json!({"positionBeats": 8.0, "name": "Intro", "color": "#102030"}), "Kumi: Marker Intro at bar 3 (kma01)");
    assert_eq!(preview["display"]["title"], "Marker “Intro” at bar 3");
    assert_eq!(applied["created"]["kind"], "marker");
    let now = markers();
    assert_eq!(now.len(), before.len() + 1);
    let made = now.last().unwrap();
    assert_eq!((made.1, made.3.as_str()), (4.0, "Intro"), "8 quarter notes at 120 bpm");
    assert_eq!(state(|s| s.markers.last().unwrap().color), 0x10 | 0x20 << 8 | 0x30 << 16 | 0x100_0000);
    let mut params = json!({"kind": "marker-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo (kund1)"});
    params["epoch"] = lab.reads.epoch().into();
    // The producer renamed it since: undo is refused, unless discarded.
    state(|s| s.markers.last_mut().unwrap().name = "Renamed".into());
    assert_eq!(lab.call("change_restore", params.clone()).unwrap_err().0, "changed");
    state(|s| s.markers.last_mut().unwrap().name = "Intro".into());
    lab.call("change_restore", params.clone()).unwrap();
    assert_eq!(markers(), before);
    assert_eq!(lab.call("change_restore", params).unwrap_err().0, "gone");
    // In seconds.
    let (_, applied) = lab.make("marker-add", json!({"position": 12.5}), "Kumi: Marker at bar 7 beat 2 (kma02)");
    assert_eq!(state(|s| s.markers.last().unwrap().start), 12.5);
    assert!(applied["display"]["title"].as_str().unwrap().starts_with("Marker at bar"));
}

#[test]
fn a_region_needs_an_end_after_its_start_and_a_marker_has_none() {
    let lab = Lab::new();
    let (preview, applied) = lab.make(
        "region-add",
        json!({"positionBeats": 16.0, "endBeats": 32.0, "name": "Chorus 2"}),
        "Kumi: Region Chorus 2 from bar 5 to bar 9 (kra01)",
    );
    assert_eq!(preview["display"]["title"], "Region “Chorus 2” from bar 5 to bar 9");
    assert_eq!(applied["created"]["kind"], "region");
    assert_eq!(
        state(|s| (s.markers.last().unwrap().start, s.markers.last().unwrap().end, s.markers.last().unwrap().is_region)),
        (8.0, 16.0, true)
    );
    for (kind, input) in [
        ("region-add", json!({"positionBeats": 16.0})),
        ("region-add", json!({"positionBeats": 16.0, "endBeats": 16.0})),
        ("region-add", json!({"positionBeats": 16.0, "end": 5.0})),
        ("marker-add", json!({})),
        ("marker-add", json!({"positionBeats": 1.0, "position": 1.0})),
        ("marker-add", json!({"positionBeats": -1.0})),
        ("marker-add", json!({"position": 1.0, "name": "a\nb"})),
        ("marker-add", json!({"position": 1.0, "color": "pink"})),
        ("marker-move", json!({"ref": "r:marker:{MARK-1}", "position": 1.0, "end": 3.0})),
        ("marker-move", json!({"ref": "r:track:{AAAA}", "position": 1.0})),
        ("marker-rename", json!({"ref": "r:marker:{MARK-1}"})),
    ] {
        assert_eq!(lab.call("change_preview", json!({"kind": kind, "input": input})).unwrap_err().0, "bad-params", "{kind} {input}");
    }
    assert_eq!(
        lab.call("change_preview", json!({"kind": "marker-move", "input": {"ref": "r:marker:{NOPE}", "position": 1.0}})).unwrap_err().0,
        "failed"
    );
    // A marker ref is not a region's.
    assert_eq!(
        lab.call("change_preview", json!({"kind": "marker-rename", "input": {"ref": "r:region:{MARK-1}", "name": "x"}})).unwrap_err().0,
        "failed"
    );
}

#[test]
fn a_marker_is_moved_renamed_and_coloured_and_each_goes_back() {
    let lab = Lab::new();
    // Drop is at 4 s; Verse is the region from 8 s to 16 s.
    let (preview, applied) =
        lab.make("marker-move", json!({"ref": "r:marker:{MARK-1}", "positionBeats": 40.0}), "Kumi: Moved marker Drop to bar 11 (kmm01)");
    assert_eq!(preview["display"]["title"], "Moved marker “Drop” to bar 11");
    assert_eq!(state(|s| s.markers[0].start), 20.0);
    lab.undo("marker-move", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(state(|s| s.markers[0].start), 4.0);
    // A region moved by its start keeps its length; one moved with an end is resized.
    let (preview, applied) =
        lab.make("marker-move", json!({"ref": "r:region:{REG-1}", "position": 20.0}), "Kumi: Moved region Verse (kmm02)");
    assert_eq!(state(|s| (s.markers[1].start, s.markers[1].end)), (20.0, 28.0));
    lab.undo("marker-move", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(state(|s| (s.markers[1].start, s.markers[1].end)), (8.0, 16.0));
    let (preview, applied) =
        lab.make("marker-move", json!({"ref": "r:region:{REG-1}", "position": 6.0, "end": 12.0}), "Kumi: Moved region Verse (kmm03)");
    assert_eq!(state(|s| (s.markers[1].start, s.markers[1].end)), (6.0, 12.0));
    lab.undo("marker-move", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    assert_eq!(state(|s| (s.markers[1].start, s.markers[1].end)), (8.0, 16.0));
    // Name and colour.
    let (preview, applied) = lab.make(
        "marker-rename",
        json!({"ref": "r:marker:{MARK-1}", "name": "Breakdown"}),
        "Kumi: Renamed marker Drop to Breakdown (kmr01)",
    );
    assert_eq!(preview["display"]["title"], "Renamed marker “Drop” → “Breakdown”");
    assert_eq!(state(|s| s.markers[0].name.clone()), "Breakdown");
    lab.undo("marker-rename", &preview, &applied, "Kumi: undo (kund4)").unwrap();
    assert_eq!(state(|s| s.markers[0].name.clone()), "Drop");
    let green = state(|s| s.markers[1].color);
    let (preview, applied) =
        lab.make("marker-color", json!({"ref": "r:region:{REG-1}", "color": "#ff0000"}), "Kumi: Region Verse colour changed (kmc01)");
    assert_eq!(state(|s| s.markers[1].color), 0xff | 0x100_0000);
    lab.undo("marker-color", &preview, &applied, "Kumi: undo (kund5)").unwrap();
    assert_eq!(state(|s| s.markers[1].color), green);
    // Moved since: refused.
    let (preview, applied) = lab.make("marker-move", json!({"ref": "r:marker:{MARK-1}", "position": 9.0}), "Kumi: Moved (kmm04)");
    state(|s| s.markers[0].start = 11.0);
    assert_eq!(lab.undo("marker-move", &preview, &applied, "Kumi: undo (kund6)").unwrap_err().0, "changed");
    // A change fenced by where it was is refused when it moved before the apply.
    let preview = lab.call("change_preview", json!({"kind": "marker-rename", "input": {"ref": "r:marker:{MARK-1}", "name": "X"}})).unwrap();
    state(|s| s.markers[0].name = "Other".into());
    assert_eq!(lab.call("change_apply", json!({"kind": "marker-rename", "input": {"ref": "r:marker:{MARK-1}", "name": "X"}, "fence": preview["fence"], "desc": "Kumi: x (k1111)"})).unwrap_err().0, "changed");
}

#[test]
fn a_deleted_regions_undo_leaves_it_out_once_the_tempo_map_changed_unless_told_to_put_it_back_anyway() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("marker-delete", json!({"ref": "r:region:{REG-1}"}), "Kumi: Deleted region Verse (kmd07)");
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 4.0, bpm: 90.0, num: 4, denom: 4, linear: false }));
    let (code, message) = lab.undo("marker-delete", &preview, &applied, "Kumi: undo (kund7)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("tempo map"), "{message}");
    assert_eq!(state(|s| s.markers.len()), 1, "nothing made again");
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund8)");
    params["kind"] = "marker-delete".into();
    params["material"] = applied["material"].clone();
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(state(|s| s.markers.len()), 2);
}

#[test]
fn deleting_a_marker_keeps_it_to_make_again_with_a_new_id_and_its_number() {
    let lab = Lab::new();
    state(|s| s.markers[1].number = 7);
    let (preview, applied) = lab.make("marker-delete", json!({"ref": "r:region:{REG-1}"}), "Kumi: Deleted region Verse (kmd01)");
    assert_eq!(preview["display"]["title"], "Deleted region “Verse”");
    assert_eq!(state(|s| s.markers.len()), 1);
    let restored = lab.undo("marker-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["restored"]["was"], "r:region:{REG-1}");
    assert!(restored["restored"]["ref"].as_str().unwrap().starts_with("r:region:{MK-NEW-"), "{restored}");
    let (start, end, name, number, region) = state(|s| {
        let m = s.markers.last().unwrap();
        (m.start, m.end, m.name.clone(), m.number, m.is_region)
    });
    assert_eq!((start, end, name.as_str(), number, region), (8.0, 16.0, "Verse", 7, true));
    // A second undo finds a marker like it and makes nothing again... by the old id it finds none, so it makes another:
    // the journal only undoes once, so this is the producer's own doing.
    assert_eq!(state(|s| s.markers.len()), 2);
}

// ----- a plan -----

#[test]
fn a_plan_is_one_undo_block_and_its_changes_open_none_of_their_own() {
    let lab = Lab::new();
    let step = lab.call("undo_step_begin", json!({"label": "Kumi: balance the drums (kplan)"})).unwrap();
    let id = step["stepId"].as_str().unwrap().to_string();
    assert!(lab.call("undo_step_begin", json!({"label": "again"})).unwrap_err().1.contains("open already"));
    let before = state(|s| s.undo_entries);
    let (_, first) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -3.0}), "Kumi: Kick volume −3 dB (kp001)");
    let (_, second) = lab.make("track-pan", json!({"ref": KICK, "pan": 0.2}), "Kumi: Kick pan 20% right (kp002)");
    // Inside the plan nothing says yet whether REAPER made a point.
    assert_eq!((first["undo"]["entry"].clone(), second["undo"]["inPlan"].clone()), (Value::Null, json!(true)));
    assert_eq!(state(|s| s.undo_entries), before, "no point until the plan ends");
    assert_eq!(lab.call("undo_step_end", json!({"stepId": "nope"})).unwrap_err().0, "failed");
    let ended = lab.call("undo_step_end", json!({"stepId": id})).unwrap();
    assert_eq!(ended["desc"], "Kumi: balance the drums (kplan)");
    assert_eq!(ended["entry"], true);
    assert_eq!(state(|s| s.undo_entries), before + 1);
    assert_eq!(top().as_deref(), Some("Kumi: balance the drums (kplan)"));
    // Over: a second end finds nothing open.
    assert_eq!(lab.call("undo_step_end", json!({"stepId": id})).unwrap_err().0, "failed");
}

#[test]
fn invalid_labels_are_refused_before_writes_and_old_control_labels_still_close() {
    let lab = Lab::new();
    let input = json!({"ref":KICK,"volumeDb":-3.0});
    let preview = lab.call("change_preview", json!({"kind":"track-volume","input":input})).unwrap();
    for label in ["Warm\0pad", "Warm\npad", "Warm\u{7f}pad"] {
        assert_eq!(
            lab.call("change_apply", json!({"kind":"track-volume","input":input,"fence":preview["fence"],"desc":label})).unwrap_err().0,
            "bad-params"
        );
        assert_eq!(lab.call("undo_step_begin", json!({"label":label})).unwrap_err().0, "bad-params");
        assert_eq!(state(|s| s.undo_depth), 0);
    }
    assert_eq!(lab.call("change_preview", json!({"kind":"project-notes","input":{"notes":"line\0two"}})).unwrap_err().0, "bad-params");
    assert!(lab.call("change_preview", json!({"kind":"project-notes","input":{"notes":"line\ntwo"}})).is_ok());
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let proj = reaper.current_project().unwrap().ptr;
    assert!(reaper.undo_begin(proj));
    state(|s| s.undo_dirty = true);
    assert!(reaper.undo_end(proj, "Warm\0pad\n\u{7f}", -1));
    assert_eq!(state(|s| s.undo_depth), 0);
    assert_eq!(reaper.undo_can_undo(proj).as_deref(), Some("Warm pad  "));
    let id = lab.call("undo_step_begin", json!({"label":"old plan"})).unwrap();
    lab.changes.plan.borrow_mut().as_mut().unwrap().label = "old\0plan\n\u{7f}".into();
    state(|s| s.undo_dirty = true);
    let closed = lab.call("undo_step_end", json!({"stepId":id["stepId"]})).unwrap();
    assert_eq!(closed["desc"], "old plan  ");
    assert_eq!(closed["entry"], true);
    assert_eq!(reaper.undo_can_undo(proj).as_deref(), Some("old plan  "));
    assert!(lab.changes.plan.borrow().is_none());
    assert_eq!(state(|s| s.undo_depth), 0);
}

#[test]
fn a_change_that_panics_inside_its_undo_block_leaves_no_block_open() {
    // The extension catches a panic per request; a block left open would swallow every later edit (review R15).
    let lab = Lab::new();
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let ctx = lab.changes.context(&reaper, &lab.reads, &json!({"epoch": lab.reads.epoch()})).unwrap();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = lab.changes.in_block::<super::Applied>(&ctx, "Kumi: boom\0 (kpn01)", true, false, |_| panic!("a bug"));
    }));
    assert!(panicked.is_err());
    assert_eq!(state(|s| s.undo_depth), 0, "the block is ended");
}

#[test]
fn a_step_in_a_plan_says_whether_the_plans_point_takes_it_back() {
    let lab = Lab::new();
    let step = lab.call("undo_step_begin", json!({"label": "Kumi: a plan (kcov1)"})).unwrap();
    let covered = |answer: &Value| answer["undo"]["covered"].clone();
    // A change that is written is in the plan's point.
    let (_, volume) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -3.0}), "Kumi: Kick volume −3 dB (kcv01)");
    assert_eq!(covered(&volume), json!(true));
    // The repeat button leaves REAPER no history to take it back from.
    let (_, repeat) = lab.make("loop-repeat", json!({"on": true}), "Kumi: Repeat on (kcv02)");
    assert_eq!(covered(&repeat), json!(false));
    assert!(state(|s| s.repeat));
    // And a change that found everything as asked wrote nothing.
    let (_, again) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -3.0}), "Kumi: Kick volume −3 dB (kcv03)");
    assert_eq!(covered(&again), json!(false));
    // Nor the project's notes, title, grid and timebase, which REAPER keeps no history of (review R14).
    for (n, (kind, input)) in [
        ("project-notes", json!({"notes": "verse first"})),
        ("project-info", json!({"title": "Night Drive"})),
        ("project-grid", json!({"division": "1/8"})),
        ("project-timebase", json!({"items": "time"})),
    ]
    .into_iter()
    .enumerate()
    {
        let (_, made) = lab.make(kind, input, &format!("Kumi: project (kcp{n:02})"));
        assert_eq!(covered(&made), json!(false), "{kind}");
    }
    // Outside a plan the answer says what REAPER's history shows, as before.
    lab.call("undo_step_end", json!({"stepId": step["stepId"]})).unwrap();
    let (_, alone) = lab.make("track-pan", json!({"ref": KICK, "pan": 0.2}), "Kumi: Kick pan 20% right (kcv04)");
    assert_eq!((alone["undo"]["entry"].clone(), alone["undo"].get("covered").cloned()), (json!(true), None));
}

#[test]
fn a_plan_left_open_is_closed_when_the_project_changes_so_no_block_stays_open() {
    let lab = Lab::new();
    lab.call("undo_step_begin", json!({"label": "Kumi: plan (kplan)"})).unwrap();
    lab.make("track-mute", json!({"ref": KICK, "mute": true}), "Kumi: Kick muted (kp001)");
    assert_eq!(state(|s| s.undo_depth), 1);
    lab.reads.bump_epoch();
    lab.tick();
    assert_eq!(state(|s| s.undo_depth), 0);
    // Or when it was never ended.
    lab.call("undo_step_begin", json!({"label": "Kumi: plan (kplan)"})).unwrap();
    lab.changes.stop(&Reaper::new(&fake::api()), &lab.reads);
    assert_eq!(state(|s| s.undo_depth), 0);
}

// ----- the producer's undo -----

#[test]
fn the_producer_undoing_and_redoing_kumis_change_is_an_event_naming_its_description() {
    let lab = Lab::new();
    let desc = "Kumi: Kick volume −6 dB (k7f3a)";
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), desc);
    lab.tick();
    assert!(lab.events.take().is_empty(), "Kumi's own point is not the producer's undo");
    fake::producer_undo();
    lab.tick();
    assert_eq!(lab.events.take(), [("undone_in_daw".to_string(), json!({"descs": [desc], "how": "undo", "epoch": lab.reads.epoch()}))]);
    fake::producer_redo();
    lab.tick();
    assert_eq!(lab.events.take(), [("undone_in_daw".to_string(), json!({"descs": [desc], "how": "redo", "epoch": lab.reads.epoch()}))]);
    // Nothing moved: nothing said.
    lab.tick();
    assert!(lab.events.take().is_empty());
}

#[test]
fn several_points_undone_at_once_are_all_named_and_the_producers_own_are_not() {
    let lab = Lab::new();
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), "Kumi: one (kaaaa)");
    fake::producer_edit("Adjust track pan");
    lab.make("track-mute", json!({"ref": KICK, "mute": true}), "Kumi: two (kbbbb)");
    lab.tick();
    lab.events.take();
    fake::producer_undo();
    fake::producer_undo();
    fake::producer_undo();
    lab.tick();
    let events = lab.events.take();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1["descs"], json!(["Kumi: one (kaaaa)", "Kumi: two (kbbbb)"]));
    assert_eq!(events[0].1["how"], "undo");
    // Redone, two of the three: only the producer's own edit and Kumi's first.
    fake::producer_redo();
    fake::producer_redo();
    lab.tick();
    let events = lab.events.take();
    assert_eq!((events.len(), events[0].1["descs"].clone(), events[0].1["how"].clone()), (1, json!(["Kumi: one (kaaaa)"]), json!("redo")));
}

#[test]
fn a_new_edit_that_drops_what_could_have_been_redone_is_no_redo() {
    let lab = Lab::new();
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), "Kumi: one (kaaaa)");
    fake::producer_undo();
    lab.tick();
    lab.events.take();
    // The producer edits instead of redoing: the change can't come back, and was not redone.
    fake::producer_edit("Adjust track pan");
    lab.tick();
    assert!(lab.events.take().is_empty());
    // Undoing something that isn't Kumi's says nothing.
    fake::producer_undo();
    lab.tick();
    assert!(lab.events.take().is_empty());
}

#[test]
fn a_producer_undo_noticed_after_a_hook_nudge_even_when_the_count_didnt_move() {
    let lab = Lab::new();
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), "Kumi: one (kaaaa)");
    lab.tick();
    lab.events.take();
    state(|s| s.undo_cur = Some(s.undo_entries - 2));
    lab.changes.nudge();
    lab.tick();
    assert_eq!(lab.events.take().len(), 1);
}

#[test]
fn a_new_project_in_the_tab_starts_the_history_afresh() {
    let lab = Lab::new();
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), "Kumi: one (kaaaa)");
    lab.tick();
    lab.events.take();
    lab.reads.bump_epoch();
    state(|s| {
        s.undo_entries = 1;
        s.undo_descs.clear();
        s.undo_cur = None;
    });
    lab.tick();
    assert!(lab.events.take().is_empty(), "another project's history is not the producer undoing Kumi's change");
}

// ----- transport -----

#[test]
fn transport_stop_stops_what_plays_and_says_whether_it_did() {
    let lab = Lab::new();
    assert_eq!(lab.call("transport_stop", json!({})).unwrap(), json!({"stopped": false, "wasRecording": false}));
    assert!(state(|s| s.actions.is_empty()));
    state(|s| s.play_bits = 1 | 4);
    assert_eq!(lab.call("transport_stop", json!({})).unwrap(), json!({"stopped": true, "wasRecording": true}));
    assert_eq!(state(|s| s.actions.clone()), [1016]);
}

#[test]
fn a_send_that_is_not_in_the_project_any_more_is_skipped_when_a_deleted_track_is_made_again() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{DDDD}"}), "Kumi: Deleted track Reverb (kdel1)");
    // The track that sent to it went too.
    state(|s| {
        s.tracks.remove(0);
    });
    let restored = lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["restored"]["routes"], 0);
}

// ----- the timeline -----

fn restore_params(preview: &Value, applied: &Value, desc: &str) -> Value {
    json!({"kind": "", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": desc})
}

fn tempo_markers() -> Vec<(f64, f64, i32, i32, bool)> {
    state(|s| s.tempo_markers.iter().map(|m| (m.time, m.bpm, m.num, m.denom, m.linear)).collect())
}

#[test]
fn the_tempo_is_set_in_one_named_block_and_put_back_by_value() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("tempo-set", json!({"bpm": 90.0}), "Kumi: Tempo 90 BPM (ktm01)");
    assert_eq!((&preview["display"]["title"], &preview["prior"]), (&json!("Tempo 90 BPM"), &json!({"bpm": 120.0})));
    assert_eq!((state(|s| s.bpm), top().as_deref()), (90.0, Some("Kumi: Tempo 90 BPM (ktm01)")));
    assert_eq!(applied["applied"], json!({"bpm": 90.0}));
    // The producer moved it since: undo is refused, unless discarded.
    state(|s| s.bpm = 100.0);
    let (code, message) = lab.undo("tempo-set", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("tempo"), "{message}");
    assert_eq!(state(|s| s.bpm), 100.0);
    state(|s| s.bpm = 90.0);
    lab.undo("tempo-set", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!((state(|s| s.bpm), top().as_deref()), (120.0, Some("Kumi: undo (kund1)")));
    // Undone already: nothing to write, and no complaint.
    lab.undo("tempo-set", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund3)");
    params["kind"] = "tempo-set".into();
    params["discard"] = true.into();
    state(|s| s.bpm = 77.0);
    lab.call("change_restore", params).unwrap();
    assert_eq!(state(|s| s.bpm), 120.0);
}

#[test]
fn a_tempo_change_is_refused_when_the_tempo_moved_or_the_project_has_a_tempo_map() {
    let lab = Lab::new();
    let preview = lab.call("change_preview", json!({"kind": "tempo-set", "input": {"bpm": 90.0}})).unwrap();
    state(|s| s.bpm = 110.0);
    let entries = state(|s| s.undo_entries);
    let apply = json!({"kind": "tempo-set", "input": {"bpm": 90.0}, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
    assert_eq!((state(|s| s.bpm), state(|s| s.undo_entries)), (110.0, entries));
    for bpm in [json!(0.5), json!(2000.0), json!("fast"), Value::Null] {
        assert_eq!(lab.call("change_preview", json!({"kind": "tempo-set", "input": {"bpm": bpm}})).unwrap_err().0, "bad-params");
    }
    // Setting what it already is says so.
    let same = lab.call("change_preview", json!({"kind": "tempo-set", "input": {"bpm": 110.0}})).unwrap();
    assert_eq!(same["display"]["unchanged"], true);
    // With a tempo map the project has no one tempo.
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 0.0, bpm: 110.0, num: -1, denom: -1, linear: false }));
    let (code, message) = lab.call("change_preview", json!({"kind": "tempo-set", "input": {"bpm": 90.0}})).unwrap_err();
    assert_eq!(code, "bad-params");
    assert!(message.contains("tempo-marker-set"), "{message}");
}

#[test]
fn a_tempo_marker_is_made_where_asked_with_its_time_signature_and_undo_takes_that_one_away() {
    let lab = Lab::new();
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 0.0, bpm: 120.0, num: 4, denom: 4, linear: false }));
    let before = tempo_markers();
    let input = json!({"positionBeats": 32.0, "bpm": 140.0, "timeSignature": [3, 4]});
    let (preview, applied) = lab.make("tempo-marker-set", input, "Kumi: Tempo 140 BPM and time signature 3/4 at bar 9 (ktm02)");
    assert_eq!(preview["prior"], json!({"exists": false}));
    assert_eq!(preview["target"]["kind"], "tempo-marker");
    assert_eq!(tempo_markers(), [before[0], (16.0, 140.0, 3, 4, false)]);
    assert_eq!(applied["applied"], json!({"exists": true, "bpm": 140.0, "num": 3, "den": 4, "linear": false, "qn": 32.0}));
    // The producer changed it since: undo is refused, unless discarded.
    state(|s| s.tempo_markers[1].bpm = 150.0);
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund1)");
    params["kind"] = "tempo-marker-set".into();
    assert_eq!(lab.call("change_restore", params.clone()).unwrap_err().0, "changed");
    assert_eq!(tempo_markers().len(), 2);
    state(|s| s.tempo_markers[1].bpm = 140.0);
    lab.call("change_restore", params.clone()).unwrap();
    assert_eq!(tempo_markers(), before);
    // Undone already.
    lab.call("change_restore", params).unwrap();
    assert_eq!(tempo_markers(), before);
}

#[test]
fn a_tempo_marker_kumi_made_is_found_by_its_beat_after_a_tempo_change_and_left_once_the_producer_dragged_it() {
    let lab = Lab::new();
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 0.0, bpm: 120.0, num: 4, denom: 4, linear: false }));
    let before = tempo_markers();
    let (preview, applied) = lab.make("tempo-marker-set", json!({"positionBeats": 32.0, "bpm": 140.0}), "Kumi: Tempo 140 BPM (ktm07)");
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund7)");
    params["kind"] = "tempo-marker-set".into();
    // The producer drags it from bar 9 to bar 11: nothing is where Kumi put it, and Kumi's marker is elsewhere.
    state(|s| s.tempo_markers[1].time = 20.0);
    for discard in [false, true] {
        params["discard"] = discard.into();
        let (code, message) = lab.call("change_restore", params.clone()).unwrap_err();
        assert_eq!(code, "changed", "discard {discard}: {message}");
        assert!(message.contains("moved or deleted"), "{message}");
        assert_eq!(tempo_markers()[1].0, 20.0, "the producer's tempo map stays");
    }
    params["discard"] = false.into();
    // In beats timebase, a slower tempo before it moves it in seconds: it is found at its beat, and taken away.
    state(|s| {
        s.bpm = 60.0;
        s.tempo_markers[1].time = 32.0;
    });
    lab.call("change_restore", params).unwrap();
    assert_eq!(tempo_markers(), before);
}

#[test]
fn a_marker_that_is_there_is_changed_by_the_part_asked_and_put_back_whole() {
    let lab = Lab::new();
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 8.0, bpm: 100.0, num: 3, denom: 4, linear: true }));
    // Only the tempo: the time signature and the shape stay.
    let (preview, applied) = lab.make("tempo-marker-set", json!({"position": 8.0, "bpm": 128.0}), "Kumi: Tempo 128 BPM at bar 5 (ktm03)");
    assert_eq!(preview["prior"], json!({"exists": true, "bpm": 100.0, "num": 3, "den": 4, "linear": true}));
    assert_eq!(tempo_markers(), [(8.0, 128.0, 3, 4, true)]);
    // Only the time signature, and the shape back to a jump.
    let (preview2, applied2) = lab.make(
        "tempo-marker-set",
        json!({"position": 8.0, "timeSignature": {"num": 7, "den": 8}, "linear": false}),
        "Kumi: Time signature 7/8 at bar 5 (ktm04)",
    );
    assert_eq!(tempo_markers(), [(8.0, 128.0, 7, 8, false)]);
    lab.undo("tempo-marker-set", &preview2, &applied2, "Kumi: undo (kund2)").unwrap();
    assert_eq!(tempo_markers(), [(8.0, 128.0, 3, 4, true)]);
    lab.undo("tempo-marker-set", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(tempo_markers(), [(8.0, 100.0, 3, 4, true)]);
}

#[test]
fn a_tempo_marker_request_names_a_place_and_something_to_change() {
    let lab = Lab::new();
    for input in [
        json!({"bpm": 120.0}),
        json!({"position": 4.0}),
        json!({"position": 4.0, "positionBeats": 8.0, "bpm": 120.0}),
        json!({"position": 4.0, "bpm": 0.0}),
        json!({"position": 4.0, "timeSignature": [4, 3]}),
        json!({"position": 4.0, "timeSignature": [0, 4]}),
        json!({"position": 4.0, "timeSignature": "3/4"}),
        json!({"position": 4.0, "bpm": 100.0, "linear": "yes"}),
        json!({"position": -1.0, "bpm": 100.0}),
    ] {
        let (code, message) = lab.call("change_preview", json!({"kind": "tempo-marker-set", "input": input})).unwrap_err();
        assert_eq!(code, "bad-params", "{input}: {message}");
    }
    assert!(tempo_markers().is_empty());
}

#[test]
fn a_tempo_marker_change_is_refused_when_the_markers_there_moved() {
    let lab = Lab::new();
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 8.0, bpm: 100.0, num: -1, denom: -1, linear: false }));
    let input = json!({"position": 8.0, "bpm": 128.0});
    let preview = lab.call("change_preview", json!({"kind": "tempo-marker-set", "input": input})).unwrap();
    state(|s| s.tempo_markers[0].bpm = 90.0);
    let entries = state(|s| s.undo_entries);
    let apply = json!({"kind": "tempo-marker-set", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply.clone()).unwrap_err().0, "changed");
    // The marker went altogether.
    state(|s| s.tempo_markers.clear());
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
    assert_eq!((tempo_markers().len(), state(|s| s.undo_entries)), (0, entries));
}

#[test]
fn deleting_a_tempo_marker_keeps_it_to_make_again_exactly() {
    let lab = Lab::new();
    state(|s| {
        s.tempo_markers.push(fake::FakeTempo { time: 0.0, bpm: 120.0, num: 4, denom: 4, linear: false });
        s.tempo_markers.push(fake::FakeTempo { time: 8.0, bpm: 90.0, num: 3, denom: 4, linear: true });
    });
    let before = tempo_markers();
    let (preview, applied) = lab.make("tempo-marker-delete", json!({"position": 8.0}), "Kumi: Deleted tempo marker at bar 5 (ktm05)");
    assert_eq!(preview["display"]["title"], "Deleted tempo marker at bar 5");
    assert_eq!(tempo_markers(), [before[0]]);
    assert_eq!(applied["material"]["state"], json!({"exists": true, "bpm": 90.0, "num": 3, "den": 4, "linear": true}));
    let restored = lab.undo("tempo-marker-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["restored"]["bpm"], 90.0);
    assert_eq!(tempo_markers(), before);
    // Made again already: nothing more.
    lab.undo("tempo-marker-delete", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(tempo_markers(), before);
    // A different marker took its place: refused, unless discarded.
    lab.make("tempo-marker-delete", json!({"position": 8.0}), "Kumi: Deleted tempo marker at bar 5 (ktm06)");
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 8.0, bpm: 70.0, num: -1, denom: -1, linear: false }));
    let (code, _) = lab.undo("tempo-marker-delete", &preview, &applied, "Kumi: undo (kund3)").unwrap_err();
    assert_eq!(code, "changed");
    assert_eq!(lab.call("change_preview", json!({"kind": "tempo-marker-delete", "input": {"position": 40.0}})).unwrap_err().0, "failed");
}

#[test]
fn the_loop_is_set_cleared_and_put_back_and_end_must_follow_start() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("loop-range", json!({"startBeats": 8.0, "endBeats": 16.0}), "Kumi: Loop bars 3–4 (klp01)");
    // Whole bars as a producer counts them (it ends where bar 5 starts); one bar, or off the bar lines, as positions.
    assert_eq!(preview["display"]["title"], "Loop bars 3–4");
    let title =
        |input: Value| lab.call("change_preview", json!({"kind": "loop-range", "input": input})).unwrap()["display"]["title"].clone();
    assert_eq!(title(json!({"startBeats": 8.0, "endBeats": 12.0})), "Loop bar 3");
    assert_eq!(title(json!({"startBeats": 8.0, "endBeats": 14.0})), "Loop bar 3 to bar 4 beat 3");
    assert_eq!(preview["prior"], json!({"start": 0.0, "end": 0.0}));
    assert_eq!(state(|s| s.loop_range), (4.0, 8.0));
    // Moved by the producer: undo is refused, unless discarded.
    state(|s| s.loop_range = (4.0, 12.0));
    assert_eq!(lab.undo("loop-range", &preview, &applied, "Kumi: undo (kund1)").unwrap_err().0, "changed");
    state(|s| s.loop_range = (4.0, 8.0));
    lab.undo("loop-range", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(state(|s| s.loop_range), (0.0, 0.0));
    // Cleared, in seconds.
    state(|s| s.loop_range = (2.0, 6.0));
    let (preview, applied) = lab.make("loop-range", json!({"start": 0.0, "end": 0.0}), "Kumi: Loop cleared (klp02)");
    assert_eq!(preview["display"]["title"], "Loop cleared");
    assert_eq!(state(|s| s.loop_range), (0.0, 0.0));
    lab.undo("loop-range", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(state(|s| s.loop_range), (2.0, 6.0));
    for input in [
        json!({"start": 4.0, "end": 4.0}),
        json!({"start": 6.0, "end": 2.0}),
        json!({"start": 2.0}),
        json!({"startBeats": 4.0, "start": 2.0, "end": 6.0}),
        json!({"start": "a", "end": 6.0}),
    ] {
        assert_eq!(lab.call("change_preview", json!({"kind": "loop-range", "input": input})).unwrap_err().0, "bad-params", "{input}");
    }
}

#[test]
fn a_loop_change_is_refused_when_the_loop_moved_since_the_preview() {
    let lab = Lab::new();
    let input = json!({"start": 2.0, "end": 6.0});
    let preview = lab.call("change_preview", json!({"kind": "loop-range", "input": input})).unwrap();
    state(|s| s.loop_range = (1.0, 3.0));
    let entries = state(|s| s.undo_entries);
    let apply = json!({"kind": "loop-range", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
    assert_eq!((state(|s| s.loop_range), state(|s| s.undo_entries)), ((1.0, 3.0), entries));
}

#[test]
fn repeat_is_switched_and_put_back_unless_the_producer_switched_it_since() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("loop-repeat", json!({"on": true}), "Kumi: Repeat on (krp01)");
    assert_eq!((preview["prior"].clone(), state(|s| s.repeat)), (json!({"on": false}), true));
    state(|s| s.repeat = false);
    assert_eq!(
        lab.undo("loop-repeat", &preview, &applied, "Kumi: undo (kund1)").unwrap().get("restored").map(|r| r["on"].clone()),
        Some(json!(false))
    );
    state(|s| s.repeat = true);
    lab.undo("loop-repeat", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert!(!state(|s| s.repeat));
    for input in [json!({}), json!({"on": "yes"})] {
        assert_eq!(lab.call("change_preview", json!({"kind": "loop-repeat", "input": input})).unwrap_err().0, "bad-params");
    }
    // The producer switched it since the preview.
    let preview = lab.call("change_preview", json!({"kind": "loop-repeat", "input": {"on": true}})).unwrap();
    state(|s| s.repeat = true);
    let apply = json!({"kind": "loop-repeat", "input": {"on": true}, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
    // And after the apply: a changed restore is refused.
    state(|s| s.repeat = false);
    let (preview, applied) = lab.make("loop-repeat", json!({"on": true}), "Kumi: Repeat on (krp02)");
    state(|s| s.repeat = false);
    // Already at its prior: nothing to write, and no complaint.
    assert!(lab.undo("loop-repeat", &preview, &applied, "Kumi: undo (kund3)").unwrap().get("restored").is_some());
}

#[test]
fn the_timeline_kinds_are_refused_for_another_epoch_and_while_reaper_is_busy() {
    let lab = Lab::new();
    for (kind, input) in [
        ("tempo-set", json!({"bpm": 90.0})),
        ("tempo-marker-set", json!({"position": 4.0, "bpm": 90.0})),
        ("loop-range", json!({"start": 2.0, "end": 6.0})),
        ("loop-repeat", json!({"on": true})),
    ] {
        let (code, _) = lab.call("change_preview", json!({"kind": kind, "input": input, "epoch": "old-1"})).unwrap_err();
        assert_eq!(code, "stale", "{kind}");
        let preview = lab.call("change_preview", json!({"kind": kind, "input": input})).unwrap();
        state(|s| s.loading = true);
        let apply = json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
        assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "busy", "{kind}");
        state(|s| s.loading = false);
    }
    assert_eq!((state(|s| s.bpm), state(|s| s.loop_range), state(|s| s.repeat), tempo_markers().len()), (120.0, (0.0, 0.0), false, 0));
}

// ----- notes -----

const SNARE_TAKE: &str = "r:take:{TAKE-S1}";

/// (pitch, start, end, velocity, channel, selected) of the snare take's notes, in order.
fn snare_notes() -> Vec<(i32, f64, f64, i32, i32, bool)> {
    state(|s| {
        s.tracks[2].items[0].takes[0].notes.iter().map(|n| (n.pitch, n.start_ppq, n.end_ppq, n.velocity, n.channel, n.selected)).collect()
    })
}

#[test]
fn notes_and_events_go_into_a_midi_take_with_nothing_in_it_yet() {
    // REAPER's MIDI_CountEvts answers the take's total: 0 for a new, empty MIDI item (accept-reaper lab: Kumi refused it,
    // "REAPER wouldn't give the take's MIDI").
    let lab = Lab::new();
    state(|s| {
        s.tracks[2].items[0].takes[0].notes.clear();
        s.tracks[2].items[0].takes[0].ccs.clear();
        s.state_count += 1;
    });
    let notes = lab.call("read", json!({"kind": "note", "parent": SNARE_TAKE})).unwrap();
    assert_eq!(notes["items"], json!([]));
    let take = lab.call("read", json!({"ref": SNARE_TAKE})).unwrap();
    assert_eq!((take["items"][0]["noteCount"].clone(), take["items"][0]["ccCount"].clone()), (json!(0), json!(0)));
    let input = json!({"ref": SNARE_TAKE, "notes": [{"pitch": 60, "start": 1.0, "length": 0.5}]});
    let (preview, applied) = lab.make("notes-add", input, "Kumi: Wrote 1 note in snare.mid take (kne01)");
    assert_eq!(snare_notes().len(), 1);
    lab.undo("notes-add", &preview, &applied, "Kumi: undo (kne02)").unwrap();
    assert!(snare_notes().is_empty());
    let input = json!({"ref": SNARE_TAKE, "events": [{"type": "cc", "start": 0.5, "controller": 1, "value": 64}]});
    lab.make("midi-events-add", input, "Kumi: Wrote 1 MIDI event (kne03)");
    assert_eq!(state(|s| s.tracks[2].items[0].takes[0].ccs.len()), 1);
}

#[test]
fn notes_are_written_where_asked_in_beats_and_undo_takes_exactly_those_away() {
    let lab = Lab::new();
    let before = snare_notes();
    let input = json!({"ref": SNARE_TAKE, "notes": [
        {"pitch": 60, "start": 1.0, "length": 0.5, "velocity": 90, "channel": 2},
        {"pitch": 64, "start": 2.0, "length": 1.0},
        {"pitch": 67, "start": 0.5, "length": 0.25, "muted": true},
    ]});
    let (preview, applied) = lab.make("notes-add", input, "Kumi: Wrote 3 notes in snare.mid take (kna01)");
    assert_eq!(preview["display"]["title"], "Wrote 3 notes in take “snare.mid”");
    assert_eq!(preview["target"], json!({"ref": SNARE_TAKE, "kind": "take"}));
    assert_eq!(top().as_deref(), Some("Kumi: Wrote 3 notes in snare.mid take (kna01)"));
    let now = snare_notes();
    assert_eq!(now.len(), 5);
    assert!(now.windows(2).all(|pair| pair[0].1 <= pair[1].1), "in order of start: {now:?}");
    assert!(now.contains(&(60, 960.0, 1440.0, 90, 1, false)) && now.contains(&(64, 1920.0, 2880.0, 100, 0, false)));
    assert_eq!(applied["applied"]["added"].as_array().unwrap().len(), 3);
    assert_eq!(applied["applied"]["removed"], json!([]));
    // The notes that were there are untouched, the selection too.
    assert!(before.iter().all(|n| now.contains(n)));
    // The producer changed one since: undo is refused and nothing is touched, unless discarded.
    state(|s| s.tracks[2].items[0].takes[0].notes.iter_mut().find(|n| n.pitch == 64).unwrap().velocity = 20);
    let (code, message) = lab.undo("notes-add", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("changed in REAPER"), "{message}");
    assert_eq!(snare_notes().len(), 5);
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund1)");
    params["kind"] = "notes-add".into();
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    // The note the producer changed is theirs now and stays; the two Kumi wrote and nobody touched are gone.
    let mut expected = [before, vec![(64, 1920.0, 2880.0, 20, 0, false)]].concat();
    expected.sort_by(|a, b| a.1.total_cmp(&b.1));
    assert_eq!(snare_notes(), expected);
}

#[test]
fn an_undo_of_written_notes_the_producer_changed_every_one_of_is_refused_not_told_back_already() {
    // None of Kumi's notes found by value read as back already: the undo answered "put back" with nothing written, HISTORY
    // said undone, and the notes stayed.
    let lab = Lab::new();
    let before = snare_notes();
    let input =
        json!({"ref": SNARE_TAKE, "notes": [{"pitch": 60, "start": 1.0, "length": 0.5}, {"pitch": 64, "start": 2.0, "length": 0.5}]});
    let (preview, applied) = lab.make("notes-add", input, "Kumi: Wrote 2 notes in snare.mid take (knb01)");
    let ours = |n: &fake::FakeNote| n.pitch == 60 || n.pitch == 64;
    state(|s| s.tracks[2].items[0].takes[0].notes.iter_mut().filter(|n| ours(n)).for_each(|n| n.velocity = 33));
    let (code, _) = lab.undo("notes-add", &preview, &applied, "Kumi: undo (knb02)").unwrap_err();
    assert_eq!(code, "changed");
    assert_eq!(snare_notes().len(), before.len() + 2, "the producer's notes stay");
    // REAPER's own undo took them back: Kumi's undo is back already, and writes nothing.
    state(|s| s.tracks[2].items[0].takes[0].notes.retain(|n| !ours(n)));
    let undone = lab.undo("notes-add", &preview, &applied, "Kumi: undo (knb03)").unwrap();
    assert!(undone["display"]["note"].as_str().is_some_and(|note| note.contains("back already")), "{undone}");
    assert_eq!(snare_notes(), before);
}

#[test]
fn notes_are_changed_by_index_and_put_back_by_their_values() {
    let lab = Lab::new();
    let before = snare_notes();
    let input = json!({"ref": SNARE_TAKE, "changes": [
        {"index": 0, "pitch": 36, "start": 0.5},
        {"index": 1, "velocity": 127, "length": 2.0, "muted": true},
    ]});
    let (preview, applied) = lab.make("notes-change", input, "Kumi: Changed 2 notes in snare.mid take (knc01)");
    assert_eq!(preview["display"]["before"].as_array().unwrap().len(), 2);
    assert_eq!(preview["display"]["after"][0]["pitch"], 36);
    let now = snare_notes();
    // The first: moved half a beat later, the same length. The second: longer and louder.
    assert_eq!(now, [(36, 480.0, 960.0, 100, 0, false), (40, 960.0, 2880.0, 127, 9, true)]);
    assert!(state(|s| s.tracks[2].items[0].takes[0].notes[1].muted));
    lab.undo("notes-change", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(snare_notes().len(), 2);
    let after: Vec<_> = snare_notes().into_iter().map(|n| (n.0, n.1, n.2, n.3, n.4)).collect();
    assert_eq!(after, before.iter().map(|n| (n.0, n.1, n.2, n.3, n.4)).collect::<Vec<_>>());
    assert!(!state(|s| s.tracks[2].items[0].takes[0].notes[1].muted));
    // Undone already: nothing more happens.
    lab.undo("notes-change", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(snare_notes().len(), 2);
    // Changing a note to what it is says so.
    let same = lab
        .call("change_preview", json!({"kind": "notes-change", "input": {"ref": SNARE_TAKE, "changes": [{"index": 0, "pitch": 38}]}}))
        .unwrap();
    assert_eq!(same["display"]["unchanged"], true);
}

#[test]
fn deleted_notes_are_written_again_with_their_selection_and_a_retry_changes_nothing() {
    let lab = Lab::new();
    let before = snare_notes();
    let (preview, applied) =
        lab.make("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [1]}), "Kumi: Deleted 1 note in snare.mid take (knd01)");
    assert_eq!(preview["display"]["title"], "Deleted 1 note in take “snare.mid”");
    assert_eq!(snare_notes().len(), 1);
    lab.undo("notes-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(snare_notes(), before, "the second note is selected, and is again");
    lab.undo("notes-delete", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(snare_notes(), before);
    // The producer wrote the same note by hand since: nothing is written twice.
    let (preview, applied) =
        lab.make("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [0, 1]}), "Kumi: Deleted 2 notes in snare.mid take (knd02)");
    assert!(snare_notes().is_empty());
    state(|s| {
        s.tracks[2].items[0].takes[0].notes.push(fake::FakeNote {
            start_ppq: 0.0,
            end_ppq: 480.0,
            channel: 0,
            pitch: 38,
            velocity: 100,
            selected: false,
            muted: false,
        })
    });
    lab.undo("notes-delete", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    assert_eq!(snare_notes().len(), 2);
}

#[test]
fn a_notes_change_is_refused_when_the_take_changed_since_the_preview_and_nothing_is_written() {
    let lab = Lab::new();
    for (kind, input) in [
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [{"pitch": 60, "start": 1.0, "length": 1.0}]})),
        ("notes-change", json!({"ref": SNARE_TAKE, "changes": [{"index": 0, "pitch": 50}]})),
        ("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [0]})),
    ] {
        let preview = lab.call("change_preview", json!({"kind": kind, "input": input})).unwrap();
        // The producer moved a note, without changing how many there are.
        state(|s| s.tracks[2].items[0].takes[0].notes[1].pitch += 1);
        let entries = state(|s| s.undo_entries);
        let before = snare_notes();
        let apply = json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
        assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed", "{kind}");
        assert_eq!((snare_notes(), state(|s| s.undo_entries)), (before, entries));
        state(|s| s.tracks[2].items[0].takes[0].notes[1].pitch -= 1);
    }
}

#[test]
fn a_bad_notes_request_is_refused_before_anything_is_written() {
    let lab = Lab::new();
    let note = |extra: Value| {
        let mut note = json!({"pitch": 60, "start": 1.0, "length": 1.0});
        for (key, value) in extra.as_object().unwrap() {
            note[key] = value.clone();
        }
        note
    };
    let cases = [
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": []})),
        ("notes-add", json!({"ref": SNARE_TAKE})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [note(json!({"pitch": 128}))]})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [note(json!({"velocity": 0}))]})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [note(json!({"channel": 17}))]})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [note(json!({"start": -1.0}))]})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [note(json!({"length": 0.0}))]})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [note(json!({"muted": "yes"}))]})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [{"start": 1.0, "length": 1.0}]})),
        ("notes-add", json!({"ref": "r:track:{CCCC}", "notes": [note(json!({}))]})),
        ("notes-add", json!({"notes": [note(json!({}))]})),
        ("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [5]})),
        ("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [0, 0]})),
        ("notes-delete", json!({"ref": SNARE_TAKE, "indexes": ["a"]})),
        ("notes-change", json!({"ref": SNARE_TAKE, "changes": [{"pitch": 50}]})),
        ("notes-change", json!({"ref": SNARE_TAKE, "changes": [{"index": 9, "pitch": 50}]})),
        ("notes-change", json!({"ref": SNARE_TAKE, "changes": [{"index": 0, "length": -1.0}]})),
    ];
    for (kind, input) in cases {
        let (code, message) = lab.call("change_preview", json!({"kind": kind, "input": input})).unwrap_err();
        assert_eq!(code, "bad-params", "{kind} {input}: {message}");
    }
    // A take that isn't MIDI.
    let audio = state(|s| s.tracks[1].items[0].takes[0].guid.clone());
    let (code, _) = lab
        .call("change_preview", json!({"kind": "notes-add", "input": {"ref": format!("r:take:{audio}"), "notes": [note(json!({}))]}}))
        .unwrap_err();
    assert_eq!(code, "bad-params");
    assert_eq!(snare_notes().len(), 2);
}

#[test]
fn a_thousand_notes_are_one_change_and_one_undo() {
    let lab = Lab::new();
    let notes: Vec<Value> =
        (0..1000).map(|i| json!({"pitch": 36 + i % 24, "start": i as f64 * 0.25, "length": 0.25, "velocity": 1 + i % 127})).collect();
    let (preview, applied) =
        lab.make("notes-add", json!({"ref": SNARE_TAKE, "notes": notes}), "Kumi: Wrote 1000 notes in snare.mid take (knb01)");
    assert_eq!(preview["display"]["count"], 1000);
    assert_eq!(snare_notes().len(), 1002);
    lab.undo("notes-add", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(snare_notes().len(), 2);
    let too_many: Vec<Value> = (0..10_001).map(|i| json!({"pitch": 60, "start": i as f64, "length": 1.0})).collect();
    assert_eq!(
        lab.call("change_preview", json!({"kind": "notes-add", "input": {"ref": SNARE_TAKE, "notes": too_many}})).unwrap_err().0,
        "bad-params"
    );
}

// ----- FX -----

fn kick_fx() -> Vec<(String, String, bool)> {
    state(|s| s.tracks[1].fx.iter().map(|f| (f.guid.clone(), f.name.clone(), f.enabled)).collect())
}

fn fx_ref_of(guid: &str) -> String {
    format!("r:fx:{guid}")
}

/// A resource folder the fake can keep FX chain files in.
fn fx_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    state(|s| s.resource = dir.path().to_string_lossy().into_owned());
    dir
}

fn chain_files(dir: &tempfile::TempDir) -> usize {
    std::fs::read_dir(dir.path().join("FXChains")).map_or(0, |d| d.count())
}

#[test]
fn a_plug_in_is_added_where_asked_and_undo_deletes_that_one_by_its_id() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added ReaEQ to Kick (kfa01)");
    assert_eq!(preview["display"]["title"], "Added “ReaEQ” to “Kick”");
    assert_eq!(top().as_deref(), Some("Kumi: Added ReaEQ to Kick (kfa01)"));
    assert_eq!(kick_fx().len(), 1);
    let eq = kick_fx()[0].0.clone();
    assert_eq!(applied["created"], json!({"ref": fx_ref_of(&eq), "kind": "fx"}));
    // At a place, in front.
    let (preview2, applied2) =
        lab.make("fx-add", json!({"ref": KICK, "name": "ReaComp", "index": 0}), "Kumi: Added ReaComp to Kick (kfa02)");
    assert_eq!(kick_fx().iter().map(|f| f.1.as_str()).collect::<Vec<_>>(), ["VST: ReaComp (Cockos)", "VST: ReaEQ (Cockos)"]);
    // The producer turned the compressor's parameter: undo is refused, unless discarded.
    state(|s| s.tracks[1].fx[0].params[0].value = 0.9);
    let (code, message) = lab.undo("fx-add", &preview2, &applied2, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("changed in REAPER"), "{message}");
    assert_eq!(kick_fx().len(), 2);
    let mut params = restore_params(&preview2, &applied2, "Kumi: undo (kund1)");
    params["kind"] = "fx-add".into();
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(kick_fx().iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), [eq.as_str()]);
    // Undone already: it is gone.
    assert_eq!(lab.undo("fx-add", &preview2, &applied2, "Kumi: undo (kund2)").unwrap_err().0, "gone");
    lab.undo("fx-add", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    assert!(kick_fx().is_empty());
}

#[test]
fn a_plug_in_that_isnt_there_or_a_bad_request_writes_nothing() {
    let lab = Lab::new();
    let entries = state(|s| s.undo_entries);
    for (input, code) in [
        (json!({"ref": KICK, "name": "NoSuchPlugin"}), "failed"),
        (json!({"ref": KICK, "name": ""}), "bad-params"),
        (json!({"ref": KICK}), "bad-params"),
        (json!({"ref": KICK, "name": "a\nb"}), "bad-params"),
        (json!({"ref": KICK, "name": "mine.RfxChain"}), "bad-params"),
        (json!({"ref": KICK, "name": "ReaEQ", "index": 3}), "bad-params"),
        (json!({"ref": "r:fx:{FX-EQ}", "name": "ReaEQ"}), "bad-params"),
    ] {
        // Unknown plug-ins are found out when it is added, not at the preview.
        let preview = lab.call("change_preview", json!({"kind": "fx-add", "input": input}));
        if code == "failed" {
            let preview = preview.unwrap();
            let apply = json!({"kind": "fx-add", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
            let (got, message) = lab.call("change_apply", apply).unwrap_err();
            assert_eq!(got, code, "{input}");
            assert!(message.contains("NoSuchPlugin"), "{message}");
        } else {
            assert_eq!(preview.unwrap_err().0, code, "{input}");
        }
    }
    assert_eq!((kick_fx().len(), state(|s| s.undo_entries)), (0, entries));
    // The chain moved between the preview and the apply.
    let input = json!({"ref": KICK, "name": "ReaEQ"});
    let preview = lab.call("change_preview", json!({"kind": "fx-add", "input": input})).unwrap();
    state(|s| s.tracks[1].fx.push(fake::FakeFx::new("{FX-X}", "VST: ReaEQ (Cockos)")));
    let apply = json!({"kind": "fx-add", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
    assert_eq!(kick_fx().len(), 1);
}

fn three_fx(lab: &Lab) -> Vec<String> {
    for (n, name) in ["ReaEQ", "ReaComp", "ReaDelay"].iter().enumerate() {
        lab.make("fx-add", json!({"ref": KICK, "name": name}), &format!("Kumi: Added {name} to Kick (kfx0{n})"));
    }
    kick_fx().into_iter().map(|f| f.0).collect()
}

#[test]
fn an_fx_is_moved_to_a_place_in_the_chain_and_undo_puts_it_back_unless_the_chain_was_rearranged() {
    let lab = Lab::new();
    let guids = three_fx(&lab);
    let (preview, applied) =
        lab.make("fx-move", json!({"ref": fx_ref_of(&guids[2]), "index": 0}), "Kumi: Moved ReaDelay to place 1 on Kick (kfm01)");
    assert_eq!(preview["display"]["title"], "Moved “ReaDelay” to place 1 on “Kick”");
    assert_eq!(kick_fx().iter().map(|f| f.0.clone()).collect::<Vec<_>>(), [guids[2].clone(), guids[0].clone(), guids[1].clone()]);
    // The producer moved another since.
    state(|s| s.tracks[1].fx.swap(1, 2));
    assert_eq!(lab.undo("fx-move", &preview, &applied, "Kumi: undo (kund1)").unwrap_err().0, "changed");
    state(|s| s.tracks[1].fx.swap(1, 2));
    lab.undo("fx-move", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(kick_fx().iter().map(|f| f.0.clone()).collect::<Vec<_>>(), guids);
    // Undone already.
    lab.undo("fx-move", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(kick_fx().iter().map(|f| f.0.clone()).collect::<Vec<_>>(), guids);
    // To where it is: nothing to do. Past the end: refused.
    let same = lab.call("change_preview", json!({"kind": "fx-move", "input": {"ref": fx_ref_of(&guids[1]), "index": 1}})).unwrap();
    assert_eq!(same["display"]["unchanged"], true);
    for index in [json!(3), json!(-1), json!("a"), Value::Null] {
        let input = json!({"ref": fx_ref_of(&guids[1]), "index": index});
        assert_eq!(lab.call("change_preview", json!({"kind": "fx-move", "input": input})).unwrap_err().0, "bad-params", "{index}");
    }
    // The producer rearranged between the preview and the apply.
    let input = json!({"ref": fx_ref_of(&guids[0]), "index": 2});
    let preview = lab.call("change_preview", json!({"kind": "fx-move", "input": input})).unwrap();
    state(|s| s.tracks[1].fx.swap(0, 1));
    let apply = json!({"kind": "fx-move", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
}

#[test]
fn an_fx_is_bypassed_and_enabled_and_undo_refuses_when_the_producer_switched_it() {
    let lab = Lab::new();
    let guids = three_fx(&lab);
    let reference = fx_ref_of(&guids[1]);
    let (preview, applied) = lab.make("fx-bypass", json!({"ref": reference, "bypass": true}), "Kumi: Bypassed ReaComp on Kick (kfb01)");
    assert_eq!(preview["display"]["title"], "Bypassed “ReaComp” on “Kick”");
    assert_eq!(kick_fx().iter().map(|f| f.2).collect::<Vec<_>>(), [true, false, true]);
    // The producer switched it back since: it is where undo would put it, and nothing is written.
    state(|s| s.tracks[1].fx[1].enabled = true);
    lab.undo("fx-bypass", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert!(kick_fx()[1].2);
    state(|s| s.tracks[1].fx[1].enabled = false);
    lab.undo("fx-bypass", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert!(kick_fx()[1].2);
    let (preview, _) = lab.make("fx-bypass", json!({"ref": reference, "bypass": false}), "Kumi: Enabled ReaComp on Kick (kfb02)");
    assert_eq!(preview["display"]["unchanged"], true);
    assert_eq!(lab.call("change_preview", json!({"kind": "fx-bypass", "input": {"ref": reference}})).unwrap_err().0, "bad-params");
    // Changed between the preview and the apply.
    let input = json!({"ref": reference, "bypass": true});
    let preview = lab.call("change_preview", json!({"kind": "fx-bypass", "input": input})).unwrap();
    state(|s| s.tracks[1].fx[1].enabled = false);
    let apply = json!({"kind": "fx-bypass", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
}

#[test]
fn a_parameter_is_set_by_its_normalized_value_or_by_what_the_plug_in_shows() {
    let lab = Lab::new();
    let guids = three_fx(&lab);
    let fx = fx_ref_of(&guids[0]);
    let param = |n: usize| -> f64 { state(|s| s.tracks[1].fx[0].params[n].value) };
    // Gain is -12 to 12 dB, centre at 0.5.
    let (preview, applied) =
        lab.make("fx-param", json!({"ref": fx, "param": "Gain", "normalized": 0.75}), "Kumi: ReaEQ Gain 6.0 dB on Kick (kfp01)");
    assert_eq!(preview["display"]["title"], "ReaEQ Gain 6.0 dB on “Kick”");
    assert_eq!((preview["prior"]["display"].as_str(), applied["applied"]["display"].as_str()), (Some("0.0 dB"), Some("6.0 dB")));
    assert_eq!(param(0), 0.75);
    lab.undo("fx-param", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(param(0), 0.5);
    // By what it shows.
    let by_ref = format!("r:param:{}:0", guids[0]);
    let (preview, applied) = lab.make("fx-param", json!({"ref": by_ref, "value": "-3 dB"}), "Kumi: ReaEQ Gain -3.0 dB on Kick (kfp02)");
    // The first value the plug-in prints as -3.0 dB.
    assert!((param(0) - 0.375).abs() < 0.005, "{}", param(0));
    assert_eq!(applied["applied"]["display"], "-3.0 dB");
    assert_eq!(preview["display"]["after"], "-3.0 dB");
    // Words.
    lab.make("fx-param", json!({"ref": fx, "param": 1, "value": "high"}), "Kumi: ReaEQ Mode High on Kick (kfp03)");
    assert!(param(1) > 0.75 && param(1) < 1.0, "the middle of the values that print High: {}", param(1));
    // The producer turned it since: undo is refused, unless discarded.
    state(|s| s.tracks[1].fx[0].params[0].value = 0.9);
    assert_eq!(lab.undo("fx-param", &preview, &applied, "Kumi: undo (kund2)").unwrap_err().0, "changed");
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund2)");
    params["kind"] = "fx-param".into();
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(param(0), 0.5);
}

#[test]
fn a_parameter_request_that_cant_be_met_writes_nothing() {
    let lab = Lab::new();
    let guids = three_fx(&lab);
    let fx = fx_ref_of(&guids[0]);
    let entries = state(|s| s.undo_entries);
    for (input, code) in [
        (json!({"ref": fx, "param": "Gain"}), "bad-params"),
        (json!({"ref": fx, "param": "Gain", "normalized": 1.5}), "bad-params"),
        (json!({"ref": fx, "param": "Gain", "normalized": 0.5, "value": "0 dB"}), "bad-params"),
        (json!({"ref": fx, "normalized": 0.5}), "bad-params"),
        (json!({"ref": fx, "param": "Nope", "normalized": 0.5}), "bad-params"),
        (json!({"ref": fx, "param": 9, "normalized": 0.5}), "bad-params"),
        (json!({"ref": format!("r:param:{}:9", guids[0]), "normalized": 0.5}), "bad-params"),
        (json!({"ref": fx, "param": "Gain", "value": "40 dB"}), "failed"),
        (json!({"ref": fx, "param": "Gain", "value": "-3.25 dB"}), "failed"),
        (json!({"ref": fx, "param": "Mode", "value": "Loud"}), "failed"),
        (json!({"ref": fx, "param": "Gain", "value": 3}), "bad-params"),
        (json!({"ref": "r:fx:{FX-COMP}", "param": 0, "normalized": 0.1}), "bad-params"),
        (json!({"ref": "r:fx:{FX-C2}", "param": 0, "normalized": 0.1}), "bad-params"),
    ] {
        let (got, message) = lab.call("change_preview", json!({"kind": "fx-param", "input": input})).unwrap_err();
        assert_eq!(got, code, "{input}: {message}");
    }
    assert_eq!(state(|s| s.undo_entries), entries);
    // Moved between the preview and the apply.
    let input = json!({"ref": fx, "param": "Gain", "normalized": 0.7});
    let preview = lab.call("change_preview", json!({"kind": "fx-param", "input": input})).unwrap();
    state(|s| s.tracks[1].fx[0].params[0].value = 0.1);
    let apply = json!({"kind": "fx-param", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
}

#[test]
fn deleting_an_fx_keeps_its_entry_and_undo_adds_it_back_with_its_id_and_state_and_no_file_is_left() {
    let lab = Lab::new();
    let dir = fx_folder();
    let guids = three_fx(&lab);
    state(|s| {
        s.tracks[1].fx[1].params[0].value = 0.8;
        s.tracks[1].fx[1].enabled = false;
    });
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx_ref_of(&guids[1])}), "Kumi: Deleted ReaComp from Kick (kfd01)");
    assert_eq!(preview["display"]["title"], "Deleted “ReaComp” from “Kick”");
    assert_eq!(kick_fx().iter().map(|f| f.0.clone()).collect::<Vec<_>>(), [guids[0].clone(), guids[2].clone()]);
    let entry = applied["material"]["entry"].as_str().unwrap();
    assert!(entry.starts_with("BYPASS 1 0 0\n<VST \"VST: ReaComp (Cockos)\"") && entry.contains(&format!("FXID {}", guids[1])), "{entry}");
    let restored = lab.undo("fx-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["restored"]["ref"], fx_ref_of(&guids[1]));
    assert_eq!(kick_fx().iter().map(|f| f.0.clone()).collect::<Vec<_>>(), guids, "in its place, with its id");
    let (enabled, gain) = state(|s| (s.tracks[1].fx[1].enabled, s.tracks[1].fx[1].params[0].value));
    assert_eq!((enabled, gain), (false, 0.8), "its state came back");
    assert_eq!(chain_files(&dir), 0, "the chain file is removed");
    // Back already: nothing is added twice.
    lab.undo("fx-delete", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(kick_fx().len(), 3);
    // The chain got shorter since: it goes where it can.
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx_ref_of(&guids[2])}), "Kumi: Deleted ReaDelay from Kick (kfd02)");
    state(|s| s.tracks[1].fx.clear());
    lab.undo("fx-delete", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    assert_eq!(kick_fx().iter().map(|f| f.0.clone()).collect::<Vec<_>>(), [guids[2].clone()]);
    assert_eq!(chain_files(&dir), 0);
}

#[test]
fn deleting_an_fx_is_refused_when_it_changed_or_went() {
    let lab = Lab::new();
    let guids = three_fx(&lab);
    let input = json!({"ref": fx_ref_of(&guids[1])});
    let preview = lab.call("change_preview", json!({"kind": "fx-delete", "input": input})).unwrap();
    state(|s| s.tracks[1].fx[1].params[0].value = 0.9);
    let entries = state(|s| s.undo_entries);
    let apply = json!({"kind": "fx-delete", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    assert_eq!(lab.call("change_apply", apply.clone()).unwrap_err().0, "changed");
    state(|s| s.tracks[1].fx.remove(1));
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "changed");
    assert_eq!(state(|s| s.undo_entries), entries);
    assert_eq!(lab.call("change_preview", json!({"kind": "fx-delete", "input": input})).unwrap_err().0, "failed");
    // (containers and the FX inside them are deleted too since M4: tests_fxenv has them)
    for reference in ["r:track:{BBBB}", "r:fx:{NOPE}"] {
        let (code, _) = lab.call("change_preview", json!({"kind": "fx-delete", "input": {"ref": reference}})).unwrap_err();
        assert!(code == "bad-params" || code == "failed", "{reference}: {code}");
    }
}

#[test]
fn the_fx_kinds_are_refused_for_another_epoch_and_while_reaper_is_busy() {
    let lab = Lab::new();
    let guids = three_fx(&lab);
    let fx = fx_ref_of(&guids[0]);
    for (kind, input) in [
        ("fx-add", json!({"ref": KICK, "name": "ReaEQ"})),
        ("fx-delete", json!({"ref": fx})),
        ("fx-move", json!({"ref": fx, "index": 1})),
        ("fx-bypass", json!({"ref": fx, "bypass": true})),
        ("fx-param", json!({"ref": fx, "param": 0, "normalized": 0.2})),
        ("notes-add", json!({"ref": SNARE_TAKE, "notes": [{"pitch": 60, "start": 1.0, "length": 1.0}]})),
        ("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [0]})),
    ] {
        let (code, _) = lab.call("change_preview", json!({"kind": kind, "input": input, "epoch": "old-1"})).unwrap_err();
        assert_eq!(code, "stale", "{kind}");
        let preview = lab.call("change_preview", json!({"kind": kind, "input": input})).unwrap();
        state(|s| s.loading = true);
        let apply = json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
        assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "busy", "{kind}");
        state(|s| s.loading = false);
    }
    assert_eq!((kick_fx().len(), snare_notes().len()), (3, 2));
}

#[test]
fn transport_play_starts_what_isnt_playing_and_the_cursor_moves_without_an_undo_point() {
    let lab = Lab::new();
    assert_eq!(
        lab.call("transport_play", json!({})).unwrap(),
        json!({"started": false, "playing": false}),
        "the fake's action doesn't start it"
    );
    assert_eq!(state(|s| s.actions.clone()), [1007]);
    state(|s| {
        s.actions.clear();
        s.play_bits = 1;
    });
    assert_eq!(lab.call("transport_play", json!({})).unwrap(), json!({"started": false, "playing": true}));
    assert!(state(|s| s.actions.is_empty()), "already playing: nothing to start");
    state(|s| s.loading = true);
    assert_eq!(lab.call("transport_play", json!({})).unwrap_err().0, "busy");
    state(|s| s.loading = false);

    let entries = state(|s| s.undo_entries);
    let moved = lab.call("cursor_set", json!({"positionBeats": 8.0})).unwrap();
    assert_eq!((moved["position"].clone(), moved["positionBeats"].clone()), (json!(4.0), json!(8.0)));
    assert_eq!((state(|s| s.cursor), state(|s| s.undo_entries)), (4.0, entries));
    assert_eq!(lab.call("cursor_set", json!({"position": 1.5})).unwrap()["position"], 1.5);
    for params in [json!({}), json!({"position": 1.0, "positionBeats": 2.0}), json!({"position": -1.0}), json!({"position": "a"})] {
        assert_eq!(lab.call("cursor_set", params).unwrap_err().0, "bad-params");
    }
    assert_eq!(lab.call("cursor_set", json!({"position": 1.0, "epoch": "old-1"})).unwrap_err().0, "stale");
    state(|s| s.loading = true);
    assert_eq!(lab.call("cursor_set", json!({"position": 1.0})).unwrap_err().0, "busy");
}

#[test]
fn the_marker_reaper_makes_at_the_start_with_the_first_one_goes_with_it_unless_the_producer_changed_it() {
    let lab = Lab::new();
    assert!(tempo_markers().is_empty());
    let (preview, applied) =
        lab.make("tempo-marker-set", json!({"positionBeats": 16.0, "bpm": 140.0}), "Kumi: Tempo 140 BPM at bar 5 (ktb01)");
    assert_eq!(tempo_markers(), [(0.0, 120.0, 4, 4, false), (8.0, 140.0, -1, -1, false)]);
    assert_eq!(applied["applied"]["base"], json!({"exists": true, "bpm": 120.0, "num": 4, "den": 4, "linear": false}));
    lab.undo("tempo-marker-set", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert!(tempo_markers().is_empty(), "back to a project with the one tempo");
    // The producer changed the one at the start: it stays.
    let (preview, applied) =
        lab.make("tempo-marker-set", json!({"positionBeats": 16.0, "bpm": 140.0}), "Kumi: Tempo 140 BPM at bar 5 (ktb02)");
    state(|s| s.tempo_markers[0].bpm = 100.0);
    lab.undo("tempo-marker-set", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(tempo_markers(), [(0.0, 100.0, 4, 4, false)]);
    // A marker at the start itself brings nothing.
    state(|s| s.tempo_markers.clear());
    let (_, applied) = lab.make("tempo-marker-set", json!({"position": 0.0, "bpm": 90.0}), "Kumi: Tempo 90 BPM at bar 1 (ktb03)");
    assert!(applied["applied"].get("base").is_none());
}

#[test]
fn the_controller_events_in_a_take_are_untouched_by_a_notes_change() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[2].items[0].takes[0].ccs =
            (0..5).map(|i| fake::FakeCc { ppq: i as f64 * 480.0, kind: 0xB0, channel: 0, data1: 1, data2: 10 * i }).collect();
    });
    let ccs = || state(|s| s.tracks[2].items[0].takes[0].ccs.iter().map(|c| (c.ppq as i64, c.data1, c.data2)).collect::<Vec<_>>());
    let before = ccs();
    let (preview, applied) = lab.make(
        "notes-add",
        json!({"ref": SNARE_TAKE, "notes": [{"pitch": 50, "start": 0.0, "length": 1.0}, {"pitch": 52, "start": 2.0, "length": 1.0}]}),
        "Kumi: Wrote 2 notes in snare.mid take (kcc01)",
    );
    assert_eq!((ccs(), snare_notes().len()), (before.clone(), 4));
    lab.undo("notes-add", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!((ccs(), snare_notes().len()), (before, 2));
}

#[test]
fn a_note_that_would_overlap_one_of_the_same_pitch_is_refused_so_no_note_is_shortened() {
    let lab = Lab::new();
    // The snare take has pitch 38 on channel 1 from tick 0 to 480 (beat 0 to 0.5).
    for (input, ok) in [
        (json!({"pitch": 38, "start": 0.25, "length": 1.0}), false),
        (json!({"pitch": 38, "start": 0.5, "length": 1.0}), true),
        (json!({"pitch": 38, "start": 0.25, "length": 1.0, "channel": 2}), true),
        (json!({"pitch": 39, "start": 0.25, "length": 1.0}), true),
    ] {
        let result = lab.call("change_preview", json!({"kind": "notes-add", "input": {"ref": SNARE_TAKE, "notes": [input]}}));
        assert_eq!(result.is_ok(), ok, "{input}: {result:?}");
        if let Err((code, message)) = result {
            assert_eq!(code, "bad-params");
            assert!(message.contains("overlap"), "{message}");
        }
    }
    // Two new notes that overlap each other.
    let both = json!([{"pitch": 50, "start": 4.0, "length": 1.0}, {"pitch": 50, "start": 4.5, "length": 1.0}]);
    assert_eq!(
        lab.call("change_preview", json!({"kind": "notes-add", "input": {"ref": SNARE_TAKE, "notes": both}})).unwrap_err().0,
        "bad-params"
    );
    // A change that moves a note onto another.
    let onto = json!([{"index": 1, "pitch": 38, "channel": 1, "start": 0.25}]);
    assert_eq!(
        lab.call("change_preview", json!({"kind": "notes-change", "input": {"ref": SNARE_TAKE, "changes": onto}})).unwrap_err().0,
        "bad-params"
    );
    // Taking a note away makes room for one in its place.
    let swap = json!([{"index": 0, "start": 0.0, "length": 1.0}]);
    lab.make("notes-change", json!({"ref": SNARE_TAKE, "changes": swap}), "Kumi: Changed 1 note in snare.mid take (kov01)");
    // A take that already overlaps (the producer's): adding elsewhere is no matter.
    state(|s| {
        s.tracks[2].items[0].takes[0].notes.push(fake::FakeNote {
            start_ppq: 100.0,
            end_ppq: 500.0,
            channel: 0,
            pitch: 38,
            velocity: 90,
            selected: false,
            muted: false,
        })
    });
    lab.make(
        "notes-add",
        json!({"ref": SNARE_TAKE, "notes": [{"pitch": 60, "start": 8.0, "length": 1.0}]}),
        "Kumi: Wrote 1 note in snare.mid take (kov02)",
    );
}

#[test]
fn a_plan_is_named_by_what_it_did_when_it_ends() {
    let lab = Lab::new();
    let id = lab.call("undo_step_begin", json!({"label": "Kumi: Kumi (kplan)"})).unwrap()["stepId"].as_str().unwrap().to_string();
    lab.make("track-mute", json!({"ref": KICK, "mute": true}), "Kumi: Kick muted (kp001)");
    lab.make("track-pan", json!({"ref": KICK, "pan": 0.2}), "Kumi: Kick pan 20% right (kp002)");
    let ended = lab.call("undo_step_end", json!({"stepId": id, "label": "Kumi: Kick muted and 1 more (kplan)"})).unwrap();
    assert_eq!((ended["desc"].clone(), ended["entry"].clone()), (json!("Kumi: Kick muted and 1 more (kplan)"), json!(true)));
    assert_eq!(top().as_deref(), Some("Kumi: Kick muted and 1 more (kplan)"), "REAPER's undo menu says what the plan did");
    // A malformed optional rename still closes under the label it began with.
    for label in [json!(7), json!(""), json!("Warm\0pad"), json!("Warm\npad"), json!("Warm\u{7f}pad")] {
        let id = lab.call("undo_step_begin", json!({"label": "Kumi: Kumi (kplan)"})).unwrap()["stepId"].clone();
        state(|s| s.undo_dirty = true);
        let ended = lab.call("undo_step_end", json!({"stepId": id, "label": label})).unwrap();
        assert_eq!(ended["desc"], "Kumi: Kumi (kplan)");
        assert_eq!(ended["entry"], true);
        assert_eq!(state(|s| s.undo_depth), 0);
        assert!(lab.changes.plan.borrow().is_none());
        assert!(lab.call("undo_step_end", json!({"stepId": id})).is_err());
    }
}

#[test]
fn a_parameters_title_keeps_the_unit_the_plug_in_prints_even_when_its_formatted_value_leaves_it_off() {
    let lab = Lab::new();
    let guids = three_fx(&lab);
    let fx = fx_ref_of(&guids[0]);
    // The formatted value of Gain is a bare number, as ReaEQ's is; asking the plug-in to print the value keeps the unit.
    assert!(state(|s| s.tracks[1].fx[0].params[0].bare));
    let (preview, applied) =
        lab.make("fx-param", json!({"ref": fx, "param": "Gain", "normalized": 0.75}), "Kumi: ReaEQ Gain 6.0 dB on Kick (ktu01)");
    assert_eq!(preview["display"]["title"], "ReaEQ Gain 6.0 dB on “Kick”");
    assert_eq!(applied["display"]["title"], "ReaEQ Gain 6.0 dB on “Kick”");
    assert_eq!((preview["prior"]["display"].as_str(), applied["applied"]["display"].as_str()), (Some("0.0 dB"), Some("6.0 dB")));
    let restored = lab.undo("fx-param", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["display"]["title"], "ReaEQ Gain 0.0 dB on “Kick” (put back)");
    // A parameter that prints no unit has its number.
    let (preview, applied) =
        lab.make("fx-param", json!({"ref": fx, "param": "Offset", "normalized": 1.0}), "Kumi: ReaEQ Offset 1.00 on Kick (ktu02)");
    assert_eq!(preview["display"]["title"], "ReaEQ Offset 1.00 on “Kick”");
    assert_eq!(applied["display"]["title"], "ReaEQ Offset 1.00 on “Kick”");
}

#[test]
fn a_track_called_track_005_reads_without_stuttering_in_every_title_and_its_restore() {
    let lab = Lab::new();
    state(|s| {
        s.tracks.push(fake::FakeTrack::new("{EEEE}", "Track 005"));
        s.state_count += 1;
    });
    let five = "r:track:{EEEE}";
    let title = |preview: &Value| preview["display"]["title"].as_str().unwrap().to_string();
    let no_stutter = |text: &str| assert!(!text.contains("Track Track") && !text.contains("track Track"), "{text}");

    // A rename: the names in quotes, the arrow between them; undone, it goes the other way.
    let (preview, applied) =
        lab.make("track-rename", json!({"ref": five, "name": "Lead Vox"}), "Kumi: Renamed track “Track 005” → “Lead Vox” (kst01)");
    assert_eq!(title(&preview), "Renamed track “Track 005” → “Lead Vox”");
    assert_eq!(applied["display"]["title"], "Renamed track “Track 005” → “Lead Vox”");
    let restored = lab.undo("track-rename", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["display"]["title"], "Renamed track “Lead Vox” → “Track 005” (put back)");

    // A title that begins with the track's name leaves it bare, and its restore does not stutter.
    let (preview, applied) = lab.make("track-volume", json!({"ref": five, "volumeDb": -3.0}), "Kumi: Track 005 volume −3 dB (kst02)");
    assert_eq!(title(&preview), "Track 005 volume −3 dB");
    let restored = lab.undo("track-volume", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(restored["display"]["title"], "Track 005 volume 0 dB (put back)");

    // A delete and its restore.
    let (preview, applied) = lab.make("track-delete", json!({"ref": five}), "Kumi: Deleted track “Track 005” (kst03)");
    assert_eq!(title(&preview), "Deleted track “Track 005”");
    assert_eq!(applied["display"]["title"], "Deleted track “Track 005”");
    let restored = lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    assert_eq!(restored["display"]["title"], "“Track 005” put back");

    // A move, a send, an FX and a track added by that name.
    let (preview, applied) =
        lab.make("track-move", json!({"ref": five, "index": 1}), "Kumi: Moved track “Track 005” to position 1 (kst04)");
    assert_eq!(title(&preview), "Moved track “Track 005” to position 1");
    let restored = lab.undo("track-move", &preview, &applied, "Kumi: undo (kund4)").unwrap();
    assert_eq!(restored["display"]["title"], "“Track 005” back at position 5");
    let (preview, _) = lab.make(
        "send-add",
        json!({"ref": five, "toRef": "r:track:{DDDD}", "volumeDb": -6.0}),
        "Kumi: Added send “Track 005” → “Reverb” (kst05)",
    );
    assert_eq!(title(&preview), "Added send “Track 005” → “Reverb”");
    let (preview, applied) = lab.make("fx-add", json!({"ref": five, "name": "ReaEQ"}), "Kumi: Added “ReaEQ” to “Track 005” (kst06)");
    assert_eq!(title(&preview), "Added “ReaEQ” to “Track 005”");
    let restored = lab.undo("fx-add", &preview, &applied, "Kumi: undo (kund6)").unwrap();
    assert_eq!(restored["display"]["title"], "“ReaEQ” taken off “Track 005”");
    let (preview, applied) = lab.make("track-add", json!({"name": "Track 006"}), "Kumi: Added track “Track 006” (kst07)");
    assert_eq!(title(&preview), "Added track “Track 006”");
    assert_eq!(applied["display"]["title"], "Added track “Track 006”");

    // A track with no name is the number REAPER shows.
    state(|s| {
        s.tracks[4].name = String::new();
        s.state_count += 1;
    });
    let (preview, applied) = lab.make("track-delete", json!({"ref": five}), "Kumi: Deleted track “Track 5” (kst08)");
    assert_eq!(title(&preview), "Deleted track “Track 5”");
    let restored = lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund8)").unwrap();
    assert_eq!(restored["display"]["title"], "“Track 5” put back");
    for text in [title(&preview), restored["display"]["title"].as_str().unwrap().to_string()] {
        no_stutter(&text);
    }
}

// ----- sends by identity (M1) -----

/// The refs of a track's sends, as a read gives them.
fn send_refs(track: i32) -> Vec<String> {
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let owner = reaper.track_at(reaper.current_project().unwrap().ptr, track).unwrap();
    crate::rows::send_keys(&reaper, owner, 0)
        .iter()
        .enumerate()
        .map(|(index, key)| crate::rows::send_row(&reaper, owner, 0, index as i32, key)["ref"].as_str().unwrap().to_string())
        .collect()
}

/// Kick sends twice to Reverb: s1 at -12 dB and s2 at -6 dB (the producer's own sends, with no mark). Their refs, read.
fn two_sends() -> (String, String) {
    state(|s| {
        let mut first = FakeSend::to(3);
        first.volume = 0.25;
        let mut second = FakeSend::to(3);
        second.volume = 0.5;
        s.tracks[1].sends = vec![first, second];
        s.state_count += 1;
    });
    let refs = send_refs(1);
    (refs[0].clone(), refs[1].clone())
}

fn send_volumes() -> Vec<(f64, bool)> {
    state(|s| s.tracks[1].sends.iter().map(|send| ((send.volume * 100.0).round() / 100.0, send.mute)).collect())
}

/// The producer removes the first send and makes a new one (a fresh send is at 0 dB).
fn producer_swaps_the_first_send() {
    state(|s| {
        s.tracks[1].sends.remove(0);
        s.tracks[1].sends.push(FakeSend::to(3));
        s.state_count += 1;
    });
}

#[test]
fn a_send_ref_that_names_another_send_after_the_producer_removed_one_and_added_one_is_refused_at_the_apply() {
    let lab = Lab::new();
    let (first, _) = two_sends();
    let input = json!({"ref": first, "mute": true});
    let preview = lab.call("change_preview", json!({"kind": "send-mute", "input": input})).unwrap();
    producer_swaps_the_first_send();
    let before = send_volumes();
    let entries = state(|s| s.undo_entries);
    let apply = json!({"kind": "send-mute", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    let (code, message) = lab.call("change_apply", apply).unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("changed") || message.contains("another send"), "{message}");
    assert_eq!((send_volumes(), state(|s| s.undo_entries)), (before.clone(), entries), "no send was muted");
    assert!(before.iter().all(|(_, mute)| !mute));
}

/// A preview and an apply through a ref, as Kumi makes them; the error, if one refused.
fn mute_through(lab: &Lab, reference: &str) -> Option<(String, String)> {
    let input = json!({"ref": reference, "mute": true});
    let preview = match lab.call("change_preview", json!({"kind": "send-mute", "input": input})) {
        Ok(preview) => preview,
        Err(fault) => return Some(fault),
    };
    lab.call("change_apply", json!({"kind": "send-mute", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k2222)"})).err()
}

#[test]
fn a_send_ref_read_before_the_producer_removed_one_and_added_one_is_refused_at_the_preview_whatever_was_read_since() {
    for poll_between in [false, true] {
        let lab = Lab::new();
        let (first, _) = two_sends();
        producer_swaps_the_first_send();
        if poll_between {
            // FOCUS's routing body reads the sends again while the producer looks at them.
            send_refs(1);
        }
        let before = send_volumes();
        let (code, message) = mute_through(&lab, &first).expect("refused");
        assert_eq!(code, "changed");
        assert!(message.contains("read them again"), "{message}");
        assert_eq!(send_volumes(), before, "no send was muted (poll between: {poll_between})");
    }
}

#[test]
fn a_send_ref_read_before_kumi_set_that_sends_automation_mode_or_showed_its_envelope_still_names_it() {
    let lab = Lab::new();
    // (REAPER hands out each send's volume envelope, hidden and off until shown)
    state(|s| {
        let mut envelope = fake::FakeEnvelope::for_send("Send Volume: Reverb", 0, "<VOLENV", 0);
        envelope.active = false;
        envelope.visible = false;
        envelope.pts = Some(vec![]);
        s.tracks[1].envelopes.push(envelope);
    });
    let (first, _) = two_sends();
    // Kumi's own writes to the first send outside the send kinds: its automation mode, its volume envelope shown.
    let (preview, applied) = lab.make("send-automation-mode", json!({"ref": first, "mode": "touch"}), "Kumi: x (km001)");
    let (shown_preview, shown) = lab.make("envelope-show", json!({"sendRef": first, "name": "volume"}), "Kumi: x (km002)");
    // The ref read before both still names that send, for a change and for both undos.
    assert_eq!(mute_through(&lab, &first), None);
    assert!(state(|s| s.tracks[1].sends[0].mute && !s.tracks[1].sends[1].mute));
    lab.undo("envelope-show", &shown_preview, &shown, "Kumi: undo (km003)").unwrap();
    lab.undo("send-automation-mode", &preview, &applied, "Kumi: undo (km004)").unwrap();
    assert_eq!(state(|s| s.tracks[1].sends[0].extra.get("I_AUTOMODE").copied().unwrap_or(-1.0)), -1.0);
}

#[test]
fn a_send_ref_read_before_the_producer_changed_the_other_send_to_that_track_is_refused() {
    let lab = Lab::new();
    let (first, _) = two_sends();
    state(|s| s.tracks[1].sends[1].volume = 0.9);
    let (code, message) = mute_through(&lab, &first).expect("refused");
    assert_eq!(code, "changed");
    assert!(message.contains("read them again"), "{message}");
    assert!(send_volumes().iter().all(|(_, mute)| !mute));
}

#[test]
fn a_plan_over_two_sends_to_one_track_from_one_read_goes_through_until_the_producer_changes_them() {
    let lab = Lab::new();
    let (first, second) = two_sends();
    lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send muted (ksi10)");
    lab.make("send-volume", json!({"ref": second, "volumeDb": -3.0}), "Kumi: Send -3 dB (ksi11)");
    lab.make("send-volume", json!({"ref": first, "volumeDb": -20.0}), "Kumi: Send -20 dB (ksi12)");
    assert_eq!(send_volumes(), [(0.1, true), (0.71, false)]);
    // The producer pans one: Kumi's writes no longer lead to what is there, and the refs read before name nothing.
    state(|s| s.tracks[1].sends[0].pan = 0.5);
    let (code, _) = mute_through(&lab, &second).expect("refused");
    assert_eq!(code, "changed");
    // Read again: the refs name them again.
    let refs = send_refs(1);
    assert_eq!(mute_through(&lab, &refs[1]), None);
    assert_eq!(send_volumes(), [(0.1, true), (0.71, true)]);
}

#[test]
fn a_send_alone_to_its_track_is_the_send_to_that_track_even_after_the_producer_made_it_again() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].sends = vec![FakeSend::to(3)];
        s.state_count += 1;
    });
    let reference = send_refs(1).remove(0);
    assert_eq!(reference, "r:send:{BBBB}:0:{DDDD}", "a send alone to its track carries nothing of its state");
    state(|s| {
        s.tracks[1].sends = vec![FakeSend { volume: 0.5, ..FakeSend::to(3) }];
        s.state_count += 1;
    });
    assert_eq!(mute_through(&lab, &reference), None);
    assert_eq!(send_volumes(), [(0.5, true)], "the new send to Reverb is muted");
}

#[test]
fn a_ref_to_one_of_two_sends_without_their_state_is_refused() {
    let lab = Lab::new();
    two_sends();
    let (code, message) = mute_through(&lab, "r:send:{BBBB}:0:{DDDD}:1of2").expect("refused");
    assert_eq!(code, "changed");
    assert!(message.contains("read them again"), "{message}");
}

#[test]
fn an_undo_told_to_put_the_values_back_anyway_still_writes_only_to_the_send_it_changed() {
    let undo_anyway = |preview: &Value, applied: &Value| {
        json!({"kind": "send-mute", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"],
            "desc": "Kumi: undo (kund9)", "discard": true})
    };
    let lab = Lab::new();
    let (first, _) = two_sends();
    let (preview, applied) = lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send “Kick” → “Reverb” muted (ksi20)");
    // The producer removes Kumi's send, adds one of their own, and mutes the one now at its place.
    producer_swaps_the_first_send();
    state(|s| s.tracks[1].sends[0].mute = true);
    let before = send_volumes();
    let (code, message) = lab.call("change_restore", undo_anyway(&preview, &applied)).unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("isn't there with its mark"), "{message}");
    assert_eq!(send_volumes(), before, "the producer's mute stays");
    // The producer only turned Kumi's send down since: it is the same send, and the mute is taken back anyway.
    let lab = Lab::new();
    let (first, _) = two_sends();
    let (preview, applied) = lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send “Kick” → “Reverb” muted (ksi21)");
    state(|s| s.tracks[1].sends[0].volume = 0.1);
    lab.call("change_restore", undo_anyway(&preview, &applied)).unwrap();
    assert_eq!(send_volumes(), [(0.1, false), (0.5, false)]);
}

#[test]
fn a_send_kumi_changes_gets_its_mark_so_its_undo_finds_it_among_sends_alike_in_every_value() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].sends = vec![FakeSend::to(3), FakeSend::to(3)];
        s.state_count += 1;
    });
    let first = send_refs(1).remove(0);
    let (preview, applied) = lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send “Kick” → “Reverb” muted (ksi30)");
    assert!(state(|s| s.tracks[1].sends[0].ext.contains_key("P_EXT:kumi_id")), "marked in the same step");
    // The producer mutes the other send too, then moves Kumi's after it: Kumi's undo unmutes its own, by its mark.
    state(|s| {
        s.tracks[1].sends[1].mute = true;
        s.tracks[1].sends.swap(0, 1);
        s.state_count += 1;
    });
    lab.undo("send-mute", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(send_volumes(), [(1.0, true), (1.0, false)], "the producer's mute stays");
    // The producer removes Kumi's send and makes one alike: Kumi's undo, even told to discard, finds no send of its own.
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].sends = vec![FakeSend::to(3), FakeSend::to(3)];
        s.state_count += 1;
    });
    let first = send_refs(1).remove(0);
    let (preview, applied) = lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send “Kick” → “Reverb” muted (ksi31)");
    state(|s| {
        s.tracks[1].sends.remove(0);
        s.tracks[1].sends.insert(0, FakeSend { mute: true, ..FakeSend::to(3) });
        s.state_count += 1;
    });
    let undo_anyway = json!({"kind": "send-mute", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"],
        "desc": "Kumi: undo (kund2)", "discard": true});
    assert!(lab.call("change_restore", undo_anyway).is_err());
    assert_eq!(send_volumes(), [(1.0, true), (1.0, false)], "nothing written");
}

#[test]
fn drawing_automation_on_a_send_kumi_changed_doesnt_block_its_undo() {
    let lab = Lab::new();
    let (first, _) = two_sends();
    let (preview, applied) = lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send “Kick” → “Reverb” muted (ksi32)");
    // The producer draws on the send's volume envelope, and on the other send's.
    state(|s| {
        s.tracks[1].envelopes.push(fake::FakeEnvelope::for_send("Send Volume", 0, "<VOLENV", 3));
        s.tracks[1].envelopes.push(fake::FakeEnvelope::for_send("Send Volume", 1, "<VOLENV", 5));
        s.state_count += 1;
    });
    let before = lab.undo("send-mute", &preview, &applied, "Kumi: undo (kund1)");
    assert!(before.is_ok() || before.as_ref().unwrap_err().0 == "changed", "{before:?}");
    // Points added to envelopes already there: still Kumi's send, and its undo goes through.
    let lab = Lab::new();
    let (first, _) = two_sends();
    state(|s| {
        s.tracks[1].envelopes.push(fake::FakeEnvelope::for_send("Send Volume", 0, "<VOLENV", 2));
        s.tracks[1].envelopes.push(fake::FakeEnvelope::for_send("Send Volume", 1, "<VOLENV", 2));
        s.state_count += 1;
    });
    let first = send_refs(1).into_iter().find(|r| r.contains("1of2")).unwrap_or(first);
    let (preview, applied) = lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send “Kick” → “Reverb” muted (ksi33)");
    state(|s| {
        for envelope in &mut s.tracks[1].envelopes {
            envelope.points += 4;
        }
        s.state_count += 1;
    });
    lab.undo("send-mute", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert!(send_volumes().iter().all(|(_, mute)| !mute));
}

#[test]
fn kumis_undo_of_a_send_change_leaves_the_producers_sends_when_the_pair_changed() {
    let lab = Lab::new();
    let (first, _) = two_sends();
    let (preview, applied) = lab.make("send-mute", json!({"ref": first, "mute": true}), "Kumi: Send “Kick” → “Reverb” muted (ksi01)");
    let kumis = state(|s| s.tracks[1].sends[0].ext.clone());
    // The producer mutes the second send too, removes the first and adds a new one.
    state(|s| s.tracks[1].sends[1].mute = true);
    producer_swaps_the_first_send();
    let before = send_volumes();
    let (code, _) = lab.undo("send-mute", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert_eq!(send_volumes(), before, "the producer's mute of the other send stays");
    // The sends as Kumi left them: the same undo goes through.
    state(|s| {
        s.tracks[1].sends.pop();
        s.tracks[1].sends.insert(0, {
            let mut send = FakeSend::to(3);
            send.volume = 0.25;
            send.mute = true;
            send.ext = kumis.clone();
            send
        });
        s.tracks[1].sends[1].mute = false;
    });
    lab.undo("send-mute", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(send_volumes(), [(0.25, false), (0.5, false)]);
}

#[test]
fn kumis_undo_of_a_send_it_added_finds_that_send_by_its_mark_and_never_the_producers() {
    let lab = Lab::new();
    state(|s| s.tracks[1].sends.clear());
    let input = json!({"ref": KICK, "toRef": "r:track:{DDDD}"});
    let (preview, applied) = lab.make("send-add", input, "Kumi: Added send “Kick” → “Reverb” (ksi02)");
    let kumis_stamp = applied["created"]["stamp"].as_str().unwrap().to_string();
    // The producer deletes it and makes their own send to the same track.
    state(|s| {
        s.tracks[1].sends.clear();
        s.tracks[1].sends.push(FakeSend::to(3));
        s.state_count += 1;
    });
    let (code, message) = lab.undo("send-add", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "gone", "{message}: Kumi's send is not there, and the producer's is not Kumi's to take away");
    assert_eq!(state(|s| s.tracks[1].sends.len()), 1);
    // The producer puts another send in front of Kumi's: the mark finds Kumi's, whatever its place.
    state(|s| {
        s.tracks[1].sends.clear();
        let mut mine = FakeSend::to(3);
        mine.ext.insert("P_EXT:kumi_id".into(), kumis_stamp.clone());
        s.tracks[1].sends = vec![FakeSend::to(3), mine];
        s.state_count += 1;
    });
    lab.undo("send-add", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(state(|s| s.tracks[1].sends.len()), 1);
    assert!(state(|s| s.tracks[1].sends[0].ext.is_empty()), "the one left is the producer's");
}

#[test]
fn kumis_mark_is_taken_only_on_a_send_to_the_track_the_ref_names_and_two_sends_with_it_are_refused() {
    let lab = Lab::new();
    state(|s| s.tracks[1].sends.clear());
    let input = json!({"ref": KICK, "toRef": "r:track:{DDDD}"});
    let (preview, applied) = lab.make("send-add", input, "Kumi: Added send “Kick” → “Reverb” (ksi06)");
    let kumis_stamp = applied["created"]["stamp"].as_str().unwrap().to_string();
    // The producer duplicates Reverb: the copy receives from Kick with the same mark, and its send comes first.
    state(|s| {
        s.tracks.push(fake::FakeTrack::new("{EEEE}", "Reverb copy"));
        let mut copy = FakeSend::to(4);
        copy.ext.insert("P_EXT:kumi_id".into(), kumis_stamp.clone());
        s.tracks[1].sends.insert(0, copy);
        s.state_count += 1;
    });
    let restored = lab.undo("send-add", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["display"]["title"], "Send “Kick” → “Reverb” taken away again");
    assert_eq!(state(|s| s.tracks[1].sends.iter().map(|send| send.dest).collect::<Vec<_>>()), [4], "the copy's send stays");

    // Two sends to Reverb with Kumi's mark: which is Kumi's can't be told, and neither is taken away.
    let lab = Lab::new();
    state(|s| s.tracks[1].sends.clear());
    let input = json!({"ref": KICK, "toRef": "r:track:{DDDD}"});
    let (preview, applied) = lab.make("send-add", input, "Kumi: Added send “Kick” → “Reverb” (ksi07)");
    state(|s| {
        let twin = FakeSend { ext: s.tracks[1].sends[0].ext.clone(), ..FakeSend::to(3) };
        s.tracks[1].sends.push(twin);
        s.state_count += 1;
    });
    let (code, message) = lab.undo("send-add", &preview, &applied, "Kumi: undo (kund2)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("can't tell which"), "{message}");
    assert_eq!(state(|s| s.tracks[1].sends.len()), 2, "nothing was taken away");
}

#[test]
fn a_send_kumi_removed_is_made_again_even_when_an_identical_one_is_left_and_keeps_its_mark() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].sends = vec![FakeSend::to(3), FakeSend::to(3)];
        s.state_count += 1;
    });
    let (preview, applied) = lab.make("send-remove", json!({"ref": send_refs(1)[0]}), "Kumi: Removed send “Kick” → “Reverb” (ksi03)");
    assert_eq!(state(|s| s.tracks[1].sends.len()), 1);
    lab.undo("send-remove", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(state(|s| s.tracks[1].sends.len()), 2, "the one that was left is not taken for the one removed");
    // A retry makes no third.
    lab.undo("send-remove", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(state(|s| s.tracks[1].sends.len()), 2);
    // A send with a mark comes back with it.
    state(|s| {
        s.tracks[1].sends = vec![{
            let mut send = FakeSend::to(3);
            send.ext.insert("P_EXT:kumi_id".into(), "abc".into());
            send
        }];
        s.state_count += 1;
    });
    let (preview, applied) =
        lab.make("send-remove", json!({"ref": "r:send:{BBBB}:0:{DDDD}"}), "Kumi: Removed send “Kick” → “Reverb” (ksi04)");
    assert!(state(|s| s.tracks[1].sends.is_empty()));
    lab.undo("send-remove", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    assert_eq!(state(|s| s.tracks[1].sends[0].ext.get("P_EXT:kumi_id").cloned()), Some("abc".to_string()));
}

#[test]
fn a_deleted_tracks_sends_come_back_with_their_marks() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[0].sends[0].ext.insert("P_EXT:kumi_id".into(), "def".into());
    });
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{AAAA}"}), "Kumi: Deleted track “Drums” (ksi05)");
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    let marks: Vec<_> =
        state(|s| s.tracks.iter().flat_map(|t| t.sends.iter().filter_map(|send| send.ext.get("P_EXT:kumi_id").cloned())).collect());
    assert_eq!(marks, ["def"]);
}

// ----- the undo of an add leaves what the producer put on it (M3) -----

#[test]
fn undoing_an_added_track_is_refused_when_the_producer_drew_automation_on_it_or_made_it_a_folder() {
    let lab = Lab::new();
    // Automation: a volume envelope with points.
    let (preview, applied) = lab.make("track-add", json!({"name": "Pad", "index": 2}), "Kumi: Added track “Pad” (kad01)");
    state(|s| {
        s.tracks[1].envelopes.push(fake::FakeEnvelope::own("Volume", "<VOLENV2", 24));
        s.state_count += 1;
    });
    let entries = state(|s| s.tracks.len());
    let (code, message) = lab.undo("track-add", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("automation"), "{message}");
    assert_eq!(state(|s| s.tracks.len()), entries);
    state(|s| s.tracks[1].envelopes.clear());
    // A folder: the track opens one that holds the next track.
    state(|s| {
        s.tracks[1].folder = 1;
        s.tracks[2].folder = -1;
        s.state_count += 1;
    });
    let (code, message) = lab.undo("track-add", &preview, &applied, "Kumi: undo (kund2)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("folder"), "{message}");
    assert_eq!(state(|s| s.tracks.len()), entries);
    // Told to discard, it goes.
    let mut params = json!({"kind": "track-add", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo (kund3)", "discard": true});
    params["epoch"] = lab.reads.epoch().into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(state(|s| s.tracks.len()), entries - 1);
}

#[test]
fn undoing_an_added_fx_is_refused_when_the_producer_automated_one_of_its_parameters() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added “ReaEQ” to “Kick” (kad02)");
    let guid = applied["applied"]["guid"].as_str().unwrap().to_string();
    state(|s| {
        s.tracks[1].envelopes.push(fake::FakeEnvelope::for_param("ReaEQ: Gain", &guid, 0, 16));
        s.state_count += 1;
    });
    let (code, message) = lab.undo("fx-add", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("automation"), "{message}");
    assert_eq!(kick_fx().len(), 1);
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund2)");
    params["kind"] = "fx-add".into();
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    assert!(kick_fx().is_empty());
}

// ----- folders around a deleted track, big tracks, a folder moved (M5, S2, S7) -----

/// Replaces the lab's tracks with these (name, folder depth), ids {T-name}, and no sends.
fn folder_tracks(spec: &[(&str, i32)]) {
    state(|s| {
        s.tracks = spec
            .iter()
            .map(|(name, depth)| {
                let mut track = fake::FakeTrack::new(&format!("{{T-{name}}}"), name);
                track.folder = *depth;
                track
            })
            .collect();
        s.state_count += 1;
    });
}

fn track_depths() -> Vec<(String, i32)> {
    state(|s| s.tracks.iter().map(|t| (t.name.clone(), t.folder)).collect())
}

fn depths_are(expected: &[(&str, i32)]) -> bool {
    track_depths() == expected.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>()
}

#[test]
fn undoing_a_delete_writes_no_folder_depth_the_deletion_didnt_change() {
    let lab = Lab::new();
    folder_tracks(&[("Group", 1), ("A", 0), ("B", 0), ("C", -1), ("D", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-B}"}), "Kumi: Deleted track “B” (kdp01)");
    assert_eq!(applied["material"]["depths"], json!([]), "deleting a middle track changes no depth");
    // The producer drags D into the folder: C no longer closes it, D does.
    state(|s| {
        s.tracks[2].folder = 0;
        s.tracks[3].folder = -1;
    });
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert!(depths_are(&[("Group", 1), ("A", 0), ("B", 0), ("C", 0), ("D", -1)]), "{:?}", track_depths());
}

#[test]
fn a_folder_that_was_mended_by_a_delete_is_mended_back_unless_the_producer_rearranged_it() {
    let lab = Lab::new();
    folder_tracks(&[("G", 1), ("H", 1), ("A", 0), ("B", -2), ("D", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-H}"}), "Kumi: Deleted track “H” (kdp02)");
    // REAPER made B close one folder fewer, and Kumi kept what it made of it.
    assert_eq!(applied["material"]["depths"], json!([{"guid": "{T-B}", "before": -2, "after": -1}]));
    assert!(depths_are(&[("G", 1), ("A", 0), ("B", -1), ("D", 0)]), "{:?}", track_depths());
    // The producer made B close nothing and D close the folder: B is not as the deletion left it.
    state(|s| {
        s.tracks[2].folder = 0;
        s.tracks[3].folder = -1;
    });
    let (code, message) = lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("rearranged"), "{message}");
    assert_eq!(state(|s| s.tracks.len()), 4, "nothing was made");
    // As the deletion left it: B is put back to -2 and H is a folder again.
    state(|s| {
        s.tracks[2].folder = -1;
        s.tracks[3].folder = 0;
    });
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert!(depths_are(&[("G", 1), ("H", 1), ("A", 0), ("B", -2), ("D", 0)]), "{:?}", track_depths());
}

#[test]
fn a_delete_whose_folders_the_producer_rearranged_is_put_back_when_told_to_discard() {
    let lab = Lab::new();
    folder_tracks(&[("G", 1), ("H", 1), ("A", 0), ("B", -2), ("D", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-H}"}), "Kumi: Deleted track “H” (kdp03)");
    state(|s| s.tracks[2].folder = 0);
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund1)");
    params["kind"] = "track-delete".into();
    params["material"] = applied["material"].clone();
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    assert!(depths_are(&[("G", 1), ("H", 1), ("A", 0), ("B", -2), ("D", 0)]), "{:?}", track_depths());
}

/// The producer drags track `from` to place `to` (places counted once it is taken out).
fn drag(from: usize, to: usize) {
    state(|s| {
        let track = s.tracks.remove(from);
        s.tracks.insert(to, track);
        s.state_count += 1;
    });
}

#[test]
fn a_deleted_folders_last_track_goes_back_after_the_track_above_it_when_the_producer_dragged_another_in_before() {
    let lab = Lab::new();
    folder_tracks(&[("Group", 1), ("A", 0), ("B", -1), ("C", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-B}"}), "Kumi: Deleted track “B” (kdp20)");
    assert!(depths_are(&[("Group", 1), ("A", -1), ("C", 0)]), "{:?}", track_depths());
    // The producer drags C into the folder, above A.
    drag(2, 1);
    state(|s| {
        s.tracks[1].folder = 0;
        s.tracks[2].folder = -1;
    });
    assert!(depths_are(&[("Group", 1), ("C", 0), ("A", -1)]), "{:?}", track_depths());
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert!(depths_are(&[("Group", 1), ("C", 0), ("A", 0), ("B", -1)]), "A stays in the folder, B closes it: {:?}", track_depths());
}

#[test]
fn a_deleted_track_goes_back_beside_its_old_neighbours_wherever_the_producer_moved_them() {
    // After the track that was above it.
    let lab = Lab::new();
    folder_tracks(&[("X", 0), ("A", 0), ("B", 0), ("C", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-B}"}), "Kumi: Deleted track “B” (kdp21)");
    drag(1, 2);
    assert_eq!(names(), ["X", "C", "A"]);
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(names(), ["X", "C", "A", "B"]);

    // Before the track that was below it, when the one above is gone.
    let lab = Lab::new();
    folder_tracks(&[("X", 0), ("A", 0), ("B", 0), ("C", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-B}"}), "Kumi: Deleted track “B” (kdp22)");
    state(|s| {
        s.tracks.remove(1);
        s.state_count += 1;
    });
    drag(1, 0);
    assert_eq!(names(), ["C", "X"]);
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(names(), ["B", "C", "X"]);

    // The first track goes back before the one that was below it.
    let lab = Lab::new();
    folder_tracks(&[("A", 0), ("B", 0), ("C", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-A}"}), "Kumi: Deleted track “A” (kdp23)");
    drag(0, 1);
    assert_eq!(names(), ["C", "B"]);
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(names(), ["C", "A", "B"]);

    // Its old place, when both are gone.
    let lab = Lab::new();
    folder_tracks(&[("X", 0), ("A", 0), ("B", 0), ("C", 0), ("D", 0)]);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{T-B}"}), "Kumi: Deleted track “B” (kdp24)");
    state(|s| {
        s.tracks.retain(|t| t.name != "A" && t.name != "C");
        s.state_count += 1;
    });
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(names(), ["X", "D", "B"], "its old place (third) is past the end now: it goes last");
}

// ----- folders over random layouts (N-1, N-2) -----

/// A small random number generator, so the layouts of the tests are the same each time.
struct Dice(u64);

impl Dice {
    fn new(seed: u64) -> Dice {
        Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) % n as u64) as usize
    }
}

/// A layout of 2 to 10 tracks, "t0", "t1"…: each opens a folder, closes one or two, or is plain.
fn random_layout(dice: &mut Dice) -> Vec<(String, i32)> {
    (0..2 + dice.below(9))
        .map(|at| {
            let depth = match dice.below(10) {
                0..=2 => 1,
                3..=6 => 0,
                7..=8 => -1,
                _ => -2,
            };
            (format!("t{at}"), depth)
        })
        .collect()
}

/// Each track's folder as the depths say, by name: the innermost one open when it comes.
fn folders_of(layout: &[(String, i32)]) -> std::collections::BTreeMap<String, Option<String>> {
    let mut open: Vec<&str> = Vec::new();
    let mut parents = std::collections::BTreeMap::new();
    for (name, depth) in layout {
        parents.insert(name.clone(), open.last().map(|parent| parent.to_string()));
        if *depth > 0 {
            open.push(name);
        }
        for _ in 0..(-*depth).max(0) {
            open.pop();
        }
    }
    parents
}

fn spec_of(layout: &[(String, i32)]) -> Vec<(&str, i32)> {
    layout.iter().map(|(name, depth)| (name.as_str(), *depth)).collect()
}

fn lab_layout() -> Vec<(String, i32)> {
    track_depths()
}

#[test]
fn a_delete_leaves_every_other_track_in_its_folder_but_for_the_tracks_of_a_folder_that_goes() {
    for seed in 0..400 {
        let mut dice = Dice::new(seed);
        let layout = random_layout(&mut dice);
        let gone = dice.below(layout.len());
        let lab = Lab::new();
        folder_tracks(&spec_of(&layout));
        let before = folders_of(&layout);
        lab.make("track-delete", json!({"ref": format!("r:track:{{T-t{gone}}}")}), "Kumi: Deleted track (kdp10)");
        let after = folders_of(&lab_layout());
        let name = format!("t{gone}");
        for (track, parent) in &after {
            let was = match &before[track] {
                Some(parent) if *parent == name => before[&name].clone(),
                other => other.clone(),
            };
            assert_eq!(*parent, was, "seed {seed}: {layout:?} without {name}: {track} is in {parent:?}, it was in {:?}", before[track]);
        }
        assert_eq!(after.len(), layout.len() - 1);
    }
}

#[test]
fn a_delete_and_its_undo_with_nothing_between_give_every_depth_back() {
    for seed in 0..400 {
        let mut dice = Dice::new(seed + 1000);
        let layout = random_layout(&mut dice);
        let gone = dice.below(layout.len());
        let lab = Lab::new();
        folder_tracks(&spec_of(&layout));
        let (preview, applied) = lab.make("track-delete", json!({"ref": format!("r:track:{{T-t{gone}}}")}), "Kumi: Deleted track (kdp11)");
        lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)")
            .unwrap_or_else(|e| panic!("seed {seed}: {layout:?} without t{gone}: {e:?}"));
        assert_eq!(lab_layout(), layout, "seed {seed}: t{gone} was deleted and put back");
    }
}

#[test]
fn a_delete_whose_folders_the_producer_refolded_is_refused_or_changes_no_other_tracks_folder() {
    let (mut put_back, mut refused) = (0, 0);
    for seed in 0..600 {
        let mut dice = Dice::new(seed + 5000);
        let layout = random_layout(&mut dice);
        let gone = dice.below(layout.len());
        let name = format!("t{gone}");
        let lab = Lab::new();
        folder_tracks(&spec_of(&layout));
        let children: Vec<String> = folders_of(&layout)
            .into_iter()
            .filter(|(_, parent)| parent.as_deref() == Some(name.as_str()))
            .map(|(track, _)| track)
            .collect();
        let (preview, applied) = lab.make("track-delete", json!({"ref": format!("r:track:{{T-t{gone}}}")}), "Kumi: Deleted track (kdp12)");
        // The producer folds again: a few of the tracks left take another depth.
        state(|s| {
            for _ in 0..1 + dice.below(3) {
                let at = dice.below(s.tracks.len());
                s.tracks[at].folder = [-2, -1, 0, 0, 1][dice.below(5)];
            }
        });
        let now = lab_layout();
        let result = lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)");
        match result {
            Err((code, _)) => {
                assert_eq!(code, "changed", "seed {seed}");
                assert_eq!(lab_layout(), now, "seed {seed}: nothing was written");
                refused += 1;
            }
            Ok(_) => {
                put_back += 1;
                let (before, after) = (folders_of(&now), folders_of(&lab_layout()));
                for (track, parent) in &before {
                    let after = &after[track];
                    let adopted = children.contains(track) && after.as_deref() == Some(name.as_str());
                    assert!(
                        after == parent || adopted,
                        "seed {seed}: {layout:?} without {name}, refolded to {now:?}: {track} was in {parent:?} and is in {after:?}"
                    );
                }
            }
        }
    }
    assert!(put_back > 20 && refused > 20, "put back {put_back}, refused {refused}");
}

#[test]
fn a_track_too_big_to_keep_is_not_deleted() {
    let lab = Lab::new();
    // A name of 25 MiB: the track's chunk is as big.
    state(|s| s.tracks[1].name = "x".repeat(25 << 20));
    let entries = state(|s| s.undo_entries);
    let preview = lab.call("change_preview", json!({"kind": "track-delete", "input": {"ref": KICK}})).unwrap();
    let apply = json!({"kind": "track-delete", "input": {"ref": KICK}, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    let (code, message) = lab.call("change_apply", apply).unwrap_err();
    assert_eq!(code, "failed");
    assert!(message.contains("won't delete what it can't put back"), "{}", &message[..message.len().min(300)]);
    assert_eq!((state(|s| s.tracks.len()), state(|s| s.undo_entries)), (4, entries));
}

#[test]
fn a_folder_is_not_moved_alone() {
    let lab = Lab::new();
    // Drums is a folder of Kick and Snare.
    let (code, message) =
        lab.call("change_preview", json!({"kind": "track-move", "input": {"ref": "r:track:{AAAA}", "index": 4}})).unwrap_err();
    assert_eq!(code, "bad-params");
    assert!(message.contains("folder"), "{message}");
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
}

#[test]
fn the_index_a_read_gives_is_the_index_a_change_takes() {
    let lab = Lab::new();
    let api = fake::api();
    let read = |kind: &str, extra: Value| {
        let mut params = json!({"kind": kind, "epoch": lab.reads.epoch()});
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        lab.reads.read(&Reaper::new(&api), &params).unwrap()
    };
    let guids = three_fx(&lab);
    let fx = read("fx", json!({"parent": KICK}));
    assert_eq!(fx["items"][2]["ref"], fx_ref_of(&guids[2]));
    lab.make("fx-move", json!({"ref": fx_ref_of(&guids[2]), "index": 0}), "Kumi: Moved “ReaDelay” to place 1 on “Kick” (kix03)");
    assert_eq!(kick_fx()[0].0, guids[2], "FX index 0 is the first place");
    // Tracks are counted from 1 in a read, and a change takes the place it shows.
    let tracks = read("track", json!({}));
    assert_eq!(tracks["indexFrom"], 1);
    let rows: Vec<(String, i64)> = tracks["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap().to_string(), r["index"].as_i64().unwrap()))
        .collect();
    // (The master comes first, as 0; the tracks are 1, 2, 3.)
    let named: Vec<(&str, i64)> = rows.iter().map(|(n, i)| (n.as_str(), *i)).collect();
    assert_eq!(named, [("MASTER", 0), ("Drums", 1), ("Kick", 2), ("Snare", 3), ("Reverb", 4)]);
    lab.make("track-move", json!({"ref": KICK, "index": 3}), "Kumi: Moved track “Kick” to position 3 (kix01)");
    assert_eq!(names(), ["Drums", "Snare", "Kick", "Reverb"], "Kick took the place its neighbour's row said");
    lab.make("track-add", json!({"name": "Pad", "index": 1}), "Kumi: Added track “Pad” (kix02)");
    assert_eq!(names()[0], "Pad", "index 1 is the first place");
    // FX, parameters and notes are counted from 0 in a read, and in a change.
    for kind in ["fx", "param", "note"] {
        let parent = match kind {
            "fx" => json!({"parent": "r:track:{AAAA}"}),
            "param" => json!({"parent": "r:fx:{FX-EQ}"}),
            _ => json!({"parent": SNARE_TAKE}),
        };
        let page = read(kind, parent);
        assert_eq!(page["indexFrom"], 0, "{kind}");
        assert_eq!(page["items"][0]["index"], 0, "{kind}");
    }
}

// ----- an undo after a look at another tab and back (S3) -----

#[test]
fn an_undo_asked_in_an_earlier_epoch_of_the_same_project_goes_through_and_another_projects_does_not() {
    let lab = Lab::new();
    let api = fake::api();
    let proj = Reaper::new(&api).current_project().unwrap().ptr;
    lab.reads.note_project(proj);
    let first = lab.reads.epoch();
    let (preview, applied) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), "Kumi: Kick volume −6 dB (ktb01)");
    // The producer looks at another tab and comes back: the epoch moved twice, to the same project.
    for _ in 0..2 {
        lab.reads.bump_epoch();
        lab.reads.note_project(proj);
    }
    assert_ne!(lab.reads.epoch(), first);
    let mut params = json!({"kind": "track-volume", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": "Kumi: undo (kund1)"});
    params["epoch"] = first.clone().into();
    // A change is not asked in a past epoch.
    let apply = json!({"kind": "track-volume", "input": {"ref": KICK, "volumeDb": -3.0}, "fence": preview["fence"], "desc": "Kumi: x (k1111)", "epoch": first});
    assert_eq!(lab.call("change_apply", apply).unwrap_err().0, "stale");
    // An undo of it is: the id it names is the same.
    let restored = lab.call("change_restore", params.clone()).unwrap();
    assert_eq!(restored["epoch"], lab.reads.epoch().as_str(), "the answer carries the epoch now");
    assert_eq!(state(|s| s.tracks[1].volume), 1.0);
    // A different project took the tab: what was done in the project before is another project's.
    let (preview, applied) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -9.0}), "Kumi: Kick volume −9 dB (ktb02)");
    let before = lab.reads.epoch();
    lab.reads.project_replaced(proj);
    lab.reads.bump_epoch();
    lab.reads.note_project(proj);
    let mut params = json!({"kind": "track-volume", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": "Kumi: undo (kund2)"});
    params["epoch"] = before.into();
    assert_eq!(lab.call("change_restore", params).unwrap_err().0, "stale");
    // An epoch that was never seen is not any project's.
    let mut unknown = json!({"kind": "track-volume", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": "Kumi: undo (kund3)"});
    unknown["epoch"] = "never-1".into();
    assert_eq!(lab.call("change_restore", unknown).unwrap_err().0, "stale");
}

// ----- a write that goes wrong is taken back, or said to be unknown (S4); notes cost and undo overlaps (S5, S6) -----

#[test]
fn a_name_that_stands_for_several_plug_ins_is_refused_before_anything_is_added() {
    let lab = Lab::new();
    for name in ["FXADD:ReaEQ", "fxadd:", "  FXADD:x"] {
        let (code, message) = lab.call("change_preview", json!({"kind": "fx-add", "input": {"ref": KICK, "name": name}})).unwrap_err();
        assert_eq!(code, "bad-params", "{name}");
        assert!(message.contains("one plug-in"), "{message}");
    }
    assert!(kick_fx().is_empty());
}

#[test]
fn an_add_that_leaves_more_than_it_should_is_taken_back_inside_the_same_call() {
    let lab = Lab::new();
    // A name that adds two plug-ins where one was asked for.
    let input = json!({"ref": KICK, "name": "ReaEQ twice"});
    let preview = lab.call("change_preview", json!({"kind": "fx-add", "input": input})).unwrap();
    let apply = json!({"kind": "fx-add", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    let (code, message) = lab.call("change_apply", apply.clone()).unwrap_err();
    assert_eq!(code, "failed");
    assert!(message.contains("taken back"), "{message}");
    assert!(kick_fx().is_empty(), "nothing of it is left");
    // Taken back inside a block that wrote nothing in the end: no entry the producer would find.
    assert_eq!(state(|s| s.undo_depth), 0);
    // If the plug-ins can't be taken off again, nobody knows what the track holds.
    state(|s| s.fail_fx_delete = true);
    let (code, message) = lab.call("change_apply", apply).unwrap_err();
    assert_eq!(code, "uncertain");
    assert!(message.contains("could not take back"), "{message}");
    assert_eq!(kick_fx().len(), 2);
}

#[test]
fn a_notes_write_that_goes_wrong_is_taken_back_or_called_uncertain() {
    let lab = Lab::new();
    let input =
        json!({"ref": SNARE_TAKE, "notes": [{"pitch": 60, "start": 4.0, "length": 1.0}, {"pitch": 61, "start": 5.0, "length": 1.0}]});
    let before = snare_notes();
    let preview = lab.call("change_preview", json!({"kind": "notes-add", "input": input})).unwrap();
    let apply = json!({"kind": "notes-add", "input": input, "fence": preview["fence"], "desc": "Kumi: x (k1111)"});
    // The write drops a note; putting the take back works.
    state(|s| s.glitch_events = 1);
    let (code, message) = lab.call("change_apply", apply.clone()).unwrap_err();
    assert_eq!(code, "failed");
    assert!(message.contains("taken back"), "{message}");
    assert_eq!(snare_notes(), before);
    // The write drops one and so does putting back.
    state(|s| s.glitch_events = 2);
    let (code, _) = lab.call("change_apply", apply.clone()).unwrap_err();
    assert_eq!(code, "uncertain");
    // And an undo that goes wrong the same way.
    let (preview, applied) = lab.make("notes-add", input, "Kumi: Wrote 2 notes (kno01)");
    let written = snare_notes();
    state(|s| s.glitch_events = 1);
    let (code, _) = lab.undo("notes-add", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "failed");
    assert_eq!(snare_notes(), written);
}

/// The notes of a take as `read` gives them.
fn read_notes_of(lab: &Lab, parent: &str) -> Vec<Value> {
    let api = fake::api();
    let answer = lab.reads.read(&Reaper::new(&api), &json!({"kind": "note", "parent": parent})).unwrap();
    answer["items"].as_array().unwrap().clone()
}

#[test]
fn a_note_is_read_with_the_length_a_change_writes_back_when_two_notes_of_a_pitch_overlap() {
    let lab = Lab::new();
    let note = |start_ppq, end_ppq, pitch, velocity| fake::FakeNote {
        start_ppq,
        end_ppq,
        channel: 0,
        pitch,
        velocity,
        selected: false,
        muted: false,
    };
    // The producer's take: two notes of one pitch and channel that overlap (REAPER does not correct a take it loaded), and
    // one note beside them. REAPER's own `MIDI_GetNote` ends both overlapping notes at the first note-off.
    state(|s| {
        s.tracks[2].items[0].takes[0].notes = vec![note(0.0, 480.0, 72, 84), note(0.0, 1440.0, 72, 85), note(1000.0, 1480.0, 60, 90)]
    });
    let rows = read_notes_of(&lab, SNARE_TAKE);
    assert_eq!(rows.iter().map(|r| r["length"].as_f64().unwrap()).collect::<Vec<_>>(), [0.5, 1.5, 0.5]);
    // What a change would write back for each note is what was read.
    for (i, row) in rows.iter().enumerate() {
        let preview = lab.call("change_preview", json!({"kind": "notes-delete", "input": {"ref": SNARE_TAKE, "indexes": [i]}})).unwrap();
        assert_eq!(preview["display"]["before"][0], *row, "note {i}");
    }
    // A change beside them writes the whole take back; the two keep the lengths that were read.
    let (preview, applied) = lab.make(
        "notes-change",
        json!({"ref": SNARE_TAKE, "changes": [{"index": 2, "velocity": 33}]}),
        "Kumi: Changed 1 note in snare.mid take (kov10)",
    );
    let after = read_notes_of(&lab, SNARE_TAKE);
    assert_eq!((&after[0], &after[1]), (&rows[0], &rows[1]));
    assert_eq!(after[2]["velocity"], 33);
    assert_eq!(state(|s| s.tracks[2].items[0].takes[0].notes.iter().map(|n| n.end_ppq).collect::<Vec<_>>()), [480.0, 1440.0, 1480.0]);
    lab.undo("notes-change", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(read_notes_of(&lab, SNARE_TAKE), rows);
}

#[test]
fn undoing_a_notes_delete_is_refused_when_the_producer_wrote_a_note_over_one_it_would_put_back() {
    let lab = Lab::new();
    // The snare take has pitch 38 on channel 1 from tick 0 to 480.
    let (preview, applied) = lab.make("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [0]}), "Kumi: Deleted 1 note (kno02)");
    state(|s| {
        s.tracks[2].items[0].takes[0].notes.insert(
            0,
            fake::FakeNote { start_ppq: 100.0, end_ppq: 600.0, channel: 0, pitch: 38, velocity: 70, selected: false, muted: false },
        );
    });
    let notes = snare_notes();
    let (code, message) = lab.undo("notes-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("overlap"), "{message}");
    assert_eq!(snare_notes(), notes, "nothing was written");
    // Told to discard, it is put back.
    let mut params = restore_params(&preview, &applied, "Kumi: undo (kund2)");
    params["kind"] = "notes-delete".into();
    params["discard"] = true.into();
    lab.call("change_restore", params).unwrap();
    assert_eq!(snare_notes().len(), 3);
}

#[test]
fn a_notes_change_in_a_big_take_costs_no_more_calls_than_in_a_small_one() {
    let lab = Lab::new();
    let fill = |count: i32| {
        state(|s| {
            s.tracks[2].items[0].takes[0].notes = (0..count)
                .map(|n| fake::FakeNote {
                    start_ppq: n as f64 * 960.0,
                    end_ppq: n as f64 * 960.0 + 480.0,
                    channel: 0,
                    pitch: 40 + n % 40,
                    velocity: 90,
                    selected: false,
                    muted: false,
                })
                .collect();
        })
    };
    let notes: Vec<Value> = (0..100).map(|i| json!({"pitch": 100 + i % 20, "start": 200_000.0 + i as f64, "length": 0.5})).collect();
    let mut costs = vec![];
    for (n, count) in [100, 20_000, 100_000].into_iter().enumerate() {
        fill(count);
        let started = Instant::now();
        let calls = state(|s| s.calls);
        let (preview, applied) =
            lab.make("notes-add", json!({"ref": SNARE_TAKE, "notes": notes}), &format!("Kumi: Wrote 100 notes (kno0{n})"));
        let apply_calls = state(|s| s.calls) - calls;
        let calls = state(|s| s.calls);
        lab.undo("notes-add", &preview, &applied, "Kumi: undo (kund1)").unwrap();
        let undo_calls = state(|s| s.calls) - calls;
        assert_eq!(snare_notes().len(), count as usize);
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "{count} notes: {:?}", started.elapsed());
        costs.push((apply_calls, undo_calls));
    }
    // (The calls a note costs are the conversions of the 100 notes it writes; the take's size adds none.)
    assert_eq!(costs[0], costs[1], "{costs:?}");
    assert_eq!(costs[1], costs[2], "{costs:?}");
}

// ----- a restore that writes nothing makes no point (N1) -----

#[test]
fn a_restore_or_apply_that_writes_nothing_closes_without_a_point_so_it_commits_nothing_of_the_producers() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), "Kumi: Kick volume −6 dB (kn101)");
    assert_eq!(applied["undo"]["entry"], true);
    let restored = lab.undo("track-volume", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["undo"]["entry"], true, "an undo that wrote is a point");
    // The producer has bare edits that REAPER hasn't made a point of yet; the same undo is asked again (a retry).
    state(|s| s.undo_dirty = true);
    let entries = state(|s| s.undo_entries);
    let again = lab.undo("track-volume", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(again["undo"]["entry"], false, "nothing was written, so there is no point");
    assert_eq!(state(|s| s.undo_entries), entries);
    assert!(state(|s| s.undo_dirty), "the producer's pending edits are not committed under Kumi's name");
    // The same for an apply that finds it so already.
    let (_, applied) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": 0.0}), "Kumi: Kick volume 0 dB (kn102)");
    assert_eq!(applied["undo"]["entry"], false);
    assert_eq!(state(|s| s.undo_entries), entries);
    assert!(state(|s| s.undo_dirty));
    // And the other kinds that can find it so: a send, an FX parameter, a note retry, a tempo, the loop.
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added “ReaEQ” to “Kick” (kn103)");
    lab.undo("fx-add", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    let entries = state(|s| s.undo_entries);
    let (preview, applied) = lab.make("notes-delete", json!({"ref": SNARE_TAKE, "indexes": [1]}), "Kumi: Deleted 1 note (kn104)");
    lab.undo("notes-delete", &preview, &applied, "Kumi: undo (kund4)").unwrap();
    let entries_after = state(|s| s.undo_entries);
    assert_eq!(entries_after, entries + 2);
    let retry = lab.undo("notes-delete", &preview, &applied, "Kumi: undo (kund5)").unwrap();
    assert_eq!(retry["undo"]["entry"], false);
    assert_eq!(state(|s| s.undo_entries), entries_after);
}

// ----- the undo watch with a long history, and an undo with an edit in one look (N2) -----

#[test]
fn an_undo_is_still_seen_when_hundreds_of_points_are_redoable() {
    let lab = Lab::new();
    let desc = "Kumi: Kick volume −6 dB (kn201)";
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), desc);
    for n in 0..300 {
        fake::producer_edit(&format!("Edit {n}"));
    }
    lab.tick();
    lab.events.take();
    // The producer steps back over 300 of their own edits: none is Kumi's.
    for _ in 0..300 {
        fake::producer_undo();
    }
    lab.tick();
    assert!(lab.events.take().is_empty());
    // And then over Kumi's: more than 256 points can be redone now, which a look at the first 256 of them can't tell apart.
    fake::producer_undo();
    lab.tick();
    assert_eq!(lab.events.take(), [("undone_in_daw".to_string(), json!({"descs": [desc], "how": "undo", "epoch": lab.reads.epoch()}))]);
    fake::producer_redo();
    lab.tick();
    assert_eq!(lab.events.take(), [("undone_in_daw".to_string(), json!({"descs": [desc], "how": "redo", "epoch": lab.reads.epoch()}))]);
}

#[test]
fn reaper_dropping_its_oldest_points_when_its_history_is_full_is_no_undo_of_kumis_change() {
    let lab = Lab::new();
    for n in 0..600 {
        fake::producer_edit(&format!("Edit {n}"));
    }
    let desc = "Kumi: Kick volume −6 dB (kn203)";
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), desc);
    for n in 0..60 {
        fake::producer_edit(&format!("Edit after {n}"));
    }
    lab.tick();
    lab.events.take();
    // The producer's next edit fills the history: REAPER drops its 246 oldest points at once.
    fake::producer_edit("Adjust track pan");
    fake::trim_history(246);
    lab.tick();
    assert!(lab.events.take().is_empty(), "Kumi's change is still applied, 61 points down");
    // Undone after all: told.
    for _ in 0..62 {
        fake::producer_undo();
    }
    lab.tick();
    assert_eq!(lab.events.take(), [("undone_in_daw".to_string(), json!({"descs": [desc], "how": "undo", "epoch": lab.reads.epoch()}))]);
}

#[test]
fn reaper_merging_the_oldest_points_of_a_short_history_is_no_undo_of_kumis_change_in_them() {
    let lab = Lab::new();
    for n in 0..10 {
        fake::producer_edit(&format!("Edit {n}"));
    }
    let desc = "Kumi: Kick volume −6 dB (kn204)";
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), desc);
    for n in 0..80 {
        fake::producer_edit(&format!("Edit after {n}"));
    }
    lab.tick();
    lab.events.take();
    // 94 points; REAPER's undo memory is full: the 31 oldest, Kumi's among them, go into its first point.
    fake::producer_edit("Adjust track pan");
    fake::trim_history(31);
    lab.tick();
    assert!(lab.events.take().is_empty(), "Kumi's change is still applied, merged into REAPER's first point");
}

#[test]
fn an_undo_and_a_new_edit_within_one_look_tell_nothing_and_kumis_undo_still_checks_the_values() {
    let lab = Lab::new();
    let desc = "Kumi: Kick volume −6 dB (kn202)";
    let (preview, applied) = lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), desc);
    lab.tick();
    lab.events.take();
    // The same number of points, with the project at the same place, and Kumi's gone: it vanished, which is evidence of
    // nothing (a known limit). The row stays Applied; Kumi's undo finds the fader back already and writes nothing.
    fake::producer_undo();
    // (the fake's undo moves the history only: the fader goes back as REAPER's undo would put it)
    state(|s| s.tracks[1].volume = 1.0);
    fake::producer_edit("Adjust track pan");
    lab.tick();
    assert!(lab.events.take().is_empty());
    let restored = lab.undo("track-volume", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(restored["display"]["note"], "It was back already: nothing was written.", "{restored}");
    // Bare edits that make no point, and the producer's own, say nothing.
    state(|s| s.state_count += 1);
    lab.tick();
    fake::producer_edit("Adjust track mute");
    lab.tick();
    assert!(lab.events.take().is_empty());
}

#[test]
fn a_track_put_back_does_not_join_the_selection_the_producer_has_made_since() {
    let lab = Lab::new();
    state(|s| s.tracks[3].selected = true);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{DDDD}"}), "Kumi: Deleted track “Reverb” (kn301)");
    // The producer selects Kick; the track comes back without being selected beside it.
    state(|s| s.tracks[1].selected = true);
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(state(|s| s.tracks.iter().map(|t| t.selected).collect::<Vec<_>>()), [false, true, false, false]);
    // With nothing selected, it is the selection again, as it was.
    state(|s| s.tracks[3].selected = true);
    let (preview, applied) = lab.make("track-delete", json!({"ref": "r:track:{DDDD}"}), "Kumi: Deleted track “Reverb” (kn302)");
    state(|s| s.tracks.iter_mut().for_each(|t| t.selected = false));
    lab.undo("track-delete", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(state(|s| s.tracks.iter().map(|t| t.selected).collect::<Vec<_>>()), [false, false, false, true]);
}

// ----- a marker made again (N3) -----

#[test]
fn a_marker_made_again_is_made_once_keeps_its_lane_and_a_region_its_render_matrix() {
    let lab = Lab::new();
    state(|s| {
        s.markers[1].number = 7;
        s.marker_lanes.insert("{REG-1}".into(), 2);
        s.render_matrix.insert(7, vec!["{BBBB}".into(), "master".into()]);
    });
    let (preview, applied) = lab.make("marker-delete", json!({"ref": "r:region:{REG-1}"}), "Kumi: Deleted region “Verse” (kmk01)");
    assert_eq!(applied["material"]["matrix"], json!(["{BBBB}", "master"]));
    assert_eq!(applied["material"]["lane"], 2);
    assert_eq!(state(|s| s.markers.len()), 1);
    let restored = lab.undo("marker-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(state(|s| s.markers.len()), 2);
    let made = restored["restored"]["ref"].as_str().unwrap().trim_start_matches("r:region:").to_string();
    assert_eq!(state(|s| s.marker_lanes.get(&made).copied()), Some(2), "its ruler lane");
    assert_eq!(state(|s| s.render_matrix.get(&7).cloned()), Some(vec!["{BBBB}".to_string(), "master".to_string()]), "its render matrix");
    // The first answer was lost and the host asks again: the marker made is the one that comes back, and no second is made.
    let entries = state(|s| s.undo_entries);
    let retry = lab.undo("marker-delete", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert_eq!(state(|s| s.markers.len()), 2, "no second marker");
    assert_eq!(retry["restored"]["ref"], restored["restored"]["ref"]);
    assert_eq!(retry["undo"]["entry"], false);
    assert_eq!(state(|s| s.undo_entries), entries);
    // A marker (not a region) the same way.
    let (preview, applied) = lab.make("marker-delete", json!({"ref": "r:marker:{MARK-1}"}), "Kumi: Deleted marker “Drop” (kmk02)");
    lab.undo("marker-delete", &preview, &applied, "Kumi: undo (kund3)").unwrap();
    lab.undo("marker-delete", &preview, &applied, "Kumi: undo (kund4)").unwrap();
    assert_eq!(state(|s| s.markers.len()), 2);
}

// ----- a plug-in with a meter among its parameters (N5) -----

// The optimization audit's R3: a signature read every parameter four times (twice to see what moves by itself, once for
// the hash, once for the look kept). The hash and the look are made from the second of the two reads now.
#[test]
fn a_signature_reads_each_parameter_twice_not_four_times() {
    use crate::reaper::fake::{FakeFx, FakeParam};
    use crate::reaper::{Fx, FxOwner};
    let lab = Lab::new();
    state(|s| {
        let mut wide = FakeFx::new("{FX-WIDE}", "VST: Wide (Kumi)");
        wide.params = (0..300).map(|p| FakeParam::new(&format!("P{p}"), 0.5, "50")).collect();
        s.tracks[1].fx = vec![wide];
    });
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let proj = reaper.current_project().unwrap().ptr;
    let ctx = Ctx { reaper: &reaper, reads: &lab.reads, proj, unkept: std::cell::Cell::new(false) };
    let fx = Fx { owner: FxOwner::Track(reaper.track_at(proj, 1).unwrap()), index: 0 };
    // (the first signature of a plug-in also reads its parameters' names, once for the plug-in: the second is the measure)
    super::fx::signature(&ctx, fx);
    let before = state(|s| s.calls);
    let signature = super::fx::signature(&ctx, fx);
    let calls = state(|s| s.calls) - before;
    assert!(signature["hash"].is_string());
    assert!((600..700).contains(&calls), "{calls} REAPER calls for 300 parameters: two sweeps and the FX's own fields, not four");
    // The look kept is the one hashed: the signature taken again as recorded is the recorded one.
    assert_eq!(super::fx::signature_as(&ctx, fx, &signature), signature);
}

#[test]
fn a_meter_shown_as_a_parameter_does_not_make_every_undo_of_an_fx_changed_but_a_tweak_still_does() {
    let lab = Lab::new();
    // A plug-in made now has two meters among its parameters: one called a meter, one that only moves by itself.
    state(|s| s.meter_params = true);
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added “ReaEQ” to “Kick” (kn501)");
    assert_eq!(applied["applied"]["signature"]["volatile"], json!([4, 5]), "{}", applied["applied"]["signature"]);
    // A real tweak is seen with the meters about.
    state(|s| s.tracks[1].fx[0].params[0].value = 0.9);
    let (code, _) = lab.undo("fx-add", &preview, &applied, "Kumi: undo (kund1)").unwrap_err();
    assert_eq!(code, "changed");
    state(|s| s.tracks[1].fx[0].params[0].value = 0.5);
    lab.undo("fx-add", &preview, &applied, "Kumi: undo (kund2)").unwrap();
    assert!(kick_fx().is_empty(), "the meters didn't refuse it");
}

#[test]
fn deleting_an_fx_with_a_meter_in_it_is_not_refused_for_the_meter() {
    let lab = Lab::new();
    state(|s| s.meter_params = true);
    lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added “ReaEQ” to “Kick” (kn502)");
    let fx = fx_ref_of(&kick_fx()[0].0);
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx}), "Kumi: Deleted “ReaEQ” from “Kick” (kn503)");
    assert_eq!(preview["fence"]["signature"]["volatile"], json!([4, 5]));
    assert!(kick_fx().is_empty());
    let dir = fx_folder();
    lab.undo("fx-delete", &preview, &applied, "Kumi: undo (kund1)").unwrap();
    assert_eq!(kick_fx().len(), 1);
    drop(dir);
}

// ----- the escape hatches: a REAPER API function by name, and a Lua chunk (M4) -----

fn call_read(lab: &Lab, params: Value) -> Answer {
    let api = fake::api();
    let reaper = Reaper::new(&api);
    lab.reads.read(&reaper, &params)
}

#[test]
fn a_function_that_only_reads_is_called_by_name_with_refs_in_and_out() {
    let lab = Lab::new();
    let name = call_read(&lab, json!({"kind": "call", "fn": "GetTrackName", "args": [KICK]})).unwrap();
    assert_eq!((&name["result"], &name["out"]["bufOut"]), (&json!(true), &json!("Kick")));
    let named = call_read(&lab, json!({"kind": "call", "fn": "GetTrackName", "args": {"track": KICK}})).unwrap();
    assert_eq!(named["out"]["bufOut"], "Kick", "by name as well as in order");
    let volume = call_read(&lab, json!({"kind": "call", "fn": "GetMediaTrackInfo_Value", "args": [KICK, "D_VOL"]})).unwrap();
    assert!(volume["result"].is_number());
    let count = call_read(&lab, json!({"kind": "call", "fn": "CountTracks"})).unwrap();
    assert_eq!(count["result"], 4, "the project, optional, is the open one");
    let second = call_read(&lab, json!({"kind": "call", "fn": "GetTrack", "args": [0, 1]})).unwrap();
    assert_eq!(second["result"], KICK, "a track REAPER gives back is its ref");
    // A GetSet… function asked to get only reads.
    let text = call_read(&lab, json!({"kind": "call", "fn": "GetSetMediaTrackInfo_String", "args": [KICK, "P_NAME", "", false]})).unwrap();
    assert_eq!(text["out"]["stringNeedBig"], "Kick");
}

#[test]
fn a_call_by_name_that_changes_denied_or_unknown_is_refused_in_a_read() {
    let lab = Lab::new();
    let refused = |params: Value| call_read(&lab, params).unwrap_err();
    let (code, message) = refused(json!({"kind": "call", "fn": "SetMediaTrackInfo_Value", "args": [KICK, "D_VOL", 0.5]}));
    assert_eq!(code, "bad-params");
    assert!(message.contains("api-call"), "{message}");
    let (_, message) = refused(json!({"kind": "call", "fn": "GetSetMediaTrackInfo_String", "args": [KICK, "P_NAME", "x", true]}));
    assert!(message.contains("api-call"), "a GetSet… asked to set changes the project: {message}");
    assert!(refused(json!({"kind": "call", "fn": "Main_OnCommand", "args": [40001, 0]})).1.contains("action method"));
    assert!(refused(json!({"kind": "call", "fn": "GetUserInputs", "args": []})).1.contains("dialog"));
    assert!(refused(json!({"kind": "call", "fn": "GetTrackNam"})).1.contains("perhaps GetTrackName"));
    assert!(refused(json!({"kind": "call", "fn": "GetTrackName", "args": ["r:track:{NOPE}"]})).1.contains("track"));
    assert!(refused(json!({"kind": "call", "fn": "GetTrackName", "args": []})).1.contains("track must be a ref"));
    // A function with a parameter the call can't make: run-lua can call it.
    let uncallable = crate::reaper::generic::function("LICE_Clear").unwrap();
    assert!(crate::reaper::generic::uncallable(uncallable).is_some());
}

#[test]
fn every_function_of_the_header_is_callable_by_name_or_says_why_not() {
    let callable = (0..crate::api::FUNCTION_COUNT).filter(|&i| crate::reaper::generic::uncallable(i).is_none()).count();
    assert!(callable > 600, "only {callable} of {} functions can be called by name", crate::api::FUNCTION_COUNT);
    for index in 0..crate::api::FUNCTION_COUNT {
        if let Some(why) = crate::reaper::generic::uncallable(index) {
            assert!(!why.is_empty(), "{}", crate::reaper::generic::name_of(index));
        }
    }
}

#[test]
fn a_function_that_changes_the_project_is_a_change_reapers_undo_takes_back_and_kumis_refuses() {
    let lab = Lab::new();
    let input = json!({"fn": "SetMediaTrackInfo_Value", "args": [KICK, "D_VOL", 0.5]});
    let (preview, applied) = lab.make("api-call", input, "Kumi: Called SetMediaTrackInfo_Value (kc001)");
    assert_eq!(preview["display"]["title"], "Called SetMediaTrackInfo_Value");
    assert!(preview["display"]["note"].as_str().unwrap().contains("REAPER's own undo"));
    assert_eq!(state(|s| s.tracks[1].volume), 0.5);
    assert_eq!(applied["applied"]["result"], true);
    assert_eq!(applied["undo"]["entry"], true, "one point, named by the host");
    assert_eq!(top().as_deref(), Some("Kumi: Called SetMediaTrackInfo_Value (kc001)"));
    let (code, message) = lab.undo("api-call", &preview, &applied, "Kumi: undo (kc002)").unwrap_err();
    assert_eq!(code, "daw-only");
    assert!(message.contains("REAPER's own undo"), "{message}");
    assert_eq!(state(|s| s.tracks[1].volume), 0.5, "nothing written");
    // Refused at the preview, nothing written: one that only reads, a bad ref, a missing argument, a denied one.
    let preview_of = |input: Value| lab.call("change_preview", json!({"kind": "api-call", "input": input})).unwrap_err();
    assert!(preview_of(json!({"fn": "GetTrackName", "args": [KICK]})).1.contains("only reads"));
    assert!(preview_of(json!({"fn": "SetMediaTrackInfo_Value", "args": ["r:track:{NOPE}", "D_VOL", 0.1]})).1.contains("track"));
    assert!(preview_of(json!({"fn": "SetMediaTrackInfo_Value", "args": [KICK, "D_VOL"]})).1.contains("newvalue"));
    assert!(preview_of(json!({"fn": "Main_OnCommandEx", "args": [40001, 0, 0]})).1.contains("action method"));
    assert_eq!(state(|s| s.tracks[1].volume), 0.5);
}

// A stand-in for REAPER's Lua: what the wrapper would do, from the wrapper's text.
fn token_of(script: &str) -> String {
    let start = script.find("local TOKEN, LIMIT_MS = \"").unwrap() + "local TOKEN, LIMIT_MS = \"".len();
    script[start..].split('"').next().unwrap().to_string()
}

fn answer(script: &str, text: &str) {
    let token = token_of(script);
    state(|s| s.ext_state.insert(("kumi_run".into(), token), text.into()));
}

fn lua_ok(script: &str) {
    // (what `reaper.SetMediaTrackInfo_Value(track, "D_VOL", 0.25); print("hi"); return 42, track` would do)
    state(|s| {
        s.tracks[1].volume = 0.25;
        s.undo_dirty = true;
    });
    answer(script, r#"{"compiled":true,"ok":true,"result":[42,"r:track:{BBBB}"],"printed":["hi"]}"#);
}

fn lua_syntax(script: &str) {
    answer(script, r#"{"compiled":false,"error":{"message":"unexpected symbol near 'x'","line":2}}"#);
}

fn lua_fails(script: &str) {
    state(|s| {
        s.tracks[1].volume = 0.125;
        s.undo_dirty = true;
    });
    answer(script, r#"{"compiled":true,"ok":false,"error":{"message":"attempt to call a nil value","line":3},"printed":[]}"#);
}

fn lua_silent(_script: &str) {}

#[test]
fn a_lua_chunk_runs_once_in_one_undo_point_and_only_reapers_undo_takes_it_back() {
    let lab = Lab::new();
    state(|s| s.on_script = Some(lua_ok));
    let code = "-- louder\nlocal track = reaper.GetTrack(0, 1)\nreaper.SetMediaTrackInfo_Value(track, \"D_VOL\", 0.25)\nprint(\"hi\")\nreturn 42, track";
    let (preview, applied) = lab.make("run-lua", json!({"code": code}), "Kumi: Ran Lua (kl001)");
    assert_eq!(preview["display"]["title"], "Ran Lua: local track = reaper.GetTrack(0, 1)");
    assert_eq!(state(|s| s.tracks[1].volume), 0.25, "run once");
    assert_eq!(applied["applied"]["result"], json!([42, KICK]));
    assert_eq!(applied["applied"]["printed"], json!(["hi"]));
    assert_eq!((&applied["undo"]["entry"], top().as_deref()), (&json!(true), Some("Kumi: Ran Lua (kl001)")));
    assert!(state(|s| s.scripts.is_empty() && s.ext_state.is_empty()), "the script is taken away, and its answer");
    let (code, message) = lab.undo("run-lua", &preview, &applied, "Kumi: undo (kl002)").unwrap_err();
    assert_eq!(code, "daw-only");
    assert!(message.contains("Lua chunk"), "{message}");
}

#[test]
fn a_lua_chunks_script_is_a_new_owner_only_file_with_an_unguessable_name_in_the_bridges_folder() {
    // The shared temporary folder and a counted name let another local user make the file first, or a link at it (review
    // R12): the bridge's own folder, a random name, made new.
    let lab = Lab::new();
    let dir = tempfile::tempdir().unwrap();
    let scripts = dir.path().join("bridge").join("scripts");
    crate::reaper::generic::set_scripts_dir(Some(scripts.clone()));
    state(|s| s.on_script = Some(lua_ok));
    lab.make("run-lua", json!({"code": "return 1"}), "Kumi: Ran Lua (kr001)");
    lab.make("run-lua", json!({"code": "return 1"}), "Kumi: Ran Lua (kr002)");
    crate::reaper::generic::set_scripts_dir(None);
    let files = state(|s| s.script_files.clone());
    assert_eq!(files.len(), 2);
    for (path, mode) in &files {
        assert!(std::path::Path::new(path).starts_with(&scripts), "{path}");
        assert!(!std::path::Path::new(path).exists(), "taken away after the run: {path}");
        #[cfg(unix)]
        assert_eq!(*mode, 0o600, "{path}");
        let _ = mode;
    }
    let name = |path: &str| std::path::Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
    assert_ne!(name(&files[0].0), name(&files[1].0));
    assert!(!name(&files[0].0).contains(&std::process::id().to_string()), "not named by the process");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&scripts).unwrap().permissions().mode() & 0o777, 0o700);
    }
}

#[test]
fn a_hundred_lua_chunks_leave_no_action_behind_and_never_write_reapers_key_map() {
    let lab = Lab::new();
    state(|s| s.on_script = Some(lua_ok));
    for n in 0..100 {
        lab.make("run-lua", json!({"code": "return 1"}), &format!("Kumi: Ran Lua (k{n:04})"));
    }
    assert_eq!(state(|s| (s.kb_writes, s.scripts.len())), (0, 0), "reaper-kb.ini untouched, and no script left registered");
}

#[test]
fn a_lua_chunks_preview_runs_nothing_in_reaper() {
    // A script run moves REAPER's state count, and right after REAPER's undo it can wipe the redo list (review R11).
    let lab = Lab::new();
    state(|s| s.on_script = Some(lua_ok));
    let before = state(|s| (s.actions.len(), s.state_count, s.undo_entries));
    lab.call("change_preview", json!({"kind": "run-lua", "input": {"code": "return 1"}})).unwrap();
    assert_eq!(state(|s| (s.actions.len(), s.state_count, s.undo_entries)), before, "no script ran");
}

#[test]
fn a_lua_chunk_that_doesnt_compile_is_refused_at_the_apply_and_one_that_fails_says_where_and_what_stays() {
    let lab = Lab::new();
    state(|s| s.on_script = Some(lua_syntax));
    let input = json!({"code": "local x =\nx x"});
    let preview = lab.call("change_preview", json!({"kind": "run-lua", "input": input})).unwrap();
    let (code, message) = lab
        .call("change_apply", json!({"kind": "run-lua", "input": input, "fence": preview["fence"], "desc": "Kumi: Ran Lua (kl005)"}))
        .unwrap_err();
    assert_eq!(code, "bad-params");
    assert!(message.contains("line 2") && message.contains("unexpected symbol"), "{message}");
    assert_ne!(top().as_deref(), Some("Kumi: Ran Lua (kl005)"), "nothing ran: no point");
    state(|s| s.on_script = Some(lua_fails));
    let (_, applied) = lab.make(
        "run-lua",
        json!({"code": "reaper.SetMediaTrackInfo_Value(reaper.GetTrack(0, 1), \"D_VOL\", 0.125)\nlocal f\nf()"}),
        "Kumi: Ran Lua (kl003)",
    );
    assert_eq!(applied["applied"]["failed"]["line"], 3);
    let note = applied["display"]["note"].as_str().unwrap();
    assert!(note.contains("line 3") && note.contains("stays"), "{note}");
    assert_eq!(state(|s| s.tracks[1].volume), 0.125);
    assert_eq!(top().as_deref(), Some("Kumi: Ran Lua (kl003)"), "what it did before the error is one point REAPER's undo takes back");
}

fn lua_fails_after_printing_a_lot(script: &str) {
    let printed = "x".repeat(5 << 20);
    answer(script, &format!(r#"{{"compiled":true,"ok":false,"error":{{"message":"boom","line":3}},"printed":["{printed}"]}}"#));
}

#[test]
fn a_lua_chunk_that_fails_after_printing_more_than_kumi_keeps_is_still_said_to_have_failed() {
    // An answer past 4 MB was read as a clean run whatever its head said.
    let lab = Lab::new();
    state(|s| s.on_script = Some(lua_fails_after_printing_a_lot));
    let (_, applied) = lab.make("run-lua", json!({"code": "for i = 1, 1e6 do print(i) end\nlocal f\nf()"}), "Kumi: Ran Lua (kl006)");
    assert_eq!(applied["applied"]["failed"], json!({"message": "boom", "line": 3}), "{applied}");
    assert!(applied["applied"].get("result").is_none(), "{applied}");
    let note = applied["display"]["note"].as_str().unwrap();
    assert!(note.contains("line 3: boom"), "{note}");
    assert!(applied["applied"]["printed"][0].as_str().unwrap().contains("more than Kumi keeps"), "{applied}");
}

#[test]
fn a_lua_chunk_that_gives_no_answer_is_uncertain_and_limits_are_checked() {
    let lab = Lab::new();
    state(|s| s.on_script = Some(lua_silent));
    let preview = lab.call("change_preview", json!({"kind": "run-lua", "input": {"code": "return 1"}})).unwrap();
    let (code, _) = lab
        .call(
            "change_apply",
            json!({"kind": "run-lua", "input": {"code": "return 1"}, "fence": preview["fence"], "desc": "Kumi: Ran Lua (kl004)"}),
        )
        .unwrap_err();
    assert_eq!(code, "uncertain");
    let refused = |input: Value| lab.call("change_preview", json!({"kind": "run-lua", "input": input})).unwrap_err().1;
    assert!(refused(json!({"code": "  "})).contains("code"));
    assert!(refused(json!({"code": "return 1", "timeoutMs": 60_000})).contains("timeoutMs"));
}

/// The wrapper run by LuaJIT against a stand-in `reaper` table (`more` adds to it): its answer, and what the stand-in wrote
/// to stderr. None when LuaJIT isn't installed.
fn luajit_wrapper(dir: &std::path::Path, more: &str, code: &str, limit: u64) -> Option<(Value, String)> {
    if std::process::Command::new("luajit").arg("-v").output().is_err() {
        eprintln!("luajit isn't installed: the wrapper's own test is skipped");
        return None;
    }
    // (LuaJIT calls no hook inside code it compiled: REAPER's Lua compiles nothing, so neither does this)
    let stub = format!(
        "if jit then jit.off() end\nreaper = {{ SetExtState = function(_, _, v) io.write(v) end, time_precise = os.clock, \
         ValidatePtr2 = function() return false end, GetMasterTrack = function() end, ShowConsoleMsg = function() end, \
         MB = function() return 1 end {more} }}\n"
    );
    let path = dir.join("w.lua");
    std::fs::write(&path, format!("{stub}{}", super::lua::wrapper(code, "t1", limit))).unwrap();
    // (a wrapper whose limit doesn't hold would run on: it's stopped after 20 s, and the test fails)
    let mut child = std::process::Command::new("luajit")
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // (read as it writes: an answer past the pipe's 64 KiB would otherwise wait for a reader that waits for it to end)
    let drain = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    };
    let (stdout, stderr) = (drain(Box::new(child.stdout.take().unwrap())), drain(Box::new(child.stderr.take().unwrap())));
    let started = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > std::time::Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the wrapper ran past 20 s: {code}");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    let answer = serde_json::from_slice(&stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&stdout)));
    Some((answer, String::from_utf8_lossy(&stderr).into_owned()))
}

/// The wrapper itself, run by LuaJIT where it is installed, against a stand-in `reaper` table.
#[test]
fn the_lua_wrapper_answers_results_prints_errors_and_the_time_limit() {
    let dir = tempfile::tempdir().unwrap();
    if luajit_wrapper(dir.path(), "", "return 1", 1000).is_none() {
        return;
    }
    let run = |code: &str, limit: u64| -> Value { luajit_wrapper(dir.path(), "", code, limit).unwrap().0 };
    let ran = run("print('a', 2)\nreaper.ShowConsoleMsg('b')\nreturn {x = 1.5, y = {1, 2, 'q\"'}}, nil, true", 1000);
    assert_eq!(ran["ok"], true);
    assert_eq!(ran["printed"], json!(["a\t2", "b"]));
    assert_eq!(ran["result"], json!([{"x": 1.5, "y": [1, 2, "q\""]}, null, true]));
    let bad = run("local x =\nx x", 1000);
    assert_eq!((&bad["compiled"], &bad["error"]["line"]), (&json!(false), &json!(2)));
    let failed = run("local a = 1\nerror('boom')", 1000);
    assert_eq!((&failed["ok"], &failed["error"]["line"], &failed["error"]["message"]), (&json!(false), &json!(2), &json!("boom")));
    let looped = run("while true do end", 50);
    assert!(looped["error"]["message"].as_str().unwrap().contains("stopped after 50 ms"), "{looped}");
    // The time limit holds against a chunk that catches errors, a coroutine and an answer like `_G` or one of many shared
    // tables (review R2: a pcall loop ran 3 s past a 50 ms limit, and `return _G` was still encoding after 10 s).
    let timed = |code: &str| {
        let started = std::time::Instant::now();
        let ran = run(code, 50);
        (ran, started.elapsed())
    };
    for code in [
        "while true do pcall(function() for i = 1, 1e9 do end end) end",
        "while true do pcall(error, 'x') end",
        "coroutine.wrap(function() while true do end end)()",
        "local co = coroutine.create(function() while true do end end)\ncoroutine.resume(co)\nwhile true do end",
        "debug.sethook()\nwhile true do end",
    ] {
        let (ran, took) = timed(code);
        assert!(
            ran["error"]["message"].as_str().is_some_and(|m| m.contains("stopped after 50 ms") || m.contains("time limit")),
            "{code}: {ran}"
        );
        assert!(took < std::time::Duration::from_secs(2), "{code} took {took:?}");
    }
    let (globals, took) = timed("return _G");
    assert!(globals["ok"] == true && took < std::time::Duration::from_secs(3), "{took:?}");
    let (shared, took) = timed("local t = {}\nfor i = 1, 60 do t = {a = t, b = t} end\nreturn t");
    assert!(shared["ok"] == true && took < std::time::Duration::from_secs(2), "{took:?}");
    assert!(shared["result"].to_string().contains("(seen above)"), "{shared}");
    // What a chunk prints is kept up to 2,000 lines and 1 MB, and says how much more there was (review R17).
    let chatty = run("for i = 1, 100000 do print(i) end\nprint(string.rep('x', 2000000))", 5000);
    let printed = chatty["printed"].as_array().unwrap();
    assert_eq!((printed.len(), printed[0].as_str(), printed[1999].as_str()), (2001, Some("1"), Some("2000")));
    assert_eq!(printed[2000], "(98001 more printed lines not kept)");
    // A dialog would hold REAPER until someone answers it: refused.
    let dialog = run("reaper.MB('done', 'Kumi', 0)", 1000);
    assert!(dialog["error"]["message"].as_str().unwrap().contains("opens a dialog"), "{dialog}");
    // A chunk that closes the wrapper's own long bracket is still one string, and so is one that ends in `]` or `]=`.
    assert_eq!(run("return ']]'", 1000)["result"], "]]");
    assert_eq!(run("local t = {5}\nreturn t[1]", 1000)["result"], 5);
    assert_eq!(run("local t = {5}\nreturn t[#t]", 1000)["result"], 5);
    assert_eq!(run("local s = 'a]='\nreturn s", 1000)["result"], "a]=");
}

#[test]
fn the_lua_time_limit_holds_for_code_the_chunk_loads_and_against_a_chunk_that_replaces_what_the_wrapper_calls() {
    // The limit raised only in code whose source was the chunk's own: code it loaded (`load`, `dofile`, `require`) ran on
    // and held REAPER. A chunk that replaced `error` or the clock stopped the limit too.
    let dir = tempfile::tempdir().unwrap();
    if luajit_wrapper(dir.path(), "", "return 1", 1000).is_none() {
        return;
    }
    let looping = dir.path().join("loop.lua");
    std::fs::write(&looping, "while true do end").unwrap();
    for code in [
        "load('while true do end')()".to_string(),
        "loadstring('while true do pcall(error, 1) end')()".to_string(),
        "local f = load('return function() while true do end end')()\nwhile true do pcall(f) end".to_string(),
        format!("dofile({:?})", looping.display().to_string()),
        "error = function() end\nwhile true do end".to_string(),
        "reaper.time_precise = function() return 0 end\nwhile true do end".to_string(),
        "string.gsub = function() while true do end end\nerror(string.rep('x', 10))".to_string(),
    ] {
        let started = std::time::Instant::now();
        let (ran, _) = luajit_wrapper(dir.path(), "", &code, 50).unwrap();
        let took = started.elapsed();
        assert_eq!(ran["ok"], false, "{code}: {ran}");
        assert!(took < std::time::Duration::from_secs(3), "{code} took {took:?}");
    }
    // An error that isn't text is named by its type: its __tostring would run the chunk's code with no limit.
    let (ran, _) = luajit_wrapper(dir.path(), "", "error(setmetatable({}, {__tostring = function() while true do end end}))", 50).unwrap();
    assert_eq!(ran["error"]["message"], "(a table raised as the error)");
    // A huge error's text is cut.
    let (ran, _) = luajit_wrapper(dir.path(), "", "error(string.rep('x', 1e6), 0)", 1000).unwrap();
    assert!(ran["error"]["message"].as_str().unwrap().len() < 70_000, "cut");
}

#[test]
fn a_lua_chunk_cant_hand_reaper_code_to_run_after_it() {
    // reaper.defer, runloop and atexit run code after the chunk: outside Kumi's undo block (bare edits, no HISTORY) and its
    // time limit (a loop there holds REAPER).
    let dir = tempfile::tempdir().unwrap();
    let later = ", defer = function() io.stderr:write('LATER ') end, runloop = function() io.stderr:write('LATER ') end, \
                 atexit = function() io.stderr:write('LATER ') end";
    if luajit_wrapper(dir.path(), later, "return 1", 1000).is_none() {
        return;
    }
    for name in ["defer", "runloop", "atexit"] {
        let (ran, told) = luajit_wrapper(dir.path(), later, &format!("reaper.{name}(function() while true do end end)"), 1000).unwrap();
        assert!(ran["error"]["message"].as_str().unwrap().contains("runs a chunk once"), "{name}: {ran}");
        assert!(!told.contains("LATER"), "{name}: {told}");
    }
}

#[test]
fn a_lua_chunk_leaves_no_undo_block_open_no_refresh_held_and_doesnt_move_reapers_history() {
    // The chunk runs inside Kumi's undo block: one it leaves open would swallow the producer's later edits, an end past its
    // own would close Kumi's, and REAPER's undo or redo would move the history under HISTORY (review R7).
    let dir = tempfile::tempdir().unwrap();
    let log = ", Undo_BeginBlock2 = function() io.stderr:write('begin ') end, Undo_EndBlock2 = function() io.stderr:write('end ') end, \
               PreventUIRefresh = function(n) io.stderr:write('refresh' .. n .. ' ') end, \
               Undo_DoUndo2 = function() io.stderr:write('UNDO ') end, Main_OnCommand = function(c) io.stderr:write('action' .. c .. ' ') end";
    let Some((left, told)) = luajit_wrapper(dir.path(), log, "reaper.Undo_BeginBlock2(0)\nreaper.PreventUIRefresh(1)\nerror('boom')", 1000)
    else {
        return;
    };
    assert_eq!(told.split_whitespace().collect::<Vec<_>>(), ["begin", "refresh1", "end", "refresh-1"], "closed and let go after it");
    assert_eq!((left["ok"].clone(), left["closedBlocks"].clone()), (json!(false), json!(1)));
    let (extra_end, told) = luajit_wrapper(dir.path(), log, "reaper.Undo_EndBlock2(0, 'x', -1)", 1000).unwrap();
    assert!(extra_end["error"]["message"].as_str().unwrap().contains("Kumi's undo block"), "{extra_end}");
    assert!(!told.contains("end"), "Kumi's own block stays open: {told}");
    let (undo, told) = luajit_wrapper(dir.path(), log, "reaper.Undo_DoUndo2(0)", 1000).unwrap();
    assert!(undo["error"]["message"].as_str().unwrap().contains("history"), "{undo}");
    let (action, told_action) = luajit_wrapper(dir.path(), log, "reaper.Main_OnCommand(40029, 0)", 1000).unwrap();
    assert!(action["error"]["message"].as_str().unwrap().contains("history"), "{action}");
    assert!(!told.contains("UNDO") && !told_action.contains("action40029"), "{told} {told_action}");
    // Its own begin and end, balanced, and other actions, run as asked.
    let (balanced, told) = luajit_wrapper(
        dir.path(),
        log,
        "reaper.Undo_BeginBlock2(0)\nreaper.Undo_EndBlock2(0, 'mine', -1)\nreaper.Main_OnCommand(40001, 0)",
        1000,
    )
    .unwrap();
    assert_eq!((balanced["ok"].clone(), balanced.get("closedBlocks")), (json!(true), None));
    assert_eq!(told.split_whitespace().collect::<Vec<_>>(), ["begin", "end", "action40001"]);
}

#[test]
fn the_generic_call_gives_reaper_a_buffer_as_big_as_it_tells_reaper_and_sends_back_what_reaper_wrote() {
    // A buffer whose size REAPER reads and writes back: it was 64 KiB whatever size REAPER was told, and REAPER overran its
    // own heap (review R1). The fake writes as much as it's told.
    let lab = Lab::new();
    let read = |args: Value| lab.call("read", json!({"kind": "call", "fn": "GetTrackMIDILyrics", "args": args}));
    // (REAPER is told 200,000 bytes and writes 199,999 of them, safely: the buffer is that big; the answer shows 96 KiB)
    let big = read(json!([KICK, 0, "", 200_000])).unwrap();
    assert_eq!(big["out"]["bufOutWantNeedBig_sz"], 199_999);
    assert_eq!(big["out"]["bufOutWantNeedBig"].as_str().map(str::len), Some(96 << 10));
    assert_eq!(big["out"]["bufOutWantNeedBig_cut"], true);
    // By name, a float that is whole, and the default.
    let named = read(json!({"track": KICK, "flag": 0, "bufOutWantNeedBig_sz": 1000})).unwrap();
    assert_eq!(named["out"]["bufOutWantNeedBig"].as_str().map(str::len), Some(999));
    assert_eq!(read(json!([KICK, 0, "", 2e3])).unwrap()["out"]["bufOutWantNeedBig_sz"], 1999);
    assert_eq!(read(json!([KICK, 0])).unwrap()["out"]["bufOutWantNeedBig_sz"], 64 * 1024 - 1);
    // Past 64 MiB, or not a whole size: refused before REAPER is called.
    assert!(read(json!([KICK, 0, "", 128 << 20])).unwrap_err().1.contains("at most"));
    assert!(read(json!([KICK, 0, "", 10.5])).unwrap_err().1.contains("whole number"));
    // What comes back is at most 96 KiB of text, within the host's 256 KiB frame, said cut (review R19).
    let huge = read(json!([KICK, 0, "", 3 << 20])).unwrap();
    assert_eq!(
        (huge["out"]["bufOutWantNeedBig"].as_str().map(str::len), huge["out"]["bufOutWantNeedBig_cut"].clone()),
        (Some(96 << 10), json!(true))
    );
    assert!(huge.to_string().len() < 200 << 10);
    // Bytes come back as hex: a take's MIDI, as many bytes as REAPER said it wrote.
    let midi = lab.call("read", json!({"kind": "call", "fn": "MIDI_GetAllEvts", "args": [SNARE_TAKE, "", 100_000]})).unwrap();
    let hex = midi["out"]["bufOutNeedBig"].as_str().unwrap();
    assert_eq!(
        (midi["out"]["bufOutNeedBig_hex"].clone(), hex.len() as i64),
        (json!(true), midi["out"]["bufOutNeedBig_sz"].as_i64().unwrap() * 2)
    );
    assert_eq!(midi["out"].get("bufOutNeedBig_cut"), None);
}

#[test]
fn a_buffer_reaper_isnt_told_the_size_of_holds_the_whole_text_whatever_the_caller_asks() {
    // `stringNeedBig` (the GetSet…Info_String family) and `strNeed64` (mkvolstr): REAPER writes the whole value, told no
    // size, and the buffer was made as small as the caller's `<name>_sz` asked, or 64 KiB. The fake writes a track's whole
    // P_EXT value, as REAPER does.
    let lab = Lab::new();
    let value = "x".repeat(50_000);
    state(|s| {
        let guid = s.tracks[1].guid.clone();
        s.listening.track_ext.insert((guid, "P_EXT:big".into()), value.clone());
    });
    let args = json!({"tr": KICK, "parmname": "P_EXT:big", "setNewValue": false, "stringNeedBig_sz": 16});
    let read = lab.call("read", json!({"kind": "call", "fn": "GetSetMediaTrackInfo_String", "args": args})).unwrap();
    assert_eq!(read["result"], true);
    assert_eq!(read["out"]["stringNeedBig"].as_str().map(str::len), Some(value.len()), "the whole value");
    assert_eq!(read["out"].get("stringNeedBig_cut"), None);
    // REAPER writes a text longer than 64 KiB whole as well (lab 7.82: a track's 200 KB P_EXT): it fits, and the answer
    // carries the first 96 KiB of it.
    state(|s| {
        let guid = s.tracks[1].guid.clone();
        s.listening.track_ext.insert((guid, "P_EXT:huge".into()), "y".repeat(200_000));
    });
    let args = json!({"tr": KICK, "parmname": "P_EXT:huge", "setNewValue": false});
    let huge = lab.call("read", json!({"kind": "call", "fn": "GetSetMediaTrackInfo_String", "args": args})).unwrap();
    assert_eq!((huge["out"]["stringNeedBig"].as_str().map(str::len), &huge["out"]["stringNeedBig_cut"]), (Some(96 << 10), &json!(true)));
    // A buffer REAPER is told the size of still takes the caller's size.
    let short = lab.call("read", json!({"kind": "call", "fn": "GetTrackName", "args": {"track": KICK, "bufOut_sz": 3}})).unwrap();
    assert_eq!((&short["out"]["bufOut"], &short["out"]["bufOut_cut"]), (&json!("Ki"), &json!(true)));
}

#[test]
fn the_bridges_own_reads_of_a_text_reaper_isnt_told_the_size_of_take_it_whole() {
    // The bridge read names, P_EXT values, razor edits and the like into 4 KiB; REAPER writes the whole text (lab 7.82), so
    // a longer one ran past it.
    let _lab = Lab::new();
    let value = "z".repeat(300_000);
    state(|s| {
        let guid = s.tracks[1].guid.clone();
        s.listening.track_ext.insert((guid, "P_EXT:long".into()), value.clone());
    });
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let track = reaper.track_at(reaper.current_project().unwrap().ptr, 1).unwrap();
    assert_eq!(reaper.track_string(track, "P_EXT:long").map(|text| text.len()), Some(value.len()));
    assert_eq!(reaper.track_string(track, "P_NAME").as_deref(), Some("Kick"), "and a short one after it");
}

#[test]
fn a_number_reaper_also_reads_as_a_pointer_cant_be_given_to_the_generic_call() {
    // GetTrackInfo's `track` is a track's index or, past them, a MediaTrack* cast to a number: 123456789 crashes REAPER.
    use crate::reaper::generic::{function, uncallable};
    let index = function("GetTrackInfo").unwrap();
    assert!(uncallable(index).is_some_and(|why| why.contains("pointer")), "{:?}", uncallable(index));
    let lab = Lab::new();
    let (_, message) = call_read(&lab, json!({"kind": "call", "fn": "GetTrackInfo", "args": [123_456_789]})).unwrap_err();
    assert!(message.contains("pointer"), "{message}");
}

#[test]
fn the_generic_time_selection_sets_the_start_and_end_it_is_given() {
    // GetSet_LoopTimeRange2's start and end are what it writes back when it gets, and what it reads when it sets: a set
    // took neither from the caller and made the time selection 0–0.
    let lab = Lab::new();
    state(|s| s.time_selection = (1.0, 3.0));
    let read = call_read(
        &lab,
        json!({"kind": "call", "fn": "GetSet_LoopTimeRange2", "args": {"isSet": false, "isLoop": false, "allowautoseek": false}}),
    )
    .unwrap();
    assert_eq!((&read["out"]["startOut"], &read["out"]["endOut"]), (&json!(1.0), &json!(3.0)));
    let set = json!({"fn": "GetSet_LoopTimeRange2", "args": {"isSet": true, "isLoop": false, "startOut": 2.0, "endOut": 4.5, "allowautoseek": false}});
    lab.make("api-call", set, "Kumi: Called GetSet_LoopTimeRange2 (kts01)");
    assert_eq!(state(|s| s.time_selection), (2.0, 4.5));
    // A set that leaves either out is refused before REAPER is called.
    let half = json!({"kind": "api-call", "input": {"fn": "GetSet_LoopTimeRange2", "args": {"isSet": true, "isLoop": false, "startOut": 1.0, "allowautoseek": false}}});
    let (_, message) = lab.call("change_preview", half).unwrap_err();
    assert!(message.contains("endOut"), "{message}");
    assert_eq!(state(|s| s.time_selection), (2.0, 4.5));
}

#[test]
fn calls_that_run_any_action_or_render_files_are_refused_by_name() {
    let lab = Lab::new();
    let read = |f: &str, args: Value| call_read(&lab, json!({"kind": "call", "fn": f, "args": args})).unwrap_err().1;
    let change =
        |f: &str, args: Value| lab.call("change_preview", json!({"kind": "api-call", "input": {"fn": f, "args": args}})).unwrap_err().1;
    assert!(change("OscLocalMessageToHost", json!(["/action/40860"])).contains("any action"));
    assert!(change("CSurf_OnOscControlMessage", json!(["/action", 40860.0])).contains("any action"));
    assert!(change("CSurf_OnOscControlMessage2", json!(["/action", 40860.0, ""])).contains("any action"));
    assert!(change("CrossfadeEditor_OnCommand", json!([40001])).contains("action"));
    assert!(change("RenderFileSection", json!(["/a.wav", "/b.wav", 0.0, 1.0, 1.0])).contains("renders"));
    // RENDER_STATS runs the action its value names before it answers: a "read" that renders.
    for args in [json!([0, "RENDER_STATS", "41824", false]), json!({"desc": "RENDER_STATS_SUMMARY", "valuestrNeedBig": "42437"})] {
        assert!(read("GetSetProjectInfo_String", args).contains("RENDER_STATS"));
    }
    let stats = call_read(&lab, json!({"kind": "call", "fn": "GetSetProjectInfo_String", "args": [0, "RENDER_STATS", "", false]}));
    assert!(stats.is_ok(), "an empty value only reads: {stats:?}");
}

#[test]
fn binary_data_with_its_size_and_arrays_reaper_fills_cant_be_passed_to_the_generic_call() {
    use crate::reaper::generic::{function, uncallable};
    for (name, why) in [
        ("MIDI_SetAllEvts", "binary data"),
        ("MIDI_InsertEvt", "binary data"),
        ("MIDI_InsertTextSysexEvt", "binary data"),
        ("MIDI_SetEvt", "binary data"),
        ("MIDI_SetTextSysexEvt", "binary data"),
        ("SendMIDIMessageToHardware", "binary data"),
        ("GetMediaItemTake_Peaks", "an array"),
    ] {
        let said = uncallable(function(name).unwrap()).unwrap_or_else(|| panic!("{name} is callable"));
        assert!(said.contains(why), "{name}: {said}");
    }
    let lab = Lab::new();
    let refused = lab.call("read", json!({"kind": "call", "fn": "GetMediaItemTake_Peaks", "args": [SNARE_TAKE, 100, 0, 2, 1000, 0]}));
    assert!(refused.unwrap_err().1.contains("run-lua"));
}

#[test]
fn a_getter_that_makes_what_it_gets_is_a_change_not_a_read() {
    // GetFXEnvelope(…, true) makes an envelope; reading container_map.add maps a macro: as reads they'd be bare edits, with
    // no undo point and no HISTORY row (review R6).
    use crate::reaper::generic::{function, reads_only};
    let fx_env = function("GetFXEnvelope").unwrap();
    assert!(reads_only(fx_env, &json!([KICK, 0, 3, false])));
    assert!(!reads_only(fx_env, &json!([KICK, 0, 3, true])));
    assert!(!reads_only(fx_env, &json!({"track": KICK, "fxindex": 0, "parameterindex": 3, "create": 1})));
    assert!(!reads_only(function("TakeFX_GetEnvelope").unwrap(), &json!(["r:take:{T}", 0, 3, true])));
    let named = function("TrackFX_GetNamedConfigParm").unwrap();
    assert!(reads_only(named, &json!([KICK, 0, "param.0.lfo.active"])));
    assert!(!reads_only(named, &json!([KICK, 0, "container_map.add.0.2"])));
    assert!(!reads_only(named, &json!([KICK, 0, "param.3.container_map.delete"])));
    assert!(!reads_only(function("TakeFX_GetNamedConfigParm").unwrap(), &json!(["r:take:{T}", 0, "container_map.add.0.0"])));
    // The read call refuses it, pointing to api-call.
    let lab = Lab::new();
    let refused = lab.call("read", json!({"kind": "call", "fn": "GetFXEnvelope", "args": [KICK, 0, 0, true]})).unwrap_err();
    assert!(refused.1.contains("api-call"), "{refused:?}");
}

#[test]
fn the_generic_call_refuses_what_would_unbalance_reapers_undo_blocks_or_ui() {
    let lab = Lab::new();
    for name in [
        "Undo_BeginBlock2",
        "Undo_EndBlock2",
        "Undo_DoUndo2",
        "Undo_DoRedo2",
        "Undo_OnStateChange",
        "Undo_OnStateChangeEx2",
        "PreventUIRefresh",
        "set_config_var_string",
        "MIDIEditor_LastFocused_OnCommand",
        "MIDIEditor_OnCommand",
    ] {
        let (code, why) = lab.call("change_preview", json!({"kind": "api-call", "input": {"fn": name, "args": []}})).unwrap_err();
        assert_eq!(code, "bad-params", "{name}");
        assert!(why.starts_with(name) && !why.contains("doesn't"), "{name}: {why}");
    }
}

// ----- sidechains: a send's channels, a track's channels, an FX's pins (S8) -----

const SNARE: &str = "r:track:{CCCC}";
const REVERB: &str = "r:track:{DDDD}";

#[test]
fn a_send_to_channels_the_track_doesnt_have_is_refused_until_it_has_them() {
    let lab = Lab::new();
    let input = json!({"ref": DRUMS_TO_REVERB, "dstChannels": {"offset": 3}});
    let (code, message) = lab.call("change_preview", json!({"kind": "send-channels", "input": input})).unwrap_err();
    assert_eq!(code, "bad-params");
    assert!(message.contains("“Reverb” has 2 channels") && message.contains("track_channels"), "{message}");
    // Odd channel counts aren't REAPER's.
    assert!(lab
        .call("change_preview", json!({"kind": "track-channels", "input": {"ref": REVERB, "channels": 3}}))
        .unwrap_err()
        .1
        .contains("even"));
    let (channels_preview, channels_applied) =
        lab.make("track-channels", json!({"ref": REVERB, "channels": 4}), "Kumi: Reverb 4 channels (ks801)");
    assert_eq!(channels_applied["display"]["title"], "Reverb 4 channels");
    let (preview, applied) = lab.make("send-channels", input, "Kumi: Send channels (ks802)");
    assert_eq!(preview["display"]["title"], "Send “Drums” → “Reverb” to channels 3/4");
    assert_eq!(state(|s| s.tracks[0].sends[0].dst_chan), 2, "destination channel 3 is REAPER's index 2");
    assert_eq!(applied["applied"]["dstChannels"], json!({"offset": 3, "mono": false}));
    // Undone in turn: the send's channels, then the track's.
    lab.undo("send-channels", &preview, &applied, "Kumi: undo (ks803)").unwrap();
    assert_eq!(state(|s| s.tracks[0].sends[0].dst_chan), 0);
    lab.undo("track-channels", &channels_preview, &channels_applied, "Kumi: undo (ks804)").unwrap();
    assert_eq!(state(|s| s.tracks[3].extra.get("I_NCHAN").copied()), Some(2.0));
}

#[test]
fn a_send_is_added_as_a_sidechain_with_its_channels_and_midi_and_undone_whole() {
    let lab = Lab::new();
    lab.make("track-channels", json!({"ref": SNARE, "channels": 4}), "Kumi: Snare 4 channels (ks805)");
    let input = json!({"ref": KICK, "toRef": SNARE, "srcChannels": {"offset": 1, "count": 2}, "dstChannels": {"offset": 3}, "midi": {"from": "none"}});
    let (preview, applied) = lab.make("send-add", input, "Kumi: Added send (ks806)");
    assert_eq!(preview["display"]["title"], "Added send “Kick” → “Snare”, channels 1/2 \u{2192} 3/4, no MIDI");
    let send =
        state(|s| s.tracks[1].sends.last().map(|send| (send.src_chan, send.dst_chan, send.extra.get("I_MIDIFLAGS").copied())).unwrap());
    assert_eq!(send, (0, 2, Some(31.0)));
    lab.undo("send-add", &preview, &applied, "Kumi: undo (ks807)").unwrap();
    assert!(state(|s| s.tracks[1].sends.iter().all(|send| send.dest != 2)));
    // A source past the sending track's channels, a mono count that isn't REAPER's, MIDI to channel 17: refused.
    let refused = |input: Value| lab.call("change_preview", json!({"kind": "send-add", "input": input})).unwrap_err().1;
    assert!(refused(json!({"ref": KICK, "toRef": SNARE, "srcChannels": {"offset": 3}})).contains("“Kick” has 2 channels"));
    assert!(refused(json!({"ref": KICK, "toRef": SNARE, "srcChannels": {"offset": 1, "count": 3}})).contains("count"));
    assert!(refused(json!({"ref": KICK, "toRef": SNARE, "midi": {"to": 17}})).contains("midi.to"));
}

#[test]
fn reapers_channel_numbers_read_back_as_words_and_mono_and_midi_round_trip() {
    let lab = Lab::new();
    let input = json!({"ref": DRUMS_TO_REVERB, "srcChannels": {"offset": 2, "count": 1}, "dstChannels": {"offset": 1, "mono": true}, "midi": {"from": 1, "to": 2}});
    let (_, applied) = lab.make("send-channels", input, "Kumi: Send channels (ks808)");
    assert_eq!(applied["applied"]["srcChannels"], json!({"offset": 2, "count": 1}));
    assert_eq!(applied["applied"]["dstChannels"], json!({"offset": 1, "mono": true}));
    assert_eq!(applied["applied"]["midi"], json!({"from": 1, "to": 2}));
    assert_eq!(state(|s| (s.tracks[0].sends[0].src_chan, s.tracks[0].sends[0].dst_chan)), (1 | 1024, 1024));
    assert_eq!(applied["display"]["title"], "Send “Drums” → “Reverb” channels 2 \u{2192} 1 (mono), MIDI 1 \u{2192} 2");
}

#[test]
fn an_fxs_pins_are_mapped_whole_put_back_and_refused_when_the_producer_remapped_them() {
    let lab = Lab::new();
    state(|s| s.tracks[3].fx[0].io = (4, 2));
    let input = json!({"ref": "r:fx:{FX-VERB}", "pins": [{"pin": 3, "channels": [3]}, {"pin": 4, "channels": [4]}, {"pin": 1, "output": true, "channels": [1, 3]}]});
    let (preview, applied) = lab.make("fx-pins", input, "Kumi: Pins (ks809)");
    assert_eq!(preview["display"]["title"], "Pins of “ReaVerb” on “Reverb”: in 3 \u{2190} 3, in 4 \u{2190} 4, out 1 \u{2192} 1+3");
    assert_eq!(state(|s| s.tracks[3].fx[0].pins.get(&(true, 0)).copied()), Some(0b101));
    lab.undo("fx-pins", &preview, &applied, "Kumi: undo (ks810)").unwrap();
    assert_eq!(state(|s| s.tracks[3].fx[0].pins.get(&(true, 0)).copied()), Some(1), "back to channel 1");
    // The producer maps a pin since: Kumi's undo leaves it.
    let (preview, applied) =
        lab.make("fx-pins", json!({"ref": "r:fx:{FX-VERB}", "pins": [{"pin": 2, "channels": []}]}), "Kumi: Pins (ks811)");
    state(|s| s.tracks[3].fx[0].pins.insert((false, 1), 0b1000));
    assert_eq!(lab.undo("fx-pins", &preview, &applied, "Kumi: undo (ks812)").unwrap_err().0, "changed");
    assert_eq!(state(|s| s.tracks[3].fx[0].pins.get(&(false, 1)).copied()), Some(0b1000));
    // A pin it doesn't have, one given twice: refused.
    let refused = |input: Value| lab.call("change_preview", json!({"kind": "fx-pins", "input": input})).unwrap_err().1;
    assert!(refused(json!({"ref": "r:fx:{FX-VERB}", "pins": [{"pin": 5, "channels": [1]}]})).contains("from 1 to 4"));
    assert!(refused(json!({"ref": "r:fx:{FX-VERB}", "pins": [{"pin": 1, "channels": [1]}, {"pin": 1, "channels": [2]}]})).contains("twice"));
}
