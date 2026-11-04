//! `focus_body` against a fake SoundCraft that shows the session and windows SoundCraft 0.17.0 showed (tests/vectors):
//! the Edit window in quarter notes, paged by track around the producer's or by cursor; the MIDI Editor's clip and its
//! notes; a track's plug-ins; the Mix window's strips. A body refused before the app is asked when it can be, one in beats
//! refused for a session with no beats, and refs from another epoch or an ended life refused as stale.
use super::fake::{ok, Answer, Fake};
use super::read::{misplaced, plugins, recorded};
use super::vectors::vectors;
use serde_json::{json, Value};
use soundcraft_kumi::control::Control;
use soundcraft_kumi::read::{content, Reads};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A fake SoundCraft showing the recorded session, whose MIDI clips have `notes`, and which describes its built-in
/// plug-ins as 0.17.0 does; and the windows it showed (`ui.inspect`).
struct App {
    fake: Fake,
    session: Arc<Mutex<Value>>,
    reads: Reads,
    ui: Value,
}

impl App {
    async fn new(notes: Value) -> App {
        let session = Arc::new(Mutex::new(recorded()));
        let (shown, described) = (session.clone(), plugins());
        let fake = Fake::start(16, move |request| match (request["method"].as_str(), request["params"]["command"].as_str()) {
            (Some("session.inspect"), _) => Answer::Now(ok(shown.lock().unwrap().clone())),
            (Some("engine.execute"), Some("midi.notes")) => Answer::Now(ok(json!({"notes": notes}))),
            (Some("engine.execute"), Some("engine.plugins")) => Answer::Now(ok(described.clone())),
            _ => Answer::Now(json!({"ok": false, "error": "unknown method `x`"})),
        })
        .await;
        let ui = vectors("soundcraft-0.17.0.ndjson").into_iter().find(|exchange| exchange["method"] == "ui.inspect").unwrap()["reply"]
            ["result"]
            .clone();
        App { reads: Reads::new(Control::new(fake.port(), None)), fake, session, ui }
    }

    async fn body(&mut self, params: Value) -> Result<Value, (&'static str, String)> {
        self.reads.focus_body(&params, Some(&self.ui), Instant::now() + Duration::from_secs(5)).await
    }

    /// The app shows the session changed by `change`, and a look sees it (so the next body asks for it again).
    fn shows(&mut self, change: impl FnOnce(&mut Value)) {
        let mut session = self.session.lock().unwrap();
        change(&mut session);
        self.reads.saw(&session);
    }

    fn asked(&self) -> usize {
        self.fake.requests().len()
    }

    /// How often the app was asked for `method`.
    fn asked_for(&self, method: &str) -> usize {
        self.fake.requests().iter().filter(|request| request["method"] == method).count()
    }
}

fn refs(rows: &Value) -> Vec<&str> {
    rows.as_array().unwrap().iter().map(|row| row["ref"].as_str().unwrap()).collect()
}

fn note(start: i64, pitch: u8, length: i64) -> Value {
    json!({"start": start, "pitch": pitch, "channel": 0, "length": length, "velocity": 100, "release_velocity": 64})
}

/// Quarter notes at 104 bpm and 48 kHz, the recorded session's, to a thousandth.
fn quarters(samples: f64) -> f64 {
    (samples / 48000.0 * 104.0 / 60.0 * 1000.0).round() / 1000.0
}

#[tokio::test]
async fn the_edit_body_is_the_edit_window_in_quarter_notes() {
    let mut app = App::new(json!([])).await;
    let body = app.body(json!({"kind": "edit"})).await.unwrap();
    assert_eq!(misplaced(&body), Vec::<String>::new(), "{body}");
    assert_eq!((&body["kind"], &body["epoch"]), (&json!("edit"), &json!(app.reads.epoch())));
    let length = quarters(2682432.0);
    assert_eq!((&body["length"], &body["window"]), (&json!(length), &json!({"from": 0.0, "to": length})));
    assert_eq!((&body["trackCount"], &body["from"], &body["beatsPerBar"]), (&json!(9), &json!(0), &json!(4.0)));
    assert!(body.get("nextCursor").is_none());
    assert_eq!(
        refs(&body["tracks"]),
        ["s:track:3", "s:track:6", "s:track:9", "s:track:12", "s:track:15", "s:track:23", "s:track:24", "s:track:27", "s:track:28"]
    );
    let kick = &body["tracks"][0];
    assert_eq!(
        (&kick["name"], &kick["type"], &kick["depth"], &kick["shown"], &kick["selected"]),
        (&json!("Kick"), &json!("audio"), &json!(0), &json!(true), &json!(true))
    );
    assert_eq!(
        kick["items"],
        json!([
            {"ref": "s:clip:4", "position": 16.0, "length": 32.0, "name": "Kick", "muted": false, "selected": false},
            {"ref": "s:clip:18", "position": 48.0, "length": 32.0, "name": "Kick", "muted": false, "selected": false},
            {"ref": "s:clip:20", "position": 80.0, "length": quarters(467047.0), "name": "Kick", "muted": false, "selected": false},
        ])
    );
    let keys = &body["tracks"][5];
    assert_eq!((&keys["type"], &keys["items"][0]["position"], &keys["items"][0]["length"]), (&json!("midi"), &json!(16.0), &json!(80.0)));
    // The playhead (the app's, as the last look saw it), and no selected time: the insertion point alone.
    assert_eq!((&body["playhead"], &body["playing"], &body["timeSelection"]), (&json!(quarters(443072.0)), &json!(false), &Value::Null));
    // The Edit window shows its timeline's 1046 pixels from the scroll, at the session's zoom.
    let shown = quarters(1046.0 * 1967.4514285714286);
    assert_eq!(body["view"], json!({"from": 0.0, "to": shown}));
    // Point markers only, in the recorded session (each one's end is its start).
    let marks: Vec<(&Value, &Value)> = body["markers"].as_array().unwrap().iter().map(|mark| (&mark["name"], &mark["position"])).collect();
    assert_eq!(
        marks,
        [
            (&json!("Intro"), &json!(0.0)),
            (&json!("Verse"), &json!(16.0)),
            (&json!("Chorus"), &json!(48.0)),
            (&json!("Outro"), &json!(80.0))
        ]
    );
    assert_eq!(body["markers"][0]["ref"], "s:marker:29");
    assert_eq!(body["regions"], json!([]));
    assert_eq!(body["meter"], json!([{"position": 0.0, "bar": 1, "beat": 0.0, "beatsPerBar": 4.0}]));
    assert!(body.get("loop").is_none(), "SoundCraft's session shows no loop");
    assert!(body["tracks"].as_array().unwrap().iter().all(|track| track["shown"] == true), "the Edit window draws every track");
    // Drawn from the summary, which holds all a view reads, never the full inspection.
    let inspections: Vec<Value> = app
        .fake
        .requests()
        .into_iter()
        .filter(|request| request["method"] == "session.inspect")
        .map(|request| request["params"].clone())
        .collect();
    assert_eq!(inspections, [json!({})]);
}

#[tokio::test]
async fn a_view_is_drawn_from_the_last_looks_summary_while_it_is_fresh() {
    let mut app = App::new(json!([])).await;
    let summary = Arc::new(app.session.lock().unwrap().clone());
    app.reads.looked_at(summary.clone(), content(&summary));
    for kind in ["edit", "mixer", "fx"] {
        app.body(json!({"kind": kind, "ref": if kind == "fx" { json!("s:track:3") } else { Value::Null }})).await.unwrap();
    }
    assert_eq!(app.asked_for("session.inspect"), 0, "the look's summary served them");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    app.body(json!({"kind": "mixer"})).await.unwrap();
    assert_eq!(app.asked_for("session.inspect"), 1, "a summary a second old is asked for again");
}

#[tokio::test]
async fn a_track_inside_a_closed_folder_is_not_shown() {
    let mut app = App::new(json!([])).await;
    // The Edit window draws its rows: Snare (6) is inside a folder the producer closed.
    app.ui["edit_layout"]["rows"].as_array_mut().unwrap().retain(|row| row[0] != 6);
    let body = app.body(json!({"kind": "edit"})).await.unwrap();
    let shown: Vec<(&Value, &Value)> = body["tracks"].as_array().unwrap().iter().map(|track| (&track["ref"], &track["shown"])).collect();
    assert_eq!(
        (shown[0], shown[1], shown[2]),
        ((&json!("s:track:3"), &json!(true)), (&json!("s:track:6"), &json!(false)), (&json!("s:track:9"), &json!(true)))
    );
    assert_eq!(body["tracks"][1]["depth"], 0, "inspect names no folder");
    // A hidden track isn't shown, whatever the rows; no rows at all (a window never drawn) leave the hidden flag alone.
    app.ui["edit_layout"]["rows"] = json!([]);
    app.shows(|session| session["tracks"][2]["hidden"] = json!(true));
    let body = app.body(json!({"kind": "edit"})).await.unwrap();
    let hidden: Vec<&Value> =
        body["tracks"].as_array().unwrap().iter().filter(|track| track["shown"] == false).map(|track| &track["ref"]).collect();
    assert_eq!(hidden, [&json!("s:track:9")]);
}

#[tokio::test]
async fn the_edit_body_shows_the_window_asked_for_with_its_regions_and_the_selected_time() {
    let mut app = App::new(json!([])).await;
    app.shows(|session| {
        // (a memory location of a selection: a region from bar 9 to bar 13)
        session["markers"][1]["end"] = json!(886154);
        session["markers"][1]["kind"] = json!("Selection");
        // (a plain marker given an end, as `markers.add {start, end}` makes one: still a point on the ruler)
        session["markers"][2]["end"] = json!(1772308);
        session["selection"] = json!({"tracks": [3], "clips": [4], "start": 443077, "end": 886154});
        session["meter"] = json!([{"denominator": 4, "numerator": 4, "tick": 0}, {"denominator": 4, "numerator": 3, "tick": 48 * 960}]);
    });
    let body = app.body(json!({"kind": "edit", "window": {"from": 10.0, "to": 60.0}})).await.unwrap();
    assert_eq!(body["window"], json!({"from": 10.0, "to": 60.0}));
    assert_eq!(refs(&body["tracks"][0]["items"]), ["s:clip:4", "s:clip:18"], "the clips that touch the window");
    assert_eq!(body["tracks"][0]["items"][0]["selected"], true);
    assert_eq!(body["timeSelection"], json!({"start": 16.0, "end": 32.0}));
    assert_eq!(body["regions"], json!([{"ref": "s:marker:30", "number": 2, "name": "Verse", "position": 16.0, "end": 32.0}]));
    assert_eq!(refs(&body["markers"]), ["s:marker:31"], "Chorus, at 48, a point");
    assert!(body["markers"][0].get("end").is_none());
    assert_eq!(
        body["meter"],
        json!([{"position": 10.0, "bar": 3, "beat": 2.0, "beatsPerBar": 4.0}, {"position": 48.0, "bar": 13, "beat": 0.0, "beatsPerBar": 3.0}])
    );
    for window in [json!({"from": 5.0, "to": 5.0}), json!({"from": 9.0, "to": 2.0}), json!({"from": 0}), json!("all")] {
        let refused = app.body(json!({"kind": "edit", "window": window})).await.unwrap_err();
        assert_eq!(refused.0, "bad-params", "{window}: {refused:?}");
    }
}

#[tokio::test]
async fn the_meter_marks_past_the_cap_are_counted() {
    let mut app = App::new(json!([])).await;
    // A 4/4 bar, 205 times over: 204 changes after the first.
    let meters: Vec<Value> = (0..205).map(|bar| json!({"denominator": 4, "numerator": 4, "tick": bar * 3840})).collect();
    app.shows(|session| session["meter"] = meters.into());
    let body = app.body(json!({"kind": "edit", "window": {"from": 0.0, "to": 1000.0}})).await.unwrap();
    assert_eq!((body["meter"].as_array().unwrap().len(), &body["meterOmitted"]), (200, &json!(5)));
}

#[tokio::test]
async fn the_midi_editors_clip_is_found_among_thousands_of_selected_clips_at_once() {
    let mut app = App::new(json!([])).await;
    // 12,800 audio clips on Hats, every one selected, and Keys' MIDI clip selected last.
    let clips: Vec<Value> = (0..12_800)
        .map(|n| json!({"id": 100_000 + n, "name": "Hat", "start": n * 100, "length": 100, "source": 8, "offset": 0, "muted": false}))
        .collect();
    let mut selected: Vec<Value> = (0..12_800).map(|n| json!(100_000 + n)).collect();
    selected.push(json!(25));
    app.shows(|session| {
        session["tracks"][2]["clips"] = clips.into();
        session["selection"]["clips"] = selected.into();
    });
    let began = Instant::now();
    assert_eq!(app.reads.midi_editor_clip(&app.session.lock().unwrap()), Some("s:clip:25".to_string()));
    assert!(began.elapsed() < Duration::from_secs(1), "{:?}", began.elapsed());
}

#[tokio::test]
async fn the_edit_window_pages_by_track_around_the_producers_and_by_cursor() {
    let mut app = App::new(json!([])).await;
    let around = app.body(json!({"kind": "edit", "around": "s:track:23", "limit": 4})).await.unwrap();
    assert_eq!(refs(&around["tracks"]), ["s:track:12", "s:track:15", "s:track:23", "s:track:24"], "Keys, half a page down");
    assert_eq!((&around["from"], &around["trackCount"]), (&json!(3), &json!(9)));
    let cursor = around["nextCursor"].as_str().unwrap().to_string();
    // A cursor goes on from where the page stopped, whatever `around` says; the rest of the window is on the first page.
    let next = app.body(json!({"kind": "edit", "around": "s:track:3", "limit": 4, "cursor": cursor})).await.unwrap();
    assert_eq!((refs(&next["tracks"]), &next["from"]), (vec!["s:track:27", "s:track:28"], &json!(7)));
    assert!(next.get("nextCursor").is_none() && next.get("playhead").is_none() && next.get("markers").is_none(), "{next}");
    // Near the end the last page is full; near the start, the first page.
    assert_eq!(app.body(json!({"kind": "edit", "around": "s:track:28", "limit": 4})).await.unwrap()["from"], 5);
    assert_eq!(app.body(json!({"kind": "edit", "around": "s:track:6", "limit": 4})).await.unwrap()["from"], 0);
    assert_eq!(app.body(json!({"kind": "edit", "limit": 4})).await.unwrap()["from"], 0);
    let refused = app.body(json!({"kind": "edit", "limit": 0})).await.unwrap_err();
    assert_eq!(refused.0, "bad-params");
    assert_eq!(app.body(json!({"kind": "edit", "around": "s:clip:4"})).await.unwrap_err().0, "bad-params", "around a track");
}

#[tokio::test]
async fn the_midi_body_is_the_clip_the_midi_editor_shows_with_its_notes_in_quarter_notes() {
    let mut app = App::new(json!([note(0, 60, 480), note(960, 62, 240)])).await;
    // Lead gets a second clip, before its first: where the MIDI Editor's choice shows.
    app.shows(|session| {
        let lead = session["tracks"].as_array_mut().unwrap().iter_mut().find(|track| track["id"] == 24).unwrap();
        lead["clips"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"id": 40, "name": "Lead Intro", "start": 443077, "length": 443077, "notes": 2, "muted": false}));
    });
    let refused = app.body(json!({"kind": "midi"})).await.unwrap_err();
    assert_eq!(refused.0, "bad-params", "Kick is selected: the MIDI Editor shows no clip ({})", refused.1);
    let editor_shows = |app: &mut App, selection: Value| {
        app.shows(|session| session["selection"] = selection);
    };
    // The first clip of a selected MIDI or instrument track when none is under the insertion point.
    editor_shows(&mut app, json!({"tracks": [24], "clips": [], "start": 0, "end": 0}));
    assert_eq!(app.body(json!({"kind": "midi"})).await.unwrap()["ref"], "s:clip:40");
    // The one under the insertion point, or in the selection.
    editor_shows(&mut app, json!({"tracks": [24], "clips": [], "start": 1400000, "end": 1400000}));
    assert_eq!(app.body(json!({"kind": "midi"})).await.unwrap()["ref"], "s:clip:26");
    editor_shows(&mut app, json!({"tracks": [24], "clips": [], "start": 0, "end": 1400000}));
    assert_eq!(app.body(json!({"kind": "midi"})).await.unwrap()["ref"], "s:clip:40", "the first that overlaps it");
    // Before them all, the first selected MIDI clip (Kick's clip is audio).
    editor_shows(&mut app, json!({"tracks": [24], "clips": [4, 25], "start": 1400000, "end": 1400000}));
    let body = app.body(json!({"kind": "midi"})).await.unwrap();
    assert_eq!(misplaced(&body), Vec::<String>::new(), "{body}");
    assert_eq!(
        (&body["kind"], &body["ref"], &body["trackRef"], &body["name"], &body["length"], &body["beatsPerBar"], &body["noteCount"]),
        (&json!("midi"), &json!("s:clip:25"), &json!("s:track:23"), &json!("Keys"), &json!(80.0), &json!(4.0), &json!(2))
    );
    assert_eq!(
        body["notes"],
        json!([
            {"ref": "s:note:25:0.60.0", "pitch": 60, "start": 0.0, "length": 0.5, "velocity": 100, "channel": 0, "selected": false},
            {"ref": "s:note:25:960.62.0", "pitch": 62, "start": 1.0, "length": 0.25, "velocity": 100, "channel": 0, "selected": false},
        ])
    );
    // A clip by its ref, a page at a time.
    let first = app.body(json!({"kind": "midi", "ref": "s:clip:26", "limit": 1})).await.unwrap();
    assert_eq!((&first["ref"], refs(&first["notes"])), (&json!("s:clip:26"), vec!["s:note:26:0.60.0"]));
    let next = app.body(json!({"kind": "midi", "ref": "s:clip:26", "cursor": first["nextCursor"]})).await.unwrap();
    assert_eq!(refs(&next["notes"]), ["s:note:26:960.62.0"]);
    assert!(next.get("nextCursor").is_none());
    let audio = app.body(json!({"kind": "midi", "ref": "s:clip:4"})).await.unwrap_err();
    assert_eq!(audio, ("bad-params", "ref: s:clip:4 isn't a MIDI clip, so it has no notes".to_string()));
    assert_eq!(app.body(json!({"kind": "midi", "ref": "s:track:23"})).await.unwrap_err().0, "bad-params", "a clip's ref");
}

#[tokio::test]
async fn the_fx_body_is_a_tracks_instrument_then_its_inserts_by_their_names() {
    let mut app = App::new(json!([])).await;
    let keys = app.body(json!({"kind": "fx", "ref": "s:track:23"})).await.unwrap();
    assert_eq!(misplaced(&keys), Vec::<String>::new(), "{keys}");
    assert_eq!(
        (&keys["kind"], &keys["ref"], &keys["name"], &keys["nodes"]),
        (
            &json!("fx"),
            &json!("s:track:23"),
            &json!("Keys"),
            &json!([{"ref": "s:insert:23:instrument", "name": "Subtractive Synth", "plugin": true}])
        )
    );
    app.shows(|session| session["tracks"][4]["inserts"][0]["bypass"] = json!(true));
    let pad = app.body(json!({"kind": "fx", "ref": "s:insert:15:a"})).await.unwrap();
    assert_eq!(
        (&pad["ref"], &pad["nodes"]),
        (&json!("s:track:15"), &json!([{"ref": "s:insert:15:a", "name": "Chorus", "enabled": false, "plugin": true}]))
    );
    // By default the track of the plug-in window opened last, else the first selected track.
    app.ui["ui"]["plugin_windows"] = json!([[15, 0], [28, 0]]);
    let master = app.body(json!({"kind": "fx"})).await.unwrap();
    assert_eq!(
        (&master["ref"], &master["nodes"]),
        (&json!("s:track:28"), &json!([{"ref": "s:insert:28:a", "name": "Maximizer", "enabled": true, "plugin": true}]))
    );
    app.ui["ui"]["plugin_windows"] = json!([]);
    let kick = app.body(json!({"kind": "fx"})).await.unwrap();
    assert_eq!(
        (&kick["ref"], &kick["nodes"][0]["name"], &kick["nodes"][1]["name"]),
        (&json!("s:track:3"), &json!("EQ 7-Band"), &json!("Compressor/Limiter"))
    );
    assert_eq!(refs(&kick["nodes"]), ["s:insert:3:a", "s:insert:3:b"]);
    app.shows(|session| session["selection"]["tracks"] = json!([]));
    assert_eq!(app.body(json!({"kind": "fx"})).await.unwrap_err().0, "bad-params");
    assert_eq!(app.body(json!({"kind": "fx", "ref": "s:clip:4"})).await.unwrap_err().0, "bad-params");
}

#[tokio::test]
async fn the_mixer_body_is_the_mix_windows_strips_with_the_master_faders_first() {
    let mut app = App::new(json!([])).await;
    let body = app.body(json!({"kind": "mixer"})).await.unwrap();
    assert_eq!(misplaced(&body), Vec::<String>::new(), "{body}");
    assert_eq!(
        (&body["kind"], &body["levels"], &body["trackCount"], &body["from"]),
        (&json!("mixer"), &json!(false), &json!(8), &json!(0))
    );
    assert_eq!(
        refs(&body["strips"]),
        ["s:track:28", "s:track:3", "s:track:6", "s:track:9", "s:track:12", "s:track:15", "s:track:23", "s:track:24", "s:track:27"]
    );
    assert_eq!(
        body["strips"][0],
        json!({"ref": "s:track:28", "name": "Master 1", "type": "master", "master": true, "volumeDb": 0.0, "pan": 0.0, "mute": false, "solo": false, "selected": false})
    );
    assert_eq!(
        body["strips"][1],
        json!({"ref": "s:track:3", "name": "Kick", "type": "audio", "arm": false, "volumeDb": body["strips"][1]["volumeDb"], "pan": body["strips"][1]["pan"], "mute": false, "solo": false, "selected": true})
    );
    let strip = |at: usize| (body["strips"][at]["name"].clone(), body["strips"][at]["type"].clone(), body["strips"][at]["pan"].clone());
    assert_eq!(strip(3), (json!("Hats"), json!("audio"), json!(0.35)), "a mono track's one pan");
    assert_eq!(strip(5), (json!("Pad"), json!("audio"), json!(0.0)), "the middle of a stereo track's two");
    assert_eq!(strip(6).1, json!("midi"), "in Kumi's chip words: an instrument track plays MIDI");
    assert_eq!(strip(8).1, json!("send"), "an aux track is a return");
    assert!(body["strips"].as_array().unwrap().iter().all(|strip| strip.get("peakDb").is_none()), "SoundCraft shows no meters");
    // A page of four around Keys, the master fader still first; then the rest by cursor.
    let around = app.body(json!({"kind": "mixer", "around": "s:track:23", "limit": 4})).await.unwrap();
    assert_eq!(refs(&around["strips"]), ["s:track:28", "s:track:12", "s:track:15", "s:track:23", "s:track:24"]);
    assert_eq!(around["from"], 3);
    let next = app.body(json!({"kind": "mixer", "limit": 4, "cursor": around["nextCursor"]})).await.unwrap();
    assert_eq!((refs(&next["strips"]), &next["from"]), (vec!["s:track:28", "s:track:27"], &json!(7)));
    // A hidden track has no strip.
    app.shows(|session| session["tracks"][2]["hidden"] = json!(true));
    let hidden = app.body(json!({"kind": "mixer"})).await.unwrap();
    assert_eq!(hidden["trackCount"], 7);
    assert!(!refs(&hidden["strips"]).contains(&"s:track:9"));
    assert_eq!(app.body(json!({"kind": "mixer", "limit": 65})).await.unwrap_err().0, "bad-params");
    // Master faders past the 64th are counted.
    app.shows(|session| {
        for n in 0..65 {
            let master = json!({"id": 1000 + n, "kind": "master", "format": "Stereo", "name": format!("Master {n}"), "instrument": null, "inserts": [], "clips": [], "pan": [-1.0, 1.0]});
            session["tracks"].as_array_mut().unwrap().push(master);
        }
    });
    let masters = app.body(json!({"kind": "mixer", "limit": 1})).await.unwrap();
    assert_eq!((masters["strips"].as_array().unwrap().len(), &masters["mastersOmitted"]), (65, &json!(2)));
}

#[tokio::test]
async fn a_body_soundcraft_has_none_of_or_a_bad_ref_is_refused_before_the_app_is_asked() {
    let mut app = App::new(json!([])).await;
    for params in [
        json!({"kind": "routing"}),
        json!({}),
        json!({"kind": "fx", "ref": "track 3"}),
        json!({"kind": "midi", "ref": "s:insert:3:a"}),
        json!({"kind": "mixer", "around": "s:clip:4"}),
        json!({"kind": "edit", "cursor": "c1.nope"}),
    ] {
        let refused = app.body(params.clone()).await.unwrap_err();
        assert_eq!(refused.0, "bad-params", "{params}: {refused:?}");
    }
    assert_eq!(app.asked(), 0, "the app wasn't asked");
}

#[tokio::test]
async fn a_session_with_no_beats_has_no_edit_or_midi_body() {
    let mut app = App::new(json!([note(0, 60, 480)])).await;
    // A tempo map with a ramp in it, which SoundCraft 0.17.0's maps don't hold.
    app.shows(|session| session["tempo"] = json!([{"bpm": 104.0, "tick": 0, "curve": "linear"}]));
    for kind in ["edit", "midi"] {
        let refused =
            app.body(json!({"kind": kind, "ref": if kind == "midi" { json!("s:clip:25") } else { Value::Null }})).await.unwrap_err();
        assert_eq!(refused.0, "failed", "{kind}: {refused:?}");
        assert!(refused.1.contains("can't count its beats"), "{kind}: {}", refused.1);
    }
    assert!(app.body(json!({"kind": "mixer"})).await.is_ok(), "the mixer has no beats to count");
    assert!(app.body(json!({"kind": "fx", "ref": "s:track:23"})).await.is_ok());
}

#[tokio::test]
async fn refs_from_another_epoch_or_an_ended_life_are_stale_and_without_an_epoch_refs_are_read_in_this_one() {
    let mut app = App::new(json!([])).await;
    let epoch = app.body(json!({"kind": "mixer"})).await.unwrap()["epoch"].as_str().unwrap().to_string();
    assert!(app.body(json!({"kind": "fx", "ref": "s:track:15", "epoch": epoch})).await.is_ok(), "this epoch's");
    assert!(app.body(json!({"kind": "fx", "ref": "s:track:15"})).await.is_ok(), "no epoch: read in this one");
    // The bridge started again, SoundCraft still open: a table of its own.
    let before = Reads::new(Control::new(app.fake.port(), None)).epoch().to_string();
    let stale = app.body(json!({"kind": "fx", "ref": "s:track:15", "epoch": before})).await.unwrap_err();
    assert_eq!(stale.0, "stale", "{stale:?}");
    assert_eq!(app.body(json!({"kind": "fx", "ref": "s:track:999"})).await.unwrap_err().0, "stale", "not of this session");
    // Kick undone, and another track made on its id: Kick's life has ended, epoch or none.
    let kick = app.session.lock().unwrap()["tracks"][0].clone();
    app.shows(|session| {
        session["tracks"].as_array_mut().unwrap().remove(0);
    });
    let mut snare = kick;
    snare["name"] = json!("Snare 2");
    snare["clips"] = json!([]);
    app.shows(|session| session["tracks"].as_array_mut().unwrap().insert(0, snare));
    for params in [
        json!({"kind": "fx", "ref": "s:track:3"}),
        json!({"kind": "edit", "around": "s:track:3"}),
        json!({"kind": "mixer", "around": "s:track:3", "epoch": epoch}),
    ] {
        let refused = app.body(params.clone()).await.unwrap_err();
        assert_eq!(refused.0, "stale", "{params}: {refused:?}");
    }
    assert_eq!(app.body(json!({"kind": "fx", "ref": "s:track:3.2"})).await.unwrap()["name"], "Snare 2", "its new life");
}
