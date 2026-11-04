//! Refs: one spelling for each kind of object, and the lives behind the ids. Undo puts SoundCraft's next id back, so an id
//! can come back on another object: a ref must never find the new one.
use super::vectors::vectors;
use serde_json::{json, Value};
use soundcraft_kumi::refs::{note_index, Id, Kind, Lives, Missing, NoteMissing, Ref, Slot};

fn track(id: u64, life: u32) -> Id {
    Id { kind: Kind::Track, id, life }
}

fn parse(text: &str) -> Ref {
    text.parse().unwrap_or_else(|error| panic!("{text}: {error}"))
}

#[test]
fn every_kind_of_ref_has_one_spelling_and_reads_back_as_written() {
    let clip = Id { kind: Kind::Clip, id: 40, life: 1 };
    let cases = [
        ("s:track:12", Ref::Object(track(12, 1))),
        ("s:track:1.2", Ref::Object(track(1, 2))),
        ("s:clip:40", Ref::Object(clip)),
        ("s:bus:4", Ref::Object(Id { kind: Kind::Bus, id: 4, life: 1 })),
        ("s:group:2", Ref::Object(Id { kind: Kind::Group, id: 2, life: 1 })),
        ("s:marker:6", Ref::Object(Id { kind: Kind::Marker, id: 6, life: 1 })),
        ("s:source:7", Ref::Object(Id { kind: Kind::Source, id: 7, life: 1 })),
        ("s:insert:12:a", Ref::Insert { track: track(12, 1), slot: Slot::Insert(0) }),
        ("s:insert:12.3:j", Ref::Insert { track: track(12, 3), slot: Slot::Insert(9) }),
        ("s:insert:12:instrument", Ref::Insert { track: track(12, 1), slot: Slot::Instrument }),
        ("s:send:12:0", Ref::Send { track: track(12, 1), slot: 0 }),
        ("s:send:12:9", Ref::Send { track: track(12, 1), slot: 9 }),
        ("s:param:12:a:high_shelf_gain", Ref::Param { track: track(12, 1), slot: Slot::Insert(0), param: "high_shelf_gain".into() }),
        ("s:param:12:instrument:7", Ref::Param { track: track(12, 1), slot: Slot::Instrument, param: "7".into() }),
        ("s:env:12:volume", Ref::Lane { track: track(12, 1), param: "volume".into() }),
        ("s:env:12:plugin:b:hpf_freq", Ref::Lane { track: track(12, 1), param: "plugin:b:hpf_freq".into() }),
        ("s:env:12:send_a_level", Ref::Lane { track: track(12, 1), param: "send_a_level".into() }),
        ("s:env:12:pan2", Ref::Lane { track: track(12, 1), param: "pan2".into() }),
        ("s:env:12:plugin:255:trim", Ref::Lane { track: track(12, 1), param: "plugin:255:trim".into() }),
        ("s:point:12:volume:96000", Ref::Point { track: track(12, 1), param: "volume".into(), sample: 96000 }),
        ("s:point:12:plugin:b:hpf_freq:-480", Ref::Point { track: track(12, 1), param: "plugin:b:hpf_freq".into(), sample: -480 }),
        ("s:note:40:480.60.0", Ref::Note { clip, start: 480, pitch: 60, channel: 0 }),
        ("s:note:40.2:-120.127.15", Ref::Note { clip: Id { life: 2, ..clip }, start: -120, pitch: 127, channel: 15 }),
        ("s:playlist:12:2", Ref::Playlist { track: track(12, 1), index: 2 }),
    ];
    for (text, named) in cases {
        assert_eq!(parse(text), named, "{text}");
        assert_eq!(named.to_string(), text);
    }
}

#[test]
fn what_isnt_one_of_the_bridges_refs_is_refused_with_what_it_is() {
    for text in [
        "r:track:{A1}",
        "s:track",
        "s:track:",
        "s:track:x",
        "s:track:+12",
        "s:track:1.1",
        "s:track:1.0",
        "s:lane:12",
        "s:insert:12:k",
        "s:insert:12",
        "s:send:12:a",
        "s:param:12:a:",
        "s:param:12:a:high-shelf",
        "s:env:12:b:hpf_freq",
        "s:env:12:plugin:instrument:cutoff",
        "s:note:40:480.60",
        "s:note:40:480.128.0",
        "s:note:40:480.60.16",
        "s:note:40:480.60.0.1",
        "s:point:12:volume",
        "s:playlist:12:-1",
        // Second spellings of what has one already, and what no read gives.
        "s:track:012",
        "s:track:00",
        "s:track:1.+2",
        "s:track:1.02",
        "s:send:12:00",
        "s:send:12:10",
        "s:note:40:-0.60.0",
        "s:note:40:480.060.0",
        "s:point:12:volume:-0",
        "s:point:12:volume:007",
        // Lanes SoundCraft's commands also take (`vol`, any case, a slot by number), and lanes it has no id for.
        "s:env:12:vol",
        "s:env:12:Volume",
        "s:env:12:VOLUME",
        "s:env:12:pan1",
        "s:env:12:send_A_level",
        "s:env:12:send_0_level",
        "s:env:12:send_k_level",
        "s:env:12:banana",
        "s:env:12:trim",
        "s:env:12:plugin:z:trim",
        "s:env:12:plugin:255:gain",
    ] {
        let refused = text.parse::<Ref>().unwrap_err();
        assert_eq!(refused, format!("{text} isn't a SoundCraft ref (s:<kind>:<parts>, as a read gave it)"));
    }
}

fn tracks(lives: &mut Lives, now: &[(u64, &str)]) {
    let shown: Vec<Value> = now
        .iter()
        .map(
            |(id, name)| json!({"id": id, "kind": "audio", "format": "Mono", "name": name, "instrument": null, "clips": [], "inserts": []}),
        )
        .collect();
    lives.saw_session(&json!({"tracks": shown}));
}

#[test]
fn an_id_that_comes_back_on_another_object_is_a_new_life_and_the_old_ref_is_stale() {
    let mut lives = Lives::new();
    tracks(&mut lives, &[(1, "First")]);
    let first = lives.now(Kind::Track, 1).unwrap();
    assert_eq!(Ref::Object(first).to_string(), "s:track:1");
    // Undo took "First" away and put the next id back, so "Second" got id 1 too (measured on 0.17.0).
    tracks(&mut lives, &[]);
    assert_eq!(lives.resolve(&first), Err(Missing::Gone));
    tracks(&mut lives, &[(1, "Second")]);
    assert_eq!(lives.resolve(&first), Err(Missing::Stale), "never the other object");
    let second = lives.now(Kind::Track, 1).unwrap();
    assert_eq!(Ref::Object(second).to_string(), "s:track:1.2");
    assert_eq!(lives.resolve(&second), Ok(1));
}

#[test]
fn an_id_given_out_again_between_two_looks_is_a_new_life_though_no_look_saw_it_go() {
    let mut lives = Lives::new();
    tracks(&mut lives, &[(33, "Audio 3")]);
    let undone = lives.now(Kind::Track, 33).unwrap();
    // An undo, then a new step, between the looks: the new track got the undone one's id.
    tracks(&mut lives, &[(33, "Bass")]);
    assert_eq!(lives.resolve(&undone), Err(Missing::Stale));
    assert_eq!(lives.now(Kind::Track, 33), Some(track(33, 2)));
}

#[test]
fn an_object_keeps_its_life_while_its_who_it_was_and_when_an_undo_puts_it_back_as_it_was() {
    let mut lives = Lives::new();
    let bass = |inserts: Value| json!({"tracks": [{"id": 3, "kind": "audio", "format": "Mono", "name": "Bass", "instrument": null, "clips": [], "inserts": inserts}]});
    let compressed = json!([{"slot": 0, "plugin": "compressor"}]);
    lives.saw_session(&bass(json!([])));
    let was = lives.now(Kind::Track, 3).unwrap();
    lives.saw_session(&bass(compressed.clone()));
    assert_eq!(lives.resolve(&was), Ok(3), "an insert added: the same track");
    lives.saw_session(&json!({"tracks": []}));
    lives.saw_session(&bass(compressed));
    assert_eq!(lives.resolve(&was), Ok(3), "deleted, and put back by an undo as it was: the same track");
    lives.saw_session(&json!({"tracks": []}));
    lives.saw_session(&bass(json!([])));
    assert_eq!(lives.resolve(&was), Err(Missing::Stale), "back without what it held when it went: another track");
}

#[test]
fn a_kumi_changes_object_that_an_undo_crossed_is_retired_at_once_even_coming_back_the_same() {
    let mut lives = Lives::new();
    tracks(&mut lives, &[(5, "Audio 1")]);
    let made = lives.now(Kind::Track, 5).unwrap();
    lives.retire(Kind::Track, 5);
    assert_eq!(lives.resolve(&made), Err(Missing::Stale));
    assert_eq!(lives.now(Kind::Track, 5), None);
    tracks(&mut lives, &[]);
    tracks(&mut lives, &[(5, "Audio 1")]);
    assert_eq!(lives.resolve(&made), Err(Missing::Stale), "the producer's own Audio 1 on the same id");
    assert_eq!(lives.now(Kind::Track, 5).map(|again| again.life), Some(2));
}

#[test]
fn an_id_retired_before_any_look_saw_it_is_stale_at_once_and_a_new_life_when_a_look_shows_it() {
    let mut lives = Lives::new();
    // A Kumi change's track, known from the change's answer, undone before a look: the producer's Vox gets its id.
    lives.retire(Kind::Track, 33);
    assert_eq!(lives.resolve(&track(33, 1)), Err(Missing::Stale));
    tracks(&mut lives, &[(33, "Vox")]);
    assert_eq!(lives.resolve(&track(33, 1)), Err(Missing::Stale));
    assert_eq!(lives.now(Kind::Track, 33), Some(track(33, 2)));
}

#[test]
fn ids_and_lives_a_table_never_gave_are_unknown_and_each_table_has_an_epoch_of_its_own() {
    let mut lives = Lives::new();
    tracks(&mut lives, &[(1, "First")]);
    assert_eq!(lives.resolve(&track(9, 1)), Err(Missing::Unknown));
    assert_eq!(lives.resolve(&track(1, 4)), Err(Missing::Unknown));
    // Another table names a first life on id 1 too: only the epoch a request carries tells its refs apart.
    let mut another = Lives::new();
    tracks(&mut another, &[(1, "Other")]);
    assert_eq!(another.resolve(&track(1, 1)), Ok(1));
    assert_ne!(another.epoch(), lives.epoch());
}

/// An import, as SoundCraft numbers it in one step: its source, then its track, then its clip.
fn imported(file: &str, name: &str) -> Value {
    json!({
        "sources": [{"id": 2, "name": file, "channels": 1, "frames": 480000, "path": format!("/samples/{file}")}],
        "tracks": [{
            "id": 3, "kind": "audio", "format": "Mono", "name": name, "instrument": null, "inserts": [],
            "clips": [{"id": 4, "name": name, "start": 0, "length": 480000, "offset": 0, "source": 2}],
        }],
    })
}

#[test]
fn an_import_undone_and_made_again_from_another_file_as_long_is_a_new_life_for_its_source_track_and_clip() {
    for look_between in [true, false] {
        let mut lives = Lives::new();
        lives.saw_session(&imported("Kick.wav", "Kick"));
        let kick = [(Kind::Source, 2), (Kind::Track, 3), (Kind::Clip, 4)].map(|(kind, id)| lives.now(kind, id).unwrap());
        if look_between {
            lives.saw_session(&json!({}));
        }
        lives.saw_session(&imported("Snare.wav", "Snare"));
        for id in kick {
            assert_eq!(lives.resolve(&id), Err(Missing::Stale), "{} (a look between: {look_between})", Ref::Object(id));
        }
    }
}

/// One object of each kind, to change one thing of at a time.
fn session() -> Value {
    json!({
        "sources": [{"id": 2, "name": "Kick.wav", "channels": 1, "frames": 48000, "path": "/samples/Kick.wav"}],
        "tracks": [
            {
                "id": 3, "kind": "audio", "format": "Mono", "name": "Kick", "instrument": null,
                "clips": [{"id": 4, "name": "Kick", "start": 0, "length": 48000, "offset": 0, "source": 2}],
                "inserts": [{"slot": 0, "plugin": "eq_7band"}],
            },
            {
                "id": 5, "kind": "instrument", "format": "Stereo", "name": "Keys", "instrument": "subtractive_synth",
                "clips": [{"id": 6, "name": "Keys", "start": 0, "length": 96000, "notes": 4}], "inserts": [],
            },
        ],
        "busses": [{"id": 7, "name": "Verb", "format": "Stereo"}],
        "groups": [{"id": 8, "name": "Drums", "letter": "a", "members": [3], "active": true}],
        "markers": [{"id": 9, "name": "Intro", "kind": "Marker", "start": 0, "end": null}],
    })
}

/// The session as a look shows it with one object gone.
fn without(session: &Value, kind: Kind, id: u64) -> Value {
    let mut gone = session.clone();
    let key = match kind {
        Kind::Track => "tracks",
        Kind::Bus => "busses",
        Kind::Group => "groups",
        Kind::Marker => "markers",
        Kind::Source => "sources",
        Kind::Clip => {
            for track in gone["tracks"].as_array_mut().unwrap() {
                track["clips"].as_array_mut().unwrap().retain(|clip| clip["id"] != id);
            }
            return gone;
        }
    };
    gone[key].as_array_mut().unwrap().retain(|item| item["id"] != id);
    gone
}

#[test]
fn who_an_object_is_decides_its_life_while_its_there_and_all_it_holds_when_its_back() {
    // (what changes, the object, the change, whether that's a new life while it's there, and back after a look without it)
    let cases: &[(&str, Kind, u64, fn(&mut Value), bool, bool)] = &[
        ("nothing", Kind::Track, 3, |_| {}, false, false),
        ("a track's kind", Kind::Track, 3, |s| s["tracks"][0]["kind"] = json!("midi"), true, true),
        ("a track's format", Kind::Track, 3, |s| s["tracks"][0]["format"] = json!("Stereo"), true, true),
        ("a track's name", Kind::Track, 3, |s| s["tracks"][0]["name"] = json!("Kick In"), true, true),
        ("a track's instrument", Kind::Track, 5, |s| s["tracks"][1]["instrument"] = json!("sampler"), true, true),
        ("a track's clips", Kind::Track, 3, |s| s["tracks"][0]["clips"][0]["start"] = json!(480), false, true),
        ("a track's inserts", Kind::Track, 3, |s| s["tracks"][0]["inserts"][0]["plugin"] = json!("compressor"), false, true),
        ("an audio clip's name", Kind::Clip, 4, |s| s["tracks"][0]["clips"][0]["name"] = json!("Kick 2"), true, true),
        ("an audio clip's source", Kind::Clip, 4, |s| s["sources"][0]["name"] = json!("Snare.wav"), true, true),
        ("an audio clip's start", Kind::Clip, 4, |s| s["tracks"][0]["clips"][0]["start"] = json!(480), false, true),
        ("an audio clip's length", Kind::Clip, 4, |s| s["tracks"][0]["clips"][0]["length"] = json!(24000), false, true),
        ("an audio clip's offset", Kind::Clip, 4, |s| s["tracks"][0]["clips"][0]["offset"] = json!(480), false, true),
        ("an audio clip's track", Kind::Clip, 4, |s| s["tracks"][0]["name"] = json!("Kick In"), false, true),
        ("a MIDI clip's start", Kind::Clip, 6, |s| s["tracks"][1]["clips"][0]["start"] = json!(480), true, true),
        ("a MIDI clip's length", Kind::Clip, 6, |s| s["tracks"][1]["clips"][0]["length"] = json!(48000), true, true),
        ("a MIDI clip's notes", Kind::Clip, 6, |s| s["tracks"][1]["clips"][0]["notes"] = json!(5), false, true),
        ("a bus's name", Kind::Bus, 7, |s| s["busses"][0]["name"] = json!("Delay"), true, true),
        ("a bus's format", Kind::Bus, 7, |s| s["busses"][0]["format"] = json!("Mono"), true, true),
        ("a group's name", Kind::Group, 8, |s| s["groups"][0]["name"] = json!("Kit"), true, true),
        ("a group's members", Kind::Group, 8, |s| s["groups"][0]["members"] = json!([3, 5]), false, true),
        ("a group's member, renamed", Kind::Group, 8, |s| s["tracks"][0]["name"] = json!("Kick In"), false, true),
        ("a marker's name", Kind::Marker, 9, |s| s["markers"][0]["name"] = json!("Verse"), true, true),
        ("a marker's place", Kind::Marker, 9, |s| s["markers"][0]["start"] = json!(48000), false, true),
        ("a source's frames", Kind::Source, 2, |s| s["sources"][0]["frames"] = json!(96000), true, true),
        ("a source's file", Kind::Source, 2, |s| s["sources"][0]["path"] = json!("/elsewhere/Kick.wav"), false, true),
    ];
    for &(what, kind, id, change, new_while_there, new_when_back) in cases {
        let before = session();
        let mut after = before.clone();
        change(&mut after);
        let life = |new: bool| if new { Err(Missing::Stale) } else { Ok(id) };
        let mut lives = Lives::new();
        lives.saw_session(&before);
        let was = lives.now(kind, id).unwrap();
        lives.saw_session(&after);
        assert_eq!(lives.resolve(&was), life(new_while_there), "{what}, while it's there");
        let mut lives = Lives::new();
        lives.saw_session(&before);
        lives.saw_session(&without(&before, kind, id));
        lives.saw_session(&after);
        assert_eq!(lives.resolve(&was), life(new_when_back), "{what}, back after a look without it");
    }
}

#[test]
fn soundcraft_0_17_0s_session_gives_every_object_a_first_life_and_a_clip_takes_its_track_with_it() {
    let recorded = vectors("soundcraft-0.17.0.ndjson");
    let full = recorded.iter().find(|exchange| exchange["method"] == "session.inspect" && exchange["params"]["detail"] == "full").unwrap();
    let session = &full["reply"]["result"];
    let mut lives = Lives::new();
    lives.saw_session(session);
    let ids = |key: &str| session[key].as_array().unwrap().iter().map(|item| item["id"].as_u64().unwrap()).collect::<Vec<_>>();
    for (kind, key) in [(Kind::Track, "tracks"), (Kind::Bus, "busses"), (Kind::Marker, "markers"), (Kind::Source, "sources")] {
        let seen = ids(key);
        assert!(!seen.is_empty(), "{key}");
        for id in seen {
            assert_eq!(lives.now(kind, id), Some(Id { kind, id, life: 1 }), "{key} {id}");
        }
    }
    let first_track = &session["tracks"][0];
    let clip = first_track["clips"][0]["id"].as_u64().unwrap();
    assert_eq!(lives.now(Kind::Clip, clip).map(|id| id.life), Some(1));
    // The same clip on another track, with the same id, after it was gone: another object.
    let mut moved = session.clone();
    let taken = moved["tracks"][0]["clips"].as_array_mut().unwrap().remove(0);
    lives.saw_session(&moved);
    assert_eq!(lives.now(Kind::Clip, clip), None);
    moved["tracks"][1]["clips"].as_array_mut().unwrap().push(taken);
    lives.saw_session(&moved);
    assert_eq!(lives.now(Kind::Clip, clip).map(|id| id.life), Some(2));
}

#[test]
fn a_note_ref_finds_its_note_by_start_pitch_and_channel_and_two_alike_are_a_tie() {
    let note = |start: i64, pitch: u8, channel: u8, length: i64| json!({"start": start, "pitch": pitch, "channel": channel, "length": length, "velocity": 100});
    // Sorted as SoundCraft sorts them: start, pitch, length.
    let notes = [note(0, 60, 0, 480), note(480, 60, 0, 240), note(480, 60, 0, 480), note(480, 60, 1, 480), note(960, 64, 0, 480)];
    assert_eq!(note_index(&notes, 0, 60, 0), Ok(0));
    assert_eq!(note_index(&notes, 480, 60, 1), Ok(3), "the channel tells it apart");
    assert_eq!(note_index(&notes, 960, 64, 0), Ok(4));
    assert_eq!(note_index(&notes, 480, 60, 0), Err(NoteMissing::Tie(2)), "SoundCraft keeps both: which one isn't known");
    assert_eq!(note_index(&notes, 240, 60, 0), Err(NoteMissing::Gone));
}
