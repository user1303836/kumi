//! Items (m4-r-items): each kind applied, undone by Kumi, refused when the producer changed it in between, gone, back
//! already, and the fence it is refused by.

use serde_json::{json, Value};

use super::*;
use crate::reaper::fake::{self, FakeItem, FakeTake};

pub(super) struct Lab {
    pub changes: Changes,
    pub reads: Reads,
}

impl Lab {
    pub fn new() -> Lab {
        fake::lab_project();
        Lab { changes: Changes::default(), reads: Reads::default() }
    }

    pub fn call(&self, method: &str, mut params: Value) -> Answer {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        if params.get("epoch").is_none() {
            params["epoch"] = self.reads.epoch().into();
        }
        match method {
            "change_preview" => self.changes.preview(&reaper, &self.reads, &params),
            "change_apply" => self.changes.apply(&reaper, &self.reads, &params),
            "change_restore" => self.changes.restore(&reaper, &self.reads, &params),
            "read" => self.reads.read(&reaper, &params),
            other => panic!("{other}"),
        }
    }

    pub fn preview(&self, kind: &str, input: Value) -> Answer {
        self.call("change_preview", json!({"kind": kind, "input": input}))
    }

    pub fn apply_with(&self, kind: &str, input: &Value, preview: &Value, desc: &str) -> Answer {
        self.call("change_apply", json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": desc}))
    }

    /// A change as Kumi's host makes it: preview, then apply with the preview's fence.
    pub fn make(&self, kind: &str, input: Value) -> (Value, Value) {
        let preview = self.preview(kind, input.clone()).unwrap_or_else(|e| panic!("preview {kind}: {e:?}"));
        let applied =
            self.apply_with(kind, &input, &preview, &format!("Kumi: {kind} (k0001)")).unwrap_or_else(|e| panic!("apply {kind}: {e:?}"));
        (preview, applied)
    }

    pub fn undo_with(&self, kind: &str, preview: &Value, applied: &Value, discard: bool) -> Answer {
        let mut params = json!({"kind": kind, "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": format!("Kumi: undo {kind} (k0002)")});
        for key in ["material", "created"] {
            if let Some(value) = applied.get(key) {
                params[key] = value.clone();
            }
        }
        if discard {
            params["discard"] = true.into();
        }
        self.call("change_restore", params)
    }

    pub fn undo(&self, kind: &str, preview: &Value, applied: &Value) -> Answer {
        self.undo_with(kind, preview, applied, false)
    }
}

pub(super) fn state<T>(f: impl FnOnce(&mut fake::State) -> T) -> T {
    fake::with(f)
}

pub(super) fn top() -> Option<String> {
    let api = fake::api();
    let reaper = Reaper::new(&api);
    reaper.undo_can_undo(reaper.current_project().unwrap().ptr)
}

pub(super) fn code(answer: Answer) -> String {
    answer.expect_err("a refusal").0
}

/// The item with this GUID, wherever it is: (track, item).
pub(super) fn item(guid: &str) -> Option<(usize, FakeItem)> {
    state(|s| s.tracks.iter().enumerate().find_map(|(t, track)| track.items.iter().find(|i| i.guid == guid).map(|i| (t, i.clone()))))
}

pub(super) fn guid_in(reference: &Value) -> String {
    reference.as_str().unwrap().rsplit(':').next().unwrap().to_string()
}

pub(super) const KICK: &str = "r:track:{BBBB}";
pub(super) const SNARE: &str = "r:track:{CCCC}";
pub(super) const K1: &str = "r:item:{ITEM-K1}";
pub(super) const S1: &str = "r:item:{ITEM-S1}";

// ----- making items -----

#[test]
fn an_empty_midi_item_is_made_on_a_track_and_undone_while_it_is_as_kumi_made_it() {
    let lab = Lab::new();
    // 120 BPM: beat 16 is 8 seconds.
    let input = json!({"ref": KICK, "positionBeats": 16.0, "lengthBeats": 8.0, "name": "Bassline"});
    let preview = lab.preview("item-add", input.clone()).unwrap();
    assert_eq!(preview["display"]["title"], "Added the MIDI item “Bassline” on “Kick” at bar 5");
    assert_eq!(state(|s| s.tracks[1].items.len()), 2, "a preview writes nothing");
    let applied = lab.apply_with("item-add", &input, &preview, "Kumi: add (k1)").unwrap();
    let made = guid_in(&applied["created"]["ref"]);
    let (track, made_item) = item(&made).expect("the item is made");
    assert_eq!((track, made_item.position, made_item.length), (1, 8.0, 4.0));
    assert_eq!(made_item.takes[0].name, "Bassline");
    assert_eq!(made_item.takes[0].source.as_deref(), Some("MIDI"));
    assert!(applied["created"]["takeRef"].as_str().unwrap().starts_with("r:take:"));
    assert_eq!(top().as_deref(), Some("Kumi: add (k1)"));
    lab.undo("item-add", &preview, &applied).unwrap();
    assert!(item(&made).is_none(), "undone: the item is gone");
    // Gone already: the undo says so.
    assert_eq!(code(lab.undo("item-add", &preview, &applied)), "gone");
}

#[test]
fn an_empty_midi_item_keeps_a_point_of_reapers_undo() {
    // REAPER's undo doesn't notice an item CreateNewMIDIItemInProj makes (accept-reaper lab: a plan of one item-add made
    // no point, and REAPER's undo took the producer's step before it instead).
    let lab = Lab::new();
    let input = json!({"ref": KICK, "position": 10.0, "length": 2.0});
    let preview = lab.preview("item-add", input.clone()).unwrap();
    let applied = lab.apply_with("item-add", &input, &preview, "Kumi: add (k9)").unwrap();
    assert_eq!(applied["undo"]["entry"], true);
    assert_eq!(top().as_deref(), Some("Kumi: add (k9)"));
}

#[test]
fn an_item_kumi_made_is_kept_once_the_producer_has_filled_it_unless_discarded() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("item-add", json!({"ref": KICK, "position": 10.0, "length": 2.0}));
    let made = guid_in(&applied["created"]["ref"]);
    // The producer writes a note into it.
    state(|s| {
        let item = s.tracks[1].items.iter_mut().find(|i| i.guid == made).unwrap();
        let raw = item.takes[0].more.raw.as_mut().unwrap();
        raw.insert(0, crate::midi::Event { pos: 0, flag: 0, msg: vec![0x90, 60, 100] });
        raw.insert(1, crate::midi::Event { pos: 480, flag: 0, msg: vec![0x80, 60, 0] });
    });
    assert_eq!(code(lab.undo("item-add", &preview, &applied)), "changed");
    assert!(item(&made).is_some());
    lab.undo_with("item-add", &preview, &applied, true).unwrap();
    assert!(item(&made).is_none());
}

#[test]
fn making_an_item_checks_the_track_and_the_span() {
    let lab = Lab::new();
    for (input, needle) in [
        (json!({"ref": "r:track:master", "position": 0.0, "length": 1.0}), "master"),
        (json!({"ref": KICK, "length": 1.0}), "positionBeats"),
        (json!({"ref": KICK, "position": 1.0}), "how long"),
        (json!({"ref": KICK, "position": 1.0, "end": 0.5}), "end after"),
        (json!({"ref": KICK, "position": 1.0, "length": 1.0, "lane": 1}), "no fixed lanes"),
        (json!({"ref": K1, "position": 1.0, "length": 1.0}), "track ref"),
    ] {
        let (code, message) = lab.preview("item-add", input.clone()).unwrap_err();
        assert_eq!(code, "bad-params", "{input}");
        assert!(message.contains(needle), "{input}: {message}");
    }
    // The track went between the preview and the apply.
    let input = json!({"ref": KICK, "position": 1.0, "length": 1.0});
    let preview = lab.preview("item-add", input.clone()).unwrap();
    state(|s| s.tracks.remove(1));
    assert_eq!(code(lab.apply_with("item-add", &input, &preview, "Kumi: x (k1)")), "changed");
}

fn media_file(name: &str, kind: &str, length: f64, beats: bool) -> String {
    let path = std::env::temp_dir().join(format!("kumi-items-{}-{name}", std::process::id()));
    std::fs::write(&path, b"RIFF").unwrap();
    let path = path.to_string_lossy().into_owned();
    fake::items::add_media(&path, kind, length, beats);
    path
}

#[test]
fn a_media_file_is_imported_in_place_and_a_missing_or_unreadable_one_is_refused() {
    let lab = Lab::new();
    let path = media_file("loop.wav", "WAVE", 3.5, false);
    let input = json!({"ref": SNARE, "path": path, "positionBeats": 4.0});
    let preview = lab.preview("item-import", input.clone()).unwrap();
    assert!(preview["display"]["title"].as_str().unwrap().contains("on “Snare” at bar 2"), "{}", preview["display"]);
    assert!(preview["display"]["note"].as_str().unwrap().contains("stays where it is"));
    let applied = lab.apply_with("item-import", &input, &preview, "Kumi: import (k1)").unwrap();
    let made = guid_in(&applied["created"]["ref"]);
    let (_, imported) = item(&made).unwrap();
    assert_eq!((imported.position, imported.length), (2.0, 3.5), "the file's whole length by default");
    assert_eq!(imported.takes[0].more.file.as_deref(), Some(path.as_str()));
    assert_eq!(imported.takes[0].source.as_deref(), Some("WAVE"));
    assert!(imported.takes[0].name.ends_with("loop.wav"));
    // The probe source the preview read is freed; the take's isn't.
    assert!(state(|s| s.items.sources.iter().all(|source| source.freed)));
    lab.undo("item-import", &preview, &applied).unwrap();
    assert!(item(&made).is_none());
    // Given a name, the take and the change's title have it (a stem's file carries its time).
    let named = lab.preview("item-import", json!({"ref": SNARE, "path": path, "positionBeats": 4.0, "name": "Loop drums"})).unwrap();
    assert!(named["display"]["title"].as_str().unwrap().starts_with("Added “Loop drums” on “Snare”"), "{}", named["display"]);
    // A MIDI file's length is in beats.
    let midi = media_file("riff.mid", "MIDI", 8.0, true);
    let (_, applied) = lab.make("item-import", json!({"ref": SNARE, "path": midi, "position": 0.0}));
    assert_eq!(item(&guid_in(&applied["created"]["ref"])).unwrap().1.length, 4.0);
    // Missing, a folder, relative, unreadable.
    let unreadable = std::env::temp_dir().join(format!("kumi-items-{}-notes.txt", std::process::id()));
    std::fs::write(&unreadable, b"hello").unwrap();
    for (path, needle) in [
        // (a full path on every system: "/nowhere" has no drive on Windows)
        (std::env::temp_dir().join("nowhere").join("at").join("all.wav").to_string_lossy().into_owned(), "no file"),
        (std::env::temp_dir().to_string_lossy().into_owned(), "no file"),
        ("relative.wav".to_string(), "full path"),
        (unreadable.to_string_lossy().into_owned(), "reads nothing"),
    ] {
        let (code, message) = lab.preview("item-import", json!({"ref": SNARE, "path": path, "position": 0.0})).unwrap_err();
        assert_eq!(code, "bad-params", "{path}");
        assert!(message.contains(needle), "{path}: {message}");
    }
}

#[test]
fn an_imported_file_loops_as_the_producers_reaper_loops_imported_media() {
    let lab = Lab::new();
    let path = media_file("pad.wav", "WAVE", 2.0, false);
    let loops = |lab: &Lab| {
        let (_, applied) = lab.make("item-import", json!({"ref": SNARE, "path": path, "position": 0.0}));
        item(&guid_in(&applied["created"]["ref"])).unwrap().1.extra.get("B_LOOPSRC").copied()
    };
    assert_eq!(loops(&lab), Some(1.0), "REAPER's factory setting loops imported media");
    // A power user's "loop source" off for imported media (loopnewitems without &4).
    fake::extra(|e| e.config.insert("loopnewitems".into(), "10".into()));
    assert_eq!(loops(&lab), Some(0.0), "the producer's setting holds for Kumi's imports too");
}

#[test]
fn an_imported_file_has_no_fades_as_the_producers_own_import_has_none() {
    let lab = Lab::new();
    let path = media_file("kick.wav", "WAVE", 0.5, false);
    // REAPER's factory setup: new items get 10 ms fades, but Insert Media's items get none, and an imported kick keeps
    // its hit.
    let (_, applied) = lab.make("item-import", json!({"ref": SNARE, "path": path, "position": 0.0}));
    let (_, imported) = item(&guid_in(&applied["created"]["ref"])).unwrap();
    let fade = |key: &str| imported.extra.get(key).copied().unwrap_or(0.0);
    assert_eq!((fade("D_FADEINLEN"), fade("D_FADEOUTLEN")), (0.0, 0.0));
}

/// Kumi's own folder with a render in it, and the project's media folder: the render's path and the folders (kept alive).
pub(super) fn kumi_render(lab: &Lab, name: &str) -> (tempfile::TempDir, tempfile::TempDir, String) {
    let kumi = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    let listening = kumi.path().join("bridges/reaper/listening");
    std::fs::create_dir_all(&listening).unwrap();
    let path = listening.join(name);
    std::fs::write(&path, b"RIFF render").unwrap();
    let path = path.to_string_lossy().into_owned();
    fake::items::add_media(&path, "WAVE", 2.0, false);
    lab.reads.set_kumi_folder(kumi.path().to_path_buf());
    fake::with(|s| s.media = media.path().to_string_lossy().into_owned());
    (kumi, media, path)
}

#[test]
fn a_render_in_kumis_own_folder_is_imported_from_a_copy_in_the_projects_media_folder() {
    let lab = Lab::new();
    let (_kumi, media, render) = kumi_render(&lab, "bounce.wav");
    // A file of that name in the media folder already is never written over.
    std::fs::write(media.path().join("bounce.wav"), b"the producer's").unwrap();
    let copy = media.path().join("bounce (2).wav").to_string_lossy().into_owned();
    fake::items::add_media(&copy, "WAVE", 2.0, false);
    let input = json!({"ref": SNARE, "path": render, "positionBeats": 0.0});
    let preview = lab.preview("item-import", input.clone()).unwrap();
    assert!(preview["display"]["note"].as_str().unwrap().contains("a copy of it in the project's media folder"), "{}", preview["display"]);
    assert!(!std::path::Path::new(&copy).exists(), "the preview copies nothing");
    let applied = lab.apply_with("item-import", &input, &preview, "Kumi: import (k1)").unwrap();
    let (_, imported) = item(&guid_in(&applied["created"]["ref"])).unwrap();
    assert_eq!(imported.takes[0].more.file.as_deref(), Some(copy.as_str()), "the project plays the copy");
    assert_eq!(std::fs::read(&copy).unwrap(), b"RIFF render");
    assert_eq!(std::fs::read(media.path().join("bounce.wav")).unwrap(), b"the producer's");
    assert!(imported.takes[0].name.ends_with("bounce.wav"), "named after the render");
    // Without a media folder to copy into, nothing is made.
    fake::with(|s| s.media.clear());
    let preview = lab.preview("item-import", input.clone()).unwrap();
    let (code, message) = lab.apply_with("item-import", &input, &preview, "Kumi: import (k2)").unwrap_err();
    assert_eq!(code, "failed");
    assert!(message.contains("no media folder"), "{message}");
}

// ----- moving -----

#[test]
fn an_item_moves_in_time_and_to_another_track_and_back_by_its_id() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("item-move", json!({"ref": K1, "positionBeats": 12.0, "toRef": SNARE}));
    assert_eq!(preview["display"]["title"], "Moved “kick.wav” to bar 4 on “Snare”");
    let (track, moved) = item("{ITEM-K1}").unwrap();
    assert_eq!((track, moved.position), (2, 6.0));
    assert_eq!(applied["applied"]["track"], "{CCCC}");
    // The producer moves another item meanwhile: the list Kumi's item is in changed, its place didn't.
    state(|s| s.tracks[1].items[0].position = 30.0);
    lab.undo("item-move", &preview, &applied).unwrap();
    let (track, back) = item("{ITEM-K1}").unwrap();
    assert_eq!((track, back.position), (1, 0.0));
    // Back already: nothing written.
    let again = lab.undo("item-move", &preview, &applied).unwrap();
    assert!(again["display"]["note"].as_str().unwrap().contains("back already"), "{again}");
}

#[test]
fn a_move_is_refused_when_the_producer_moved_the_item_in_between_and_its_undo_too() {
    let lab = Lab::new();
    let input = json!({"ref": K1, "position": 5.0});
    let preview = lab.preview("item-move", input.clone()).unwrap();
    state(|s| s.tracks[1].items[0].position = 0.5);
    assert_eq!(code(lab.apply_with("item-move", &input, &preview, "Kumi: x (k1)")), "changed");
    assert_eq!(item("{ITEM-K1}").unwrap().1.position, 0.5, "nothing written");
    let (preview, applied) = lab.make("item-move", json!({"ref": K1, "position": 5.0}));
    state(|s| s.tracks[1].items.iter_mut().find(|i| i.guid == "{ITEM-K1}").unwrap().position = 7.0);
    assert_eq!(code(lab.undo("item-move", &preview, &applied)), "changed");
    assert_eq!(item("{ITEM-K1}").unwrap().1.position, 7.0);
    // (put back where the change found it: where the producer had moved it before)
    lab.undo_with("item-move", &preview, &applied, true).unwrap();
    assert_eq!(item("{ITEM-K1}").unwrap().1.position, 0.5);
    // Deleted since: gone.
    let (preview, applied) = lab.make("item-move", json!({"ref": K1, "position": 5.0}));
    state(|s| s.tracks[1].items.retain(|i| i.guid != "{ITEM-K1}"));
    assert_eq!(code(lab.undo("item-move", &preview, &applied)), "gone");
}

#[test]
fn a_locked_item_is_left_as_it_is_until_it_is_unlocked() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].extra.insert("C_LOCK".into(), 1.0));
    let input = json!({"ref": K1, "position": 5.0});
    let preview = lab.preview("item-move", input.clone()).unwrap();
    assert!(preview["display"]["note"].as_str().unwrap().contains("locked"));
    let (code_, message) = lab.apply_with("item-move", &input, &preview, "Kumi: x (k1)").unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("unlock it first"), "{message}");
    for kind in ["item-split", "item-delete"] {
        let input = if kind == "item-split" { json!({"ref": K1, "position": 1.0}) } else { json!({"ref": K1}) };
        let preview = lab.preview(kind, input.clone()).unwrap();
        assert_eq!(code(lab.apply_with(kind, &input, &preview, "Kumi: x (k1)")), "bad-params", "{kind}");
    }
    // Unlocked and moved in one step, and the undo locks it again after moving it back.
    let (preview, applied) = lab.make("item-props", json!({"ref": K1, "lock": false, "mute": true}));
    assert_eq!(state(|s| (s.tracks[1].items[0].extra["C_LOCK"], s.tracks[1].items[0].mute)), (0.0, true));
    lab.undo("item-props", &preview, &applied).unwrap();
    assert_eq!(state(|s| (s.tracks[1].items[0].extra["C_LOCK"], s.tracks[1].items[0].mute)), (1.0, false));
}

// ----- splitting -----

#[test]
fn a_split_makes_a_right_item_and_its_undo_puts_the_item_back_whole() {
    let lab = Lab::new();
    state(|s| {
        let take = &mut s.tracks[1].items[0].takes[0];
        take.more.stretch = vec![(0.5, 0.5, 0.0), (1.5, 1.5, 0.0)];
        take.more.markers = vec![(1.2, "hit".into(), 0)];
    });
    let before = item("{ITEM-K1}").unwrap().1;
    let (preview, applied) = lab.make("item-split", json!({"ref": K1, "position": 1.0}));
    assert_eq!(preview["display"]["title"], "Split “kick.wav” at bar 1 beat 3");
    let right = guid_in(&applied["created"]["ref"]);
    let (left_now, right_now) = (item("{ITEM-K1}").unwrap().1, item(&right).unwrap().1);
    assert_eq!((left_now.length, right_now.position, right_now.length), (1.0, 1.0, 1.0));
    assert_ne!(right_now.takes[0].guid, before.takes[0].guid, "the right part's take is a new one");
    assert!(applied["material"]["chunk"].as_str().unwrap().contains("IGUID {ITEM-K1}"));
    lab.undo("item-split", &preview, &applied).unwrap();
    assert!(item(&right).is_none(), "the right part is taken away");
    let whole = item("{ITEM-K1}").unwrap().1;
    assert!(whole == before, "the item is exactly as before the split");
    // Back already.
    let again = lab.undo("item-split", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

#[test]
fn a_split_is_undone_only_while_both_parts_are_as_it_left_them() {
    let lab = Lab::new();
    // The producer edits the right part.
    let (preview, applied) = lab.make("item-split", json!({"ref": K1, "position": 1.0}));
    let right = guid_in(&applied["created"]["ref"]);
    state(|s| s.tracks[1].items.iter_mut().find(|i| i.guid == right).unwrap().mute = true);
    assert_eq!(code(lab.undo("item-split", &preview, &applied)), "changed");
    // The producer deletes the right part: putting the item back whole would bring it back.
    state(|s| s.tracks[1].items.retain(|i| i.guid != right));
    let (_, message) = lab.undo("item-split", &preview, &applied).unwrap_err();
    assert!(message.contains("deleted in REAPER"), "{message}");
    // The producer edits the left part.
    let lab = Lab::new();
    let (preview, applied) = lab.make("item-split", json!({"ref": K1, "position": 1.0}));
    state(|s| s.tracks[1].items[0].extra.insert("D_VOL".into(), 0.5));
    assert_eq!(code(lab.undo("item-split", &preview, &applied)), "changed");
    // Discard too: the whole item from before would write over the producer's volume (review R4).
    let (code_, why) = lab.undo_with("item-split", &preview, &applied, true).unwrap_err();
    assert!(code_ == "changed" && why.contains("even with discard"), "{why}");
    assert_eq!(item("{ITEM-K1}").unwrap().1.extra.get("D_VOL"), Some(&0.5), "the producer's edit stays");
}

#[test]
fn a_split_outside_the_item_or_of_a_changed_item_is_refused_and_writes_nothing() {
    let lab = Lab::new();
    for position in [0.0, 2.0, 5.0] {
        assert_eq!(code(lab.preview("item-split", json!({"ref": K1, "position": position}))), "bad-params", "{position}");
    }
    let input = json!({"ref": K1, "position": 1.0});
    let preview = lab.preview("item-split", input.clone()).unwrap();
    state(|s| s.tracks[1].items[0].length = 1.8);
    assert_eq!(code(lab.apply_with("item-split", &input, &preview, "Kumi: x (k1)")), "changed");
    assert_eq!(state(|s| s.tracks[1].items.len()), 2);
    // REAPER refusing the split: failed, nothing made.
    let preview = lab.preview("item-split", input.clone()).unwrap();
    state(|s| s.items.fail_split = true);
    assert_eq!(code(lab.apply_with("item-split", &input, &preview, "Kumi: x (k1)")), "failed");
    assert_eq!(state(|s| s.tracks[1].items.len()), 2);
}

#[test]
fn a_midi_item_split_keeps_its_events_with_each_part_and_comes_back_whole() {
    let lab = Lab::new();
    // The snare's take as a full buffer: notes, a controller and the end marker.
    let events = vec![
        crate::midi::Event { pos: 0, flag: 0, msg: vec![0xB0, 7, 100] },
        crate::midi::Event { pos: 0, flag: 0, msg: vec![0x90, 38, 100] },
        crate::midi::Event { pos: 480, flag: 0, msg: vec![0x80, 38, 0] },
        crate::midi::Event { pos: 3840, flag: 0, msg: vec![0x90, 40, 90] },
        crate::midi::Event { pos: 4320, flag: 0, msg: vec![0x80, 40, 0] },
        crate::midi::Event { pos: 7680, flag: 0, msg: vec![0xB0, 0x7B, 0] },
    ];
    state(|s| s.tracks[2].items[0].takes[0].more.raw = Some(events.clone()));
    let (preview, applied) = lab.make("item-split", json!({"ref": S1, "position": 10.0}));
    let right = guid_in(&applied["created"]["ref"]);
    let left_raw = item("{ITEM-S1}").unwrap().1.takes[0].more.raw.clone().unwrap();
    assert_eq!(left_raw.last().unwrap().pos, 3840, "the left part's source ends at the split");
    assert!(item(&right).unwrap().1.takes[0].more.raw.as_ref().unwrap().iter().any(|e| e.msg == [0x90, 40, 90]));
    lab.undo("item-split", &preview, &applied).unwrap();
    assert_eq!(item("{ITEM-S1}").unwrap().1.takes[0].more.raw.clone().unwrap(), events);
}

// ----- trimming -----

#[test]
fn a_trim_moves_the_edges_and_the_takes_offsets_so_the_media_stays_in_place() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].extra.insert("B_LOOPSRC".into(), 0.0));
    let (preview, applied) = lab.make("item-trim", json!({"ref": K1, "start": 0.5, "end": 1.5}));
    assert_eq!(preview["display"]["title"], "Trimmed “kick.wav” to bar 1 beat 2–bar 1 beat 4");
    let trimmed = item("{ITEM-K1}").unwrap().1;
    assert_eq!((trimmed.position, trimmed.length, trimmed.takes[0].extra["D_STARTOFFS"]), (0.5, 1.0, 0.5));
    lab.undo("item-trim", &preview, &applied).unwrap();
    let back = item("{ITEM-K1}").unwrap().1;
    assert_eq!((back.position, back.length, back.takes[0].extra["D_STARTOFFS"]), (0.0, 2.0, 0.0));
    // The playrate counts: a take at half speed moves half as far into its source.
    state(|s| s.tracks[1].items[0].takes[0].extra.insert("D_PLAYRATE".into(), 0.5));
    lab.make("item-trim", json!({"ref": K1, "start": 1.0}));
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes[0].extra["D_STARTOFFS"], 0.5);
}

#[test]
fn a_start_trims_undo_leaves_an_item_the_producer_put_stretch_markers_on_since() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].extra.insert("B_LOOPSRC".into(), 0.0));
    let (preview, applied) = lab.make("item-trim", json!({"ref": K1, "start": 0.5}));
    // The producer warps the trimmed item.
    state(|s| s.tracks[1].items[0].takes[0].more.stretch = vec![(0.25, 0.75, 0.0)]);
    for discard in [false, true] {
        let (code, message) = lab.undo_with("item-trim", &preview, &applied, discard).unwrap_err();
        assert_eq!(code, "changed", "discard {discard}");
        assert!(message.contains("stretch markers"), "{message}");
        assert_eq!(item("{ITEM-K1}").unwrap().1.position, 0.5, "the item is as the producer has it");
    }
    // Without markers, the start goes back.
    state(|s| s.tracks[1].items[0].takes[0].more.stretch.clear());
    lab.undo("item-trim", &preview, &applied).unwrap();
    assert_eq!(item("{ITEM-K1}").unwrap().1.position, 0.0);
}

#[test]
fn a_trim_before_the_media_or_under_stretch_markers_is_refused_and_its_undo_fenced() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].items[1].extra.insert("B_LOOPSRC".into(), 0.0);
    });
    let (_, message) = lab.preview("item-trim", json!({"ref": "r:item:{ITEM-K2}", "start": 1.0})).unwrap_err();
    assert!(message.contains("before the start of its media"), "{message}");
    // Looping, the item may start before its source does.
    state(|s| s.tracks[1].items[1].extra.insert("B_LOOPSRC".into(), 1.0));
    assert!(lab.preview("item-trim", json!({"ref": "r:item:{ITEM-K2}", "start": 1.0})).is_ok());
    state(|s| s.tracks[1].items[1].takes[0].more.stretch = vec![(1.0, 1.0, 0.0)]);
    let (_, message) = lab.preview("item-trim", json!({"ref": "r:item:{ITEM-K2}", "start": 2.5})).unwrap_err();
    assert!(message.contains("stretch markers"), "{message}");
    // Its end is fine.
    let (preview, applied) = lab.make("item-trim", json!({"ref": "r:item:{ITEM-K2}", "end": 3.0}));
    state(|s| s.tracks[1].items[1].length = 1.5);
    assert_eq!(code(lab.undo("item-trim", &preview, &applied)), "changed");
}

// ----- fades -----

#[test]
fn a_fade_is_set_by_length_shape_and_curve_and_put_back_exactly_whatever_its_shape_was() {
    let lab = Lab::new();
    // A custom fade the producer drew: REAPER's legacy curve with no preset shape.
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let k1 = reaper.track_item(reaper.track_at(Project::first(), 1).unwrap(), 0).unwrap();
    reaper.set_item_value(k1, "D_FADEINLEN", 0.25);
    reaper.set_item_value(k1, "D_FADEINDIR", 0.4);
    let before = item("{ITEM-K1}").unwrap().1.extra.clone();
    let (preview, applied) =
        lab.make("item-fade", json!({"ref": K1, "fadeIn": {"length": 0.5, "shape": 2}, "fadeOut": {"lengthBeats": 1.0}}));
    assert_eq!(preview["display"]["title"], "kick.wav: fade in 0.5 s, fade out 0.5 s");
    let after = item("{ITEM-K1}").unwrap().1.extra;
    assert_eq!((after["D_FADEINLEN"], after["C_FADEINSHAPE"], after["D_FADEOUTLEN"]), (0.5, 2.0, 0.5));
    lab.undo("item-fade", &preview, &applied).unwrap();
    let back = item("{ITEM-K1}").unwrap().1.extra;
    for key in ["D_FADEINLEN", "C_FADEINSHAPE", "D_FADEINDIR", "D_FADEINDIR_NEW", "D_FADEINDIR2_NEW", "D_FADEOUTLEN"] {
        assert_eq!(back.get(key).copied().unwrap_or(0.0), before.get(key).copied().unwrap_or(0.0), "{key}");
    }
    // The 7.81 curve and S values, given with or without a shape.
    let (_, applied) = lab.make("item-fade", json!({"ref": K1, "fadeIn": {"curve": -0.3, "s": 0.6}}));
    assert_eq!((applied["applied"]["fadeIn"]["curve"].as_f64(), applied["applied"]["fadeIn"]["s"].as_f64()), (Some(-0.3), Some(0.6)));
}

#[test]
fn a_fade_longer_than_the_item_or_without_anything_to_set_is_refused_and_its_undo_fenced() {
    let lab = Lab::new();
    for (input, needle) in [
        (json!({"ref": K1, "fadeIn": {"length": 3.0}}), "longer than the item"),
        (json!({"ref": K1}), "fadeIn, fadeOut"),
        (json!({"ref": K1, "fadeIn": {}}), "its length"),
        (json!({"ref": K1, "fadeOut": {"shape": 7}}), "0 to 6"),
        (json!({"ref": K1, "fadeOut": {"curve": 2.0}}), "-1 to 1"),
    ] {
        let (code, message) = lab.preview("item-fade", input.clone()).unwrap_err();
        assert_eq!(code, "bad-params", "{input}");
        assert!(message.contains(needle), "{input}: {message}");
    }
    let (preview, applied) = lab.make("item-fade", json!({"ref": K1, "fadeOut": {"length": 0.3}}));
    state(|s| s.tracks[1].items[0].extra.insert("D_FADEOUTLEN".into(), 0.1));
    assert_eq!(code(lab.undo("item-fade", &preview, &applied)), "changed");
    assert_eq!(state(|s| s.tracks[1].items[0].extra["D_FADEOUTLEN"]), 0.1);
}

// ----- properties -----

#[test]
fn item_properties_are_set_together_and_each_put_back() {
    let lab = Lab::new();
    let input =
        json!({"ref": K1, "mute": true, "volumeDb": -6.0, "color": "#ff0000", "loop": true, "timebase": "beats", "snapOffset": 0.25});
    let (preview, applied) = lab.make("item-props", input);
    assert_eq!(
        preview["display"]["title"],
        "kick.wav: muted, volume −6 dB, colour changed, loops its source, timebase beats, snap offset 0.25 s"
    );
    // The display's before and after are in the request's words, not REAPER's numbers.
    assert_eq!(
        applied["display"]["before"],
        json!({"mute": false, "volumeDb": 0.0, "color": null, "loop": false, "timebase": "project", "snapOffset": 0.0})
    );
    assert_eq!(
        applied["display"]["after"],
        json!({"mute": true, "volumeDb": -6.0, "color": "#ff0000", "loop": true, "timebase": "beats", "snapOffset": 0.25})
    );
    let set = item("{ITEM-K1}").unwrap().1;
    assert!(set.mute && (set.extra["D_VOL"] - 0.501187).abs() < 1e-5 && set.color == fake::native_color(255, 0, 0));
    assert_eq!((set.extra["B_LOOPSRC"], set.extra["C_BEATATTACHMODE"], set.extra["D_SNAPOFFSET"]), (1.0, 1.0, 0.25));
    lab.undo("item-props", &preview, &applied).unwrap();
    let back = item("{ITEM-K1}").unwrap().1;
    assert!(!back.mute && back.extra["D_VOL"] == 1.0 && back.color == 0);
    assert_eq!((back.extra["B_LOOPSRC"], back.extra["C_BEATATTACHMODE"], back.extra["D_SNAPOFFSET"]), (0.0, -1.0, 0.0));
    // Only what it set is fenced: the producer's fade since doesn't stop the undo of a mute.
    let (preview, applied) = lab.make("item-props", json!({"ref": K1, "mute": true}));
    state(|s| s.tracks[1].items[0].extra.insert("D_FADEINLEN".into(), 0.3));
    lab.undo("item-props", &preview, &applied).unwrap();
    // But a volume changed since is the producer's; one put back as it was by the producer is back already.
    let (preview, applied) = lab.make("item-props", json!({"ref": K1, "volumeDb": -6.0}));
    state(|s| s.tracks[1].items[0].extra.insert("D_VOL".into(), 0.7));
    assert_eq!(code(lab.undo("item-props", &preview, &applied)), "changed");
    assert_eq!(state(|s| s.tracks[1].items[0].extra["D_VOL"]), 0.7);
    let (preview, applied) = lab.make("item-props", json!({"ref": K1, "mute": true}));
    state(|s| s.tracks[1].items[0].mute = false);
    assert_eq!(lab.undo("item-props", &preview, &applied).unwrap()["undo"]["entry"], false);
}

#[test]
fn an_unchanged_property_is_said_and_writes_no_point() {
    let lab = Lab::new();
    let preview = lab.preview("item-props", json!({"ref": K1, "mute": false})).unwrap();
    assert_eq!(preview["display"]["unchanged"], true);
    let entries = state(|s| s.undo_entries);
    let applied = lab.apply_with("item-props", &json!({"ref": K1, "mute": false}), &preview, "Kumi: x (k1)").unwrap();
    assert_eq!(applied["undo"]["entry"], false);
    assert_eq!(state(|s| s.undo_entries), entries);
}

// ----- groups -----

#[test]
fn items_are_grouped_in_a_new_group_and_put_back_in_theirs() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[1].extra.insert("I_GROUPID".into(), 4.0));
    let (preview, applied) = lab.make("item-group", json!({"itemRefs": [K1, S1], "grouped": true}));
    assert_eq!(preview["display"]["title"], "Grouped 2 items (group 5)");
    assert_eq!(state(|s| (s.tracks[1].items[0].extra["I_GROUPID"], s.tracks[2].items[0].extra["I_GROUPID"])), (5.0, 5.0));
    lab.undo("item-group", &preview, &applied).unwrap();
    assert_eq!(state(|s| (s.tracks[1].items[0].extra["I_GROUPID"], s.tracks[2].items[0].extra["I_GROUPID"])), (0.0, 0.0));
    // Ungrouping, and the producer regrouping one meanwhile.
    let (preview, applied) = lab.make("item-group", json!({"itemRefs": ["r:item:{ITEM-K2}"], "grouped": false}));
    assert_eq!(state(|s| s.tracks[1].items[1].extra["I_GROUPID"]), 0.0);
    state(|s| s.tracks[1].items[1].extra.insert("I_GROUPID".into(), 9.0));
    assert_eq!(code(lab.undo("item-group", &preview, &applied)), "changed");
    assert_eq!(code(lab.preview("item-group", json!({"itemRefs": [K1], "grouped": true}))), "bad-params", "one item is no group");
    assert_eq!(code(lab.preview("item-group", json!({"itemRefs": [K1, K1], "grouped": true}))), "bad-params");
}

// ----- deleting -----

#[test]
fn a_deleted_items_undo_leaves_it_out_once_the_tempo_map_changed_unless_told_to_put_it_back_anyway() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("item-delete", json!({"ref": K1}));
    // The producer adds a tempo change before where it was: at its old time it would sit at another bar.
    state(|s| s.tempo_markers.push(fake::FakeTempo { time: 0.0, bpm: 90.0, num: 4, denom: 4, linear: false }));
    let (code, message) = lab.undo("item-delete", &preview, &applied).unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("tempo map"), "{message}");
    assert!(item("{ITEM-K1}").is_none(), "nothing put back");
    lab.undo_with("item-delete", &preview, &applied, true).unwrap();
    assert!(item("{ITEM-K1}").is_some());
}

#[test]
fn a_deleted_items_undo_isnt_refused_because_the_cursor_moved_across_a_tempo_marker() {
    let lab = Lab::new();
    state(|s| {
        s.tempo_markers = vec![
            fake::FakeTempo { time: 0.0, bpm: 120.0, num: 4, denom: 4, linear: false },
            fake::FakeTempo { time: 8.0, bpm: 90.0, num: 4, denom: 4, linear: false },
        ]
    });
    let (preview, applied) = lab.make("item-delete", json!({"ref": K1}));
    // The cursor goes past the second marker: REAPER's tempo (at the cursor) reads 90 now, the map is the same.
    state(|s| s.bpm = 90.0);
    lab.undo("item-delete", &preview, &applied).unwrap();
    assert!(item("{ITEM-K1}").is_some());
}

#[test]
fn undoing_item_props_puts_back_only_kumis_fields_and_leaves_a_lock_the_producer_set_since() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("item-props", json!({"ref": K1, "mute": true}));
    state(|s| {
        s.tracks[1].items[0].extra.insert("C_LOCK".into(), 1.0);
    });
    lab.undo("item-props", &preview, &applied).unwrap();
    let back = item("{ITEM-K1}").unwrap().1;
    assert!(!back.mute, "Kumi's mute is taken back");
    assert_eq!(back.extra.get("C_LOCK"), Some(&1.0), "the producer's lock stays");
}

#[test]
fn a_deleted_item_comes_back_with_its_ids_from_its_chunk() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].takes[0].more.markers = vec![(0.2, "hit".into(), 0)]);
    let before = item("{ITEM-K1}").unwrap().1;
    let (preview, applied) = lab.make("item-delete", json!({"ref": K1}));
    assert_eq!(preview["display"]["title"], "Deleted “kick.wav”");
    assert!(item("{ITEM-K1}").is_none());
    let restored = lab.undo("item-delete", &preview, &applied).unwrap();
    assert_eq!(restored["restored"]["ref"], K1);
    let (track, back) = item("{ITEM-K1}").unwrap();
    assert_eq!(track, 1);
    assert!(back == before, "the item is back as it was, takes, markers and ids");
    // Back already.
    let again = lab.undo("item-delete", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
    // Its track deleted since: gone.
    let (preview, applied) = lab.make("item-delete", json!({"ref": K1}));
    state(|s| s.tracks.remove(1));
    assert_eq!(code(lab.undo("item-delete", &preview, &applied)), "gone");
}

#[test]
fn a_delete_is_refused_when_the_item_changed_since_the_preview() {
    let lab = Lab::new();
    let preview = lab.preview("item-delete", json!({"ref": K1})).unwrap();
    state(|s| s.tracks[1].items[0].takes[0].name = "kick 2.wav".into());
    assert_eq!(code(lab.apply_with("item-delete", &json!({"ref": K1}), &preview, "Kumi: x (k1)")), "changed");
    assert!(item("{ITEM-K1}").is_some());
}

/// Each kind's apply is refused, writing nothing, when the producer changed what its preview looked at.
#[test]
fn an_apply_is_refused_when_the_producer_changed_the_item_since_the_preview() {
    type Edit = fn(&mut fake::State);
    let cases: [(&str, Value, Edit); 6] = [
        ("item-trim", json!({"ref": K1, "end": 1.0}), |s| s.tracks[1].items[0].length = 1.5),
        ("item-fade", json!({"ref": K1, "fadeIn": {"length": 0.2}}), |s| {
            s.tracks[1].items[0].extra.insert("D_FADEINLEN".into(), 0.3);
        }),
        ("item-props", json!({"ref": K1, "mute": true, "volumeDb": -3.0}), |s| s.tracks[1].items[0].mute = true),
        ("item-group", json!({"itemRefs": [K1, "r:item:{ITEM-K2}"], "grouped": true}), |s| {
            s.tracks[1].items[1].extra.insert("I_GROUPID".into(), 4.0);
        }),
        ("item-duplicate", json!({"ref": K1, "toRef": SNARE, "position": 20.0}), |s| s.tracks[1].items[0].position = 0.5),
        ("item-move", json!({"ref": K1, "position": 3.0}), |s| s.tracks[1].items[0].position = 0.5),
    ];
    for (kind, input, edit) in cases {
        let lab = Lab::new();
        let preview = lab.preview(kind, input.clone()).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        state(edit);
        let before = state(|s| s.tracks.clone());
        let answer = lab.apply_with(kind, &input, &preview, "Kumi: x (k1)");
        assert_eq!(answer.as_ref().err().map(|e| e.0.as_str()), Some("changed"), "{kind}: {answer:?}");
        assert!(state(|s| s.tracks.clone()) == before, "{kind}: nothing written");
    }
}

#[test]
fn a_deleted_item_back_in_the_project_but_changed_is_the_producers() {
    let lab = Lab::new();
    let was = item("{ITEM-K1}").unwrap().1;
    let (preview, applied) = lab.make("item-delete", json!({"ref": K1}));
    // REAPER's undo brings it back, and the producer moves it.
    state(|s| {
        let mut back = was.clone();
        back.position = 3.0;
        s.tracks[1].items.push(back);
    });
    assert_eq!(code(lab.undo("item-delete", &preview, &applied)), "changed");
    assert_eq!(state(|s| s.tracks[1].items.iter().filter(|i| i.guid == "{ITEM-K1}").count()), 1, "no second item with its id");
    // With discard: said as it is, there but changed and left so, not "back already" (review R18).
    let left = lab.undo_with("item-delete", &preview, &applied, true).unwrap();
    assert_eq!(left["display"]["note"], "It is in the project again, changed since it was deleted: Kumi left it as it is.");
    assert!(!left["display"]["title"].as_str().unwrap().contains("back already"), "{left}");
    assert_eq!(item("{ITEM-K1}").unwrap().1.position, 3.0);
    // Back as it was: back already.
    state(|s| s.tracks[1].items.iter_mut().find(|i| i.guid == "{ITEM-K1}").unwrap().position = was.position);
    let again = lab.undo("item-delete", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

/// What REAPER's undo and redo give back is its saved state's text: volumes and fade curves to 6 decimals (lab, 7.82). A
/// producer who undid and redid Kumi's change in REAPER can still have Kumi undo it.
#[test]
fn values_back_from_reapers_undo_and_redo_are_still_the_ones_kumi_wrote() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("item-props", json!({"ref": K1, "volumeDb": -6.0}));
    state(|s| s.tracks[1].items[0].extra.insert("D_VOL".into(), 0.501187));
    lab.undo("item-props", &preview, &applied).unwrap();
    assert_eq!(item("{ITEM-K1}").unwrap().1.extra["D_VOL"], 1.0);
    let (preview, applied) = lab.make("item-fade", json!({"ref": K1, "fadeIn": {"length": 0.25, "curve": 0.123456789}}));
    state(|s| s.tracks[1].items[0].extra.insert("D_FADEINDIR_NEW".into(), 0.123457));
    lab.undo("item-fade", &preview, &applied).unwrap();
    // A change the producer made is still theirs.
    let (preview, applied) = lab.make("item-props", json!({"ref": K1, "volumeDb": -6.0}));
    state(|s| s.tracks[1].items[0].extra.insert("D_VOL".into(), 0.5012));
    assert_eq!(code(lab.undo("item-props", &preview, &applied)), "changed");
}

/// Every item kind's undo says when the item is back already (writing nothing) and when it is gone.
#[test]
fn every_item_undo_says_back_already_and_gone() {
    let back_already = "It was back already: nothing was written.";
    let cases: [(&str, Value); 7] = [
        ("item-move", json!({"ref": K1, "position": 3.0})),
        ("item-split", json!({"ref": K1, "position": 1.0})),
        ("item-trim", json!({"ref": K1, "end": 1.0})),
        ("item-fade", json!({"ref": K1, "fadeIn": {"length": 0.2}})),
        ("item-props", json!({"ref": K1, "mute": true})),
        ("item-group", json!({"itemRefs": [K1, "r:item:{ITEM-K2}"], "grouped": true})),
        ("item-delete", json!({"ref": K1})),
    ];
    for (kind, input) in cases {
        let lab = Lab::new();
        let (preview, applied) = lab.make(kind, input.clone());
        lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        let again = lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind} again: {e:?}"));
        assert_eq!(again["display"]["note"], back_already, "{kind}");
        // The item went (for a delete, the track it was on) before Kumi's undo.
        let lab = Lab::new();
        let (preview, applied) = lab.make(kind, input);
        state(|s| {
            if kind == "item-delete" {
                s.tracks.remove(1);
            } else {
                s.tracks[1].items.retain(|i| i.guid != "{ITEM-K1}");
            }
        });
        assert_eq!(code(lab.undo(kind, &preview, &applied)), "gone", "{kind}");
    }
    // An item Kumi made and its undo took away: the undo again says it is gone.
    for kind in ["item-import", "item-duplicate"] {
        let lab = Lab::new();
        let input = match kind {
            "item-import" => json!({"ref": SNARE, "path": media_file("again.wav", "WAVE", 2.0, false), "position": 0.0}),
            _ => json!({"ref": K1, "position": 20.0}),
        };
        let (preview, applied) = lab.make(kind, input);
        lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        assert_eq!(code(lab.undo(kind, &preview, &applied)), "gone", "{kind}");
    }
}

// ----- copying -----

#[test]
fn a_copy_shares_no_ids_with_its_item_and_its_undo_takes_it_away() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].takes[0].fx = vec![fake::FakeFx::new("{FX-TAKE}", "VST: ReaEQ (Cockos)")]);
    let (preview, applied) = lab.make("item-duplicate", json!({"ref": K1, "toRef": SNARE, "position": 20.0}));
    assert_eq!(preview["display"]["title"], "Copied “kick.wav” to bar 11 on “Snare”");
    let copy = guid_in(&applied["created"]["ref"]);
    let (track, made) = item(&copy).unwrap();
    assert_eq!((track, made.position), (2, 20.0));
    assert_ne!(made.takes[0].guid, "{TAKE-K1}");
    assert_ne!(made.takes[0].fx[0].guid, "{FX-TAKE}");
    // FX in a container get new ids too.
    state(|s| {
        s.tracks[1].items[0].takes[0].fx = vec![fake::FakeFx::container("{FX-BOX}", vec![fake::FakeFx::new("{FX-IN-BOX}", "JS: volume")])]
    });
    let (_, boxed) = lab.make("item-duplicate", json!({"ref": K1, "position": 40.0}));
    let copied = item(&guid_in(&boxed["created"]["ref"])).unwrap().1.takes[0].fx[0].clone();
    let inner = copied.children.as_ref().expect("the copy's container")[0].guid.clone();
    assert!(copied.guid != "{FX-BOX}" && inner != "{FX-IN-BOX}", "{} {inner}", copied.guid);
    // Right after the item by default.
    let (_, applied2) = lab.make("item-duplicate", json!({"ref": K1}));
    assert_eq!(item(&guid_in(&applied2["created"]["ref"])).unwrap().1.position, 2.0);
    lab.undo("item-duplicate", &preview, &applied).unwrap();
    assert!(item(&copy).is_none());
}

#[test]
fn every_item_answer_names_refs_the_way_the_contract_says() {
    let lab = Lab::new();
    let path = media_file("refs.wav", "WAVE", 1.0, false);
    for (kind, input) in [
        ("item-add", json!({"ref": KICK, "position": 30.0, "length": 1.0})),
        ("item-import", json!({"ref": KICK, "path": path, "position": 40.0})),
        ("item-move", json!({"ref": K1, "position": 3.0})),
        ("item-split", json!({"ref": S1, "position": 9.0})),
        ("item-trim", json!({"ref": "r:item:{ITEM-K2}", "end": 3.5})),
        ("item-fade", json!({"ref": "r:item:{ITEM-K2}", "fadeIn": {"length": 0.1}})),
        ("item-props", json!({"ref": "r:item:{ITEM-K2}", "mute": true})),
        ("item-group", json!({"itemRefs": [K1, "r:item:{ITEM-K2}"], "grouped": true})),
        ("item-duplicate", json!({"ref": K1})),
        ("item-delete", json!({"ref": K1})),
    ] {
        let (preview, applied) = lab.make(kind, input);
        let restored = lab.undo(kind, &preview, &applied).unwrap();
        for answer in [&preview, &applied, &restored] {
            assert_eq!(daw_bridge::contract::misplaced_refs(answer), Vec::<String>::new(), "{kind}: {answer}");
        }
    }
}

#[test]
fn a_change_to_an_item_needs_an_item_ref_and_a_take_ref_names_its_item() {
    let lab = Lab::new();
    assert_eq!(code(lab.preview("item-move", json!({"ref": KICK, "position": 1.0}))), "bad-params");
    let preview = lab.preview("item-move", json!({"ref": "r:take:{TAKE-K1}", "position": 1.0})).unwrap();
    assert_eq!(preview["target"]["ref"], K1);
    assert_eq!(code(lab.preview("item-move", json!({"ref": "r:item:{NOPE}", "position": 1.0}))), "failed");
}

#[test]
fn a_fake_take_is_a_media_take_with_no_buffer_unless_given_one() {
    // (the fixture the tests above build on)
    let take = FakeTake::wave("{T}", "x.wav");
    assert!(take.more.raw.is_none() && take.more.file.is_none());
}

// ----- what HISTORY draws (m4-tui) -----

#[test]
fn each_item_kind_tells_history_the_items_it_leaves_before_and_after() {
    let lab = Lab::new();
    let drawn = |applied: &Value, side: &str| -> Vec<(f64, f64)> {
        applied["display"]["items"][side]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| (item["start"].as_f64().unwrap(), item["length"].as_f64().unwrap()))
            .collect()
    };
    let (_, applied) = lab.make("item-move", json!({"ref": K1, "position": 4.0}));
    assert_eq!((drawn(&applied, "before"), drawn(&applied, "after")), (vec![(0.0, 2.0)], vec![(4.0, 2.0)]));
    let item = &applied["display"]["items"]["after"][0];
    assert_eq!((item["name"].as_str(), item["at"].as_str()), (Some("kick.wav"), Some("bar 3")), "named, and where in the ruler's words");
    let (_, applied) = lab.make("item-split", json!({"ref": K1, "position": 5.0}));
    assert_eq!(
        (drawn(&applied, "before"), drawn(&applied, "after")),
        (vec![(4.0, 2.0)], vec![(4.0, 1.0), (5.0, 1.0)]),
        "whole, then both parts"
    );
    let (_, applied) = lab.make("item-fade", json!({"ref": K1, "fadeOut": {"length": 0.25}}));
    assert_eq!(applied["display"]["items"]["after"][0]["fadeOut"], 0.25);
    let (_, applied) = lab.make("item-delete", json!({"ref": S1}));
    assert_eq!((drawn(&applied, "before").len(), drawn(&applied, "after").len()), (1, 0));
    let (_, applied) = lab.make("item-add", json!({"ref": KICK, "position": 8.0, "length": 2.0}));
    assert_eq!((drawn(&applied, "before"), drawn(&applied, "after")), (vec![], vec![(8.0, 2.0)]));
}
