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
        ("s:param:12:a:high_shelf_gain", Ref::Param { track: track(12, 1), slot: Slot::Insert(0), param: "high_shelf_gain".into() }),
        ("s:param:12:instrument:7", Ref::Param { track: track(12, 1), slot: Slot::Instrument, param: "7".into() }),
        ("s:env:12:volume", Ref::Lane { track: track(12, 1), param: "volume".into() }),
        ("s:env:12:b:hpf_freq", Ref::Lane { track: track(12, 1), param: "b:hpf_freq".into() }),
        ("s:point:12:volume:96000", Ref::Point { track: track(12, 1), param: "volume".into(), sample: 96000 }),
        ("s:point:12:b:hpf_freq:-480", Ref::Point { track: track(12, 1), param: "b:hpf_freq".into(), sample: -480 }),
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
        "s:note:40:480.60",
        "s:note:40:480.128.0",
        "s:note:40:480.60.16",
        "s:note:40:480.60.0.1",
        "s:point:12:volume",
        "s:playlist:12:-1",
    ] {
        let refused = text.parse::<Ref>().unwrap_err();
        assert_eq!(refused, format!("{text} isn't a SoundCraft ref (s:<kind>:<parts>, as a read gave it)"));
    }
}

fn tracks(lives: &mut Lives, now: &[(u64, &str)]) {
    let shown: Vec<Value> = now
        .iter()
        .map(|(id, name)| json!({"id": id, "kind": "audio", "format": "Mono", "name": name, "clips": [], "inserts": []}))
        .collect();
    lives.saw_session(&json!({"tracks": shown}));
}

#[test]
fn an_id_that_comes_back_on_another_object_is_a_new_life_and_the_old_ref_is_stale() {
    let mut lives = Lives::default();
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
fn an_object_an_undo_puts_back_as_it_was_keeps_its_life_however_it_changed_while_there() {
    let mut lives = Lives::default();
    tracks(&mut lives, &[(3, "Bass")]);
    let bass = lives.now(Kind::Track, 3).unwrap();
    tracks(&mut lives, &[(3, "Bass DI")]);
    assert_eq!(lives.resolve(&bass), Ok(3), "renamed, the same track");
    tracks(&mut lives, &[]);
    tracks(&mut lives, &[(3, "Bass DI")]);
    assert_eq!(lives.resolve(&bass), Ok(3), "deleted and put back by an undo: the same track");
    assert_eq!(lives.now(Kind::Track, 3), Some(bass));
}

#[test]
fn a_kumi_changes_object_that_an_undo_crossed_is_retired_at_once_even_coming_back_the_same() {
    let mut lives = Lives::default();
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
fn refs_from_another_session_or_a_later_life_than_seen_are_unknown() {
    let mut lives = Lives::default();
    tracks(&mut lives, &[(1, "First")]);
    assert_eq!(lives.resolve(&track(9, 1)), Err(Missing::Unknown));
    assert_eq!(lives.resolve(&track(1, 4)), Err(Missing::Unknown));
}

#[test]
fn soundcraft_0_17_0s_session_gives_every_object_a_first_life_and_a_clip_takes_its_track_with_it() {
    let recorded = vectors("soundcraft-0.17.0.ndjson");
    let full = recorded.iter().find(|exchange| exchange["method"] == "session.inspect" && exchange["params"]["detail"] == "full").unwrap();
    let session = &full["reply"]["result"];
    let mut lives = Lives::default();
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
