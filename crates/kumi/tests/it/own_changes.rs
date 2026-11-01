//! The own bridges' change engine against REAPER's and Renoise's own sets of changes (from own_changes' tests: the app is where both DAWs' integrations meet).
use async_trait::async_trait;
use kumi_agent::{
    core::{
        plan::PlanHost,
        refs::{Book, RefScheme},
    },
    integrations::shared::{changes::ChangeSummary, steps::own_step_line},
    mcp::client::McpEndpoint,
};
use kumi_common::abort::Signal;
use kumi_daw_own::bridge::OwnDaw;
use kumi_daw_own::changes::*;
use kumi_daw_reaper::changes as reaper_changes;
use kumi_daw_renoise::changes as renoise_changes;
use kumi_types::contracts::*;
use kumi_types::{daw::DawProfile, errors::RuntimeError};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
};

/// Live's words, whole: none may reach the model from another DAW's changes.
const LIVE_WORDS: &str = r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Remote Script|Control Surface|Main|Drum Rack|Operator|[Rr]acks?|[Cc]lips?|[Cc]lip slots?|[Ss]cenes?)\b";

fn sets() -> [(&'static str, &'static OwnChangeSet); 2] {
    [("REAPER", &reaper_changes::CHANGES), ("Renoise", &renoise_changes::CHANGES)]
}

/// The kinds a bridge's code registers, found with `pattern` in the files under `folder` whose names start with
/// `prefix`, less the kinds that only name a change's target.
fn registered(folder: &str, prefix: &str, pattern: &str) -> BTreeSet<String> {
    let folder = kumi_testkit::workspace_path(folder);
    let found = Regex::new(pattern).unwrap();
    let mut kinds = BTreeSet::new();
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().to_string_lossy().starts_with(prefix) {
            let text = std::fs::read_to_string(&path).unwrap();
            kinds.extend(found.captures_iter(&text).filter_map(|c| c.iter().skip(1).flatten().next().map(|m| m.as_str().to_string())));
        }
    }
    kinds.remove("pattern-track");
    kinds
}

#[test]
fn each_daws_kinds_are_exactly_what_its_bridge_makes() {
    let reaper =
        registered("daws/reaper/reaper-kumi/src/changes", "", r#"kind: "([a-z]+(?:-[a-z]+)+)"|"([a-z]+(?:-[a-z]+)+)" => Some\(Kind"#);
    let renoise = registered("daws/renoise/com.kumi.Bridge.xrnx/payload", "changes", r#"(?:kind = |register\()"([a-z]+(?:-[a-z]+)+)""#);
    for ((app, set), bridge) in sets().into_iter().zip([reaper, renoise]) {
        let ours: BTreeSet<String> = set.kinds.iter().map(|kind| kind.kind.to_string()).collect();
        assert_eq!(ours, bridge, "{app}: Kumi's kinds against its bridge's");
    }
}

/// Fields of Renoise's kinds that its tool never sees: the host makes those changes itself (a Doofer or Splitter from
/// the device's preset data, an instrument's macro mappings in its file) and hands the tool what it made. Each is read
/// in the host's file named.
const READ_BY_THE_HOST: &[(&str, &str, &str)] = &[
    ("doofer-macro", "activeMacros", "daws/own/daw-mcp-server/src/renoise/presets.rs"),
    ("doofer-macro", "mappings.lane", "daws/own/daw-mcp-server/src/renoise/presets.rs"),
    ("doofer-make", "activeMacros", "daws/own/daw-mcp-server/src/renoise/presets.rs"),
    ("doofer-make", "laneRefs", "daws/own/daw-mcp-server/src/renoise/presets.rs"),
    ("doofer-make", "macros", "daws/own/daw-mcp-server/src/renoise/presets.rs"),
    ("doofer-make", "macros.mappings.lane", "daws/own/daw-mcp-server/src/renoise/presets.rs"),
    ("doofer-make", "split", "daws/own/daw-mcp-server/src/renoise/presets.rs"),
    ("instrument-macro-map", "add", "daws/own/daw-mcp-server/src/renoise/instruments.rs"),
    ("instrument-macro-map", "add.modulation", "daws/own/daw-mcp-server/src/renoise/instruments.rs"),
];

/// Every field a schema offers, a nested one as `outer.inner` (inside an array's items or one of its shapes too).
fn schema_fields(schema: &Value, path: &str, fields: &mut Vec<String>) {
    let Some(schema) = schema.as_object() else { return };
    for (name, inner) in schema.get("properties").and_then(Value::as_object).into_iter().flatten() {
        let field = if path.is_empty() { name.clone() } else { format!("{path}.{name}") };
        schema_fields(inner, &field, fields);
        fields.push(field);
    }
    for key in ["items", "additionalProperties"] {
        if let Some(inner) = schema.get(key) {
            schema_fields(inner, path, fields);
        }
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        for inner in schema.get(key).and_then(Value::as_array).into_iter().flatten() {
            schema_fields(inner, path, fields);
        }
    }
}

/// A bridge's change code: the files under `folder` whose names start with `prefix`, less its tests' files.
fn change_code(folder: &str, prefix: &str) -> String {
    let mut code = String::new();
    for entry in std::fs::read_dir(kumi_testkit::workspace_path(folder)).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with(prefix) && !name.starts_with("tests") {
            code.push_str(&std::fs::read_to_string(&path).unwrap());
        }
    }
    code
}

/// (#399 K34) Each field a kind offers the model is read on the way to the DAW: by its bridge (REAPER's extension names
/// it as a string; Renoise's tool as a key or a string), or by the host for the fields in READ_BY_THE_HOST. A loose
/// check, the field named anywhere in the change code, but it finds a field that nothing reads, which the model would
/// fill for nothing.
#[test]
fn each_daws_bridge_reads_every_field_its_kinds_offer() {
    let reaper = change_code("daws/reaper/reaper-kumi/src/changes", "");
    let renoise = change_code("daws/renoise/com.kumi.Bridge.xrnx/payload", "changes");
    // Named as a string ("field"), or in Lua also as 'field' or a key (.field, not .fields).
    let named = |code: &str, field: &str, lua: bool| {
        code.contains(&format!("\"{field}\""))
            || lua
                && (code.contains(&format!("'{field}'"))
                    || code.match_indices(&format!(".{field}")).any(|(at, dotted)| {
                        !code[at + dotted.len()..].starts_with(|next: char| next.is_ascii_alphanumeric() || next == '_')
                    }))
    };
    let mut unread = Vec::new();
    for ((app, set), (code, lua)) in sets().into_iter().zip([(&reaper, false), (&renoise, true)]) {
        for kind in set.kinds {
            let schema: Value = serde_json::from_str(kind.schema).unwrap();
            let mut fields = Vec::new();
            schema_fields(&schema, "", &mut fields);
            for field in &fields {
                // (a top-level field may go to the bridge under another name)
                let wire = kind.wire.iter().find(|(ours, _)| *ours == field.as_str()).map_or(field.as_str(), |(_, theirs)| *theirs);
                let last = wire.rsplit('.').next().unwrap();
                let by_host = READ_BY_THE_HOST
                    .iter()
                    .find(|(host_kind, host_field, _)| app == "Renoise" && *host_kind == kind.kind && *host_field == field.as_str());
                if let Some((_, _, file)) = by_host {
                    let host = std::fs::read_to_string(kumi_testkit::workspace_path(file)).unwrap();
                    if !named(&host, last, false) {
                        unread.push(format!("{app}: {}'s {field} is said to be read by the host, but {file} never names it", kind.kind));
                    }
                } else if !named(code, last, lua) {
                    unread.push(format!("{app}: {}'s {field} is offered to the model, but {app}'s bridge never reads it", kind.kind));
                }
            }
        }
    }
    for (kind, field, _) in READ_BY_THE_HOST {
        let offered = renoise_changes::CHANGES.kinds.iter().find(|ours| ours.kind == *kind).is_some_and(|ours| {
            let mut fields = Vec::new();
            schema_fields(&serde_json::from_str(ours.schema).unwrap(), "", &mut fields);
            fields.iter().any(|offered| offered.as_str() == *field)
        });
        assert!(offered, "READ_BY_THE_HOST names Renoise's {kind}'s {field}, which its schema doesn't offer");
    }
    assert!(unread.is_empty(), "{}", unread.join("\n"));
}

#[test]
fn the_host_knows_the_kinds_each_daws_undo_never_takes_back_as_kumi_does() {
    for ((app, set), daw) in sets().into_iter().zip([daw_bridge::Daw::Reaper, daw_bridge::Daw::Renoise]) {
        let ours: BTreeSet<&str> = set.kinds.iter().filter(|kind| kind.skipped_by_daw_undo).map(|kind| kind.kind).collect();
        let host: BTreeSet<&str> = daw_mcp_server::tools::kinds(daw).outside_undo.iter().copied().collect();
        assert_eq!(ours, host, "{app}");
    }
}

/// (review U8) REAPER's note and MIDI event changes are drawn in NOW and HISTORY, from what their previews drew.
#[test]
fn reapers_midi_changes_have_a_picture() {
    let note = |pitch: u32, start: f64| json!({"index": 0, "pitch": pitch, "start": start, "length": 1.0, "velocity": 100});
    let event = |start: f64, value: i64| json!({"index": 0, "type": "cc", "channel": 1, "controller": 1, "value": value, "start": start});
    for (tool, before, after) in [
        ("notes_add", vec![], vec![note(36, 0.)]),
        ("notes_change", vec![note(36, 0.)], vec![note(38, 0.)]),
        ("notes_delete", vec![note(36, 0.)], vec![]),
        ("notes_quantize", vec![note(36, 0.1)], vec![note(36, 0.)]),
        ("notes_humanize", vec![note(36, 0.)], vec![note(36, 0.02)]),
        ("midi_events_add", vec![], vec![event(0., 64)]),
        ("midi_events_change", vec![event(0., 64)], vec![event(0., 100)]),
        ("midi_events_delete", vec![event(0., 64)], vec![]),
    ] {
        let kind = reaper_changes::CHANGES.kinds.iter().find(|kind| kind.tool == tool).unwrap();
        let preview = json!({"display": {"title": "t", "count": 1, "before": before, "after": after}});
        let summary = kind.summary("t", Some(&preview), Some(&json!({"applied": {}, "display": {"title": "t"}})));
        assert!(summary.picture.is_some(), "{tool}");
    }
}

fn engine<S: RefScheme + Default + 'static>(daw: &'static OwnDaw, file: Option<String>) -> Rc<OwnPlan<S>> {
    let link = Rc::new(Link {
        daw,
        endpoint: Rc::new(RefCell::new(None)),
        book: Rc::new(RefCell::new(Book::default())),
        epoch: Rc::default(),
        layout: Rc::default(),
        seen_in: Rc::default(),
        connected: Rc::default(),
        lifetime: Signal::new(),
        tempo: Rc::default(),
        file: Rc::new(RefCell::new(file)),
        step_titles: RefCell::default(),
        outside_daw_undo: RefCell::default(),
        undo_unanswered: RefCell::default(),
        in_plan: RefCell::default(),
        maybe_redone: RefCell::default(),
        put_back_by_daw: RefCell::default(),
        project: Rc::default(),
        made_in: RefCell::default(),
    });
    OwnPlan::new(link, None, None, None)
}

/// Review U18: a copy is named by when the project was saved in the producer's local time, with its offset, and the
/// oldest go by that time, whatever zone each copy was made in (or a copy named in UTC before).
#[test]
fn a_copy_is_named_by_local_save_time_and_the_oldest_go_by_time_not_by_name() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("Mix.rpp");
    std::fs::write(&project, "<REAPER_PROJECT").unwrap();
    let copies = folder.path();
    // Six kept already. In UTC: 00:30 (named without an offset), 01:00 (made in Tokyo), then 02:00 to 05:00.
    for name in [
        "Mix (Kumi copy 2026-10-20 00.30.00).rpp",
        "Mix (Kumi copy 2026-10-20 10.00.00 +0900).rpp",
        "Mix (Kumi copy 2026-10-20 02.00.00 +0000).rpp",
        "Mix (Kumi copy 2026-10-20 03.00.00 +0000).rpp",
        "Mix (Kumi copy 2026-10-20 04.00.00 +0000).rpp",
        "Mix (Kumi copy 2026-10-20 05.00.00 +0000).rpp",
    ] {
        std::fs::write(copies.join(name), "kept").unwrap();
    }
    let Copied::Made(copy) = copy_beside(&project) else { panic!("a copy") };
    let saved: chrono::DateTime<chrono::Local> = std::fs::metadata(&project).unwrap().modified().unwrap().into();
    let named = format!("Mix (Kumi copy {}).rpp", saved.format("%Y-%m-%d %H.%M.%S %z"));
    assert_eq!(Path::new(&copy).file_name().unwrap().to_string_lossy(), named, "local time, with its offset");
    let mut left: Vec<String> = std::fs::read_dir(copies)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("(Kumi copy "))
        .collect();
    left.sort();
    assert_eq!(left.len(), COPIES_KEPT);
    assert!(!left.contains(&"Mix (Kumi copy 2026-10-20 00.30.00).rpp".to_string()), "the oldest went: {left:?}");
    assert!(!left.contains(&"Mix (Kumi copy 2026-10-20 10.00.00 +0900).rpp".to_string()), "the next oldest (01:00 UTC) went: {left:?}");
    assert!(left.contains(&"Mix (Kumi copy 2026-10-20 02.00.00 +0000).rpp".to_string()), "a later one stays: {left:?}");
    assert!(project.exists(), "the project itself is never taken for a copy");
}

#[test]
fn a_delete_copies_the_saved_project_beside_it_once_per_saved_version_and_keeps_the_newest_few() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("Night Drive.rpp");
    std::fs::write(&project, "<REAPER_PROJECT").unwrap();
    let copies = || std::fs::read_dir(folder.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("(Kumi copy "));
    let Copied::Made(copy) = copy_beside(&project) else { panic!("a copy") };
    // Review K12: in the project's own folder, so the media REAPER names relative to it (Audio/kick.wav) is found
    // from the copy too, and "next to it" is where it is.
    assert_eq!(Path::new(&copy).parent(), project.parent(), "{copy}");
    assert!(copy.ends_with(").rpp") && copy.contains("Night Drive (Kumi copy "), "{copy}");
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), "<REAPER_PROJECT");
    assert_eq!(copy_beside(&project), Copied::Nothing, "the same saved version is copied once, in this session or the next");
    // Each newer save is copied again, and only the newest few are kept.
    let saved = std::time::SystemTime::now();
    for n in 1..=COPIES_KEPT as u64 + 2 {
        std::fs::write(&project, format!("<REAPER_PROJECT save {n}>")).unwrap();
        let file = std::fs::File::options().write(true).open(&project).unwrap();
        file.set_modified(saved + std::time::Duration::from_secs(n)).unwrap();
        assert!(matches!(copy_beside(&project), Copied::Made(_)), "save {n} is copied");
    }
    assert_eq!(copies().count(), COPIES_KEPT);
    // A save older than those kept (a project put back from elsewhere) is copied, and the copy named is there.
    std::fs::write(&project, "<REAPER_PROJECT older>").unwrap();
    let file = std::fs::File::options().write(true).open(&project).unwrap();
    file.set_modified(saved - std::time::Duration::from_secs(3600)).unwrap();
    let Copied::Made(older) = copy_beside(&project) else { panic!("a copy") };
    assert!(Path::new(&older).exists(), "{older} was pruned as soon as it was made");
    assert_eq!(copies().count(), COPIES_KEPT);
    // Another project of the same name beside it keeps its own copies.
    let song = folder.path().join("Night Drive.xrns");
    std::fs::write(&song, "xrns").unwrap();
    assert!(matches!(copy_beside(&song), Copied::Made(_)));
    assert_eq!(copies().count(), COPIES_KEPT + 1, "the song's copy doesn't prune the project's, nor the other way");
    // Never saved: nothing to copy; past the size Kumi copies: said so.
    assert_eq!(copy_beside(&folder.path().join("Untitled.rpp")), Copied::Nothing);
    let big = folder.path().join("Big.rpp");
    std::fs::File::create(&big).unwrap().set_len(MOST_COPIED + 1).unwrap();
    assert_eq!(copy_beside(&big), Copied::TooBig(MOST_COPIED + 1));
}

/// Review K34: a plan's step is held to its kind's schema, as a call of the kind's own tool is: a misspelled or
/// unknown field, a missing one or one of the wrong type is refused in words, and nothing goes to the bridge (this
/// engine has none to go to).
#[tokio::test]
async fn a_step_its_kinds_schema_doesnt_take_is_refused_in_words_before_the_bridge() {
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    let volume = plan.change_kind("track_volume").unwrap();
    for (input, said) in [
        (json!({"ref": "track:1", "volumeDb": -6, "stag": "pre"}), "The input has no field `stag` (it takes "),
        (json!({"volumeDb": -6}), "The input needs `ref`."),
        (json!({"ref": "track:1", "volumeDb": "loud"}), "`volumeDb` must be a number, not a string."),
        (json!({"ref": "track:1", "volumeDb": -6, "stage": "mid"}), "`stage` must be one of \"post\", \"pre\", not \"mid\"."),
    ] {
        let outcome = PlanHost::change(&*plan, volume, input.as_object().unwrap().clone(), Signal::new(), false).await;
        assert!(outcome.is_error && !outcome.stops, "{}", outcome.text);
        assert!(outcome.text.starts_with(&format!("track_volume: {said}")), "{}", outcome.text);
    }
    assert_eq!(plan.history.changes_this_turn.get(), 0, "nothing was counted, let alone sent");
}

#[tokio::test]
async fn a_delete_never_says_what_an_earlier_delete_couldnt_copy() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("Small.rpp");
    std::fs::write(&project, "<REAPER_PROJECT").unwrap();
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, Some(project.to_string_lossy().into_owned()));
    // An earlier delete found the saved file too big, and was refused: its size is not this delete's to say.
    *plan.not_copied.borrow_mut() = Some(Copied::TooBig(600 * 1024 * 1024));
    assert!(plan.copy_before_delete().await.is_some());
    assert_eq!(*plan.not_copied.borrow(), None);
}

/// Review K12: a copy that fails (a folder Kumi can't write in) is told apart from one there's no need for, and the
/// delete says no copy was kept first.
#[cfg(unix)]
#[tokio::test]
async fn a_copy_that_fails_is_said_not_taken_for_one_made_already() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions. Root writes in a read-only folder.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("Locked.rpp");
    std::fs::write(&project, "<REAPER_PROJECT").unwrap();
    std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let copied = copy_beside(&project);
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, Some(project.to_string_lossy().into_owned()));
    let made = plan.copy_before_delete().await;
    std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(&copied, Copied::Failed(why) if why.contains("ermission denied")), "{copied:?}");
    assert_eq!(made, None);
    assert!(matches!(*plan.not_copied.borrow(), Some(Copied::Failed(_))), "the delete says so");
}

#[test]
fn what_the_daw_puts_back_is_followed_even_for_a_change_only_kumi_takes_back() {
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    let mute = plan.change_kind("track_mute").unwrap();
    plan.link().outside_daw_undo.borrow_mut().insert("t-mute".into());
    let id = plan.remember(mute, ChangeSummary::title("Kick muted"), "t-mute", ChangeState::Applied, None);
    let state = || plan.history.entries.borrow().get(&id).map(|entry| entry.borrow().record.state).unwrap();
    // The DAW's undo of a plan's step doesn't take the mute back…
    plan.undone_in_daw(&json!({"undone": ["t-mute"], "redone": []}));
    assert_eq!(state(), ChangeState::Applied);
    // …but its undo of Kumi's own undo step does put it back.
    plan.history.update(&plan.history.entries.borrow().get(&id).cloned().unwrap(), ChangeState::Undone, None);
    plan.undone_in_daw(&json!({"undone": [], "redone": ["t-mute"]}));
    assert_eq!(state(), ChangeState::Applied);
}

#[test]
fn a_change_kumi_kept_is_undone_by_the_daws_undo_of_its_step_and_put_back_by_its_redo() {
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    let lua = plan.change_kind("run_lua").unwrap();
    // Kumi's undo of a Lua chunk is refused (only REAPER's undo knows what it did): the row is kept.
    let id = plan.remember(lua, ChangeSummary::title("Ran Lua"), "t-lua", ChangeState::Applied, None);
    let entry = plan.history.entries.borrow().get(&id).cloned().unwrap();
    plan.history.update(&entry, ChangeState::Kept, Some("Only REAPER's own undo can take back what it does".into()));
    let state = || plan.history.entries.borrow().get(&id).map(|entry| entry.borrow().record.state).unwrap();
    // The producer's Cmd-Z takes the chunk's own step back (accept-reaper lab: HISTORY still said Kept)…
    plan.undone_in_daw(&json!({"undone": ["t-lua"], "redone": []}));
    assert_eq!(state(), ChangeState::Undone);
    // …and their redo puts it back.
    plan.undone_in_daw(&json!({"undone": [], "redone": ["t-lua"]}));
    assert_eq!(state(), ChangeState::Applied);
}

#[tokio::test]
async fn renoise_adds_a_few_tracks_an_answer_and_says_so() {
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    let add = plan.change_kind("track_add").unwrap();
    plan.added.set(32);
    let stopped = plan.one_change(add, JsonObject::new(), Signal::new()).await;
    assert!(stopped.stops && stopped.text.contains("32 tracks added in one answer"), "{}", stopped.text);
    plan.new_turn();
    let next = plan.one_change(add, JsonObject::new(), Signal::new()).await;
    assert!(!next.text.contains("tracks added in one answer"), "a new answer starts again: {}", next.text);
}

#[test]
fn an_epoch_renoise_moved_only_for_devices_keeps_the_names_of_what_stayed() {
    use kumi_daw_own::bridge::follow_epoch;
    use kumi_daw_renoise::RenoiseRefs;
    let fresh = || {
        let mut book = Book::<RenoiseRefs>::default();
        for reference in ["1:track:3", "1:device:3.2", "1:seq:1"] {
            book.short_ref(reference);
        }
        (RefCell::new(book), RefCell::new(Some("1".to_string())), Cell::new(Some(4)))
    };
    let named = |book: &RefCell<Book<RenoiseRefs>>| {
        ["track:1", "device:1", "seq:1"].map(|short| book.borrow().lengthen(&json!({"ref": short}))["ref"] != short)
    };
    // Tracks kept: the track's name stays, the device's and the slot's go; the layout goes on as it was.
    let (book, epoch, layout) = fresh();
    follow_epoch(&book, &epoch, &layout, &json!({"epoch": "2", "layout": 9, "kept": {"from": "1", "tracks": true}}));
    assert_eq!((named(&book), epoch.borrow().clone(), layout.get()), ([true, false, false], Some("2".into()), Some(4)));
    // The kinds the tool names as kept stay too.
    let (book, epoch, layout) = fresh();
    follow_epoch(&book, &epoch, &layout, &json!({"epoch": "2", "kept": {"from": "1", "tracks": true, "kinds": ["track", "seq"]}}));
    assert_eq!(named(&book), [true, false, true]);
    // Kept from another epoch than the book's, or not kept: every name goes, and the answer's layout is taken.
    let (book, epoch, layout) = fresh();
    follow_epoch(&book, &epoch, &layout, &json!({"epoch": "3", "layout": 9, "kept": {"from": "2", "tracks": true}}));
    assert_eq!((named(&book), layout.get()), ([false, false, false], Some(9)));
}

#[test]
fn a_layout_goes_with_any_request_naming_a_track_anywhere_and_renoises_refs_follow_their_track() {
    use kumi_agent::core::refs::{Moved, Shift};
    use kumi_daw_own::bridge::names_tracks;
    use kumi_daw_renoise::RenoiseRefs;
    let asks = |value: Value| names_tracks(&RenoiseRefs, value.as_object().unwrap());
    assert!(asks(json!({"type": "sequencer", "group": "1:track:2"})), "track_add's group");
    assert!(asks(json!({"lines": [{"line": 1}], "track": "1:track:3", "pattern": "1:pattern:1"})));
    assert!(asks(json!({"refs": ["1:device:2.3"]})) && asks(json!({"track": 3})));
    assert!(!asks(json!({"bpm": 120})) && !asks(json!({"pattern": "1:pattern:2"})));
    let shift = Shift { tracks_made: vec![1], ..Shift::default() };
    assert_eq!(RenoiseRefs.moved(&shift, "1:ptrack:3.2"), Moved::To("1:ptrack:3.3".into()), "a pattern track's track is its second number");
    assert_eq!(RenoiseRefs.moved(&shift, "1:slot:2.4"), Moved::To("1:slot:3.4".into()));
    assert_eq!(RenoiseRefs.moved(&shift, "1:pattern:2"), Moved::Same);
}

/// A host that never answers in time.
struct Silent;
#[async_trait(?Send)]
impl McpEndpoint for Silent {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<kumi_agent::mcp::types::Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<kumi_agent::mcp::types::ListToolsResult, RuntimeError> {
        Err(RuntimeError::plain("silent"))
    }
    async fn call(&self, _: &str, _: JsonObject, signal: Signal) -> Result<kumi_agent::mcp::types::CallToolResult, RuntimeError> {
        signal.cancelled().await;
        Err(RuntimeError::plain("MCP request cancelled"))
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> kumi_agent::mcp::client::StderrStatus {
        kumi_agent::mcp::client::StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// A host whose project_save writes the file it's asked for.
struct Saves;
#[async_trait(?Send)]
impl McpEndpoint for Saves {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<kumi_agent::mcp::types::Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<kumi_agent::mcp::types::ListToolsResult, RuntimeError> {
        Err(RuntimeError::plain("no list"))
    }
    async fn call(&self, tool: &str, args: JsonObject, _: Signal) -> Result<kumi_agent::mcp::types::CallToolResult, RuntimeError> {
        assert_eq!(tool, "project_save");
        let text = json!({"saved": args["path"], "dirty": false}).to_string();
        Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": text}]})).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> kumi_agent::mcp::client::StderrStatus {
        kumi_agent::mcp::client::StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_project_saved_by_kumi_is_the_one_a_later_delete_copies() {
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    *plan.link().endpoint.borrow_mut() = Some(Rc::new(Saves));
    let save = plan.action_kind("save_project").unwrap();
    let input = json!({"path": "/Projects/Night Drive.rpp"}).as_object().unwrap().clone();
    let saved = plan.one_action(save, input, Signal::new()).await;
    assert!(!saved.is_error, "{}", saved.text);
    assert_eq!(plan.link().file.borrow().as_deref(), Some("/Projects/Night Drive.rpp"));
}

#[tokio::test]
async fn a_preview_with_no_answer_in_time_stops_the_plan_and_one_the_producer_stopped_says_so() {
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    *plan.link().endpoint.borrow_mut() = Some(Rc::new(Silent));
    let tempo = plan.change_kind("tempo_set").unwrap();
    // Its time ran out (here: the link closed): nothing was written, and the plan stops.
    let lifetime_over = plan.link().lifetime.clone();
    lifetime_over.cancel();
    let late = plan.one_change(tempo, json!({"bpm": 124}).as_object().unwrap().clone(), Signal::new()).await;
    assert!(late.stops && late.text.contains("didn't answer in time"), "{}", late.text);
    // Stopped by the producer: says it stopped.
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    *plan.link().endpoint.borrow_mut() = Some(Rc::new(Silent));
    let stop = Signal::new();
    stop.cancel();
    let stopped = plan.one_change(tempo, json!({"bpm": 124}).as_object().unwrap().clone(), stop).await;
    assert!(stopped.text.starts_with("Stopped before REAPER was asked"), "{}", stopped.text);
}

#[tokio::test]
async fn past_the_window_a_change_only_kumi_can_undo_goes_last_and_each_says_who_can_take_it_back() {
    // (Releasing what Kumi lets go of is a task on Kumi's thread.)
    tokio::task::LocalSet::new().run_until(async { past_the_window() }).await;
}

#[tokio::test]
async fn the_change_just_made_is_never_let_go_and_what_is_undone_goes_first() {
    tokio::task::LocalSet::new().run_until(async { just_made_stays() }).await;
}

fn just_made_stays() {
    let row = |plan: &OwnPlan<kumi_daw_renoise::RenoiseRefs>, id: &str| {
        plan.history.entries.borrow().get(id).map(|entry| entry.borrow().record.clone()).unwrap()
    };
    // A plan of 450 mutes (only Kumi takes them back), then a volume.
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    let (mute, volume) = (plan.change_kind("track_mute").unwrap(), plan.change_kind("track_volume").unwrap());
    *plan.link().step_titles.borrow_mut() = Some(vec![]);
    let mutes: Vec<String> = (0..450)
        .map(|n| {
            plan.link().outside_daw_undo.borrow_mut().insert(format!("t-m{n}"));
            plan.remember(mute, ChangeSummary::title("Kick muted"), &format!("t-m{n}"), ChangeState::Applied, None)
        })
        .collect();
    *plan.link().step_titles.borrow_mut() = None;
    let last = plan.remember(volume, ChangeSummary::title("Kick volume −6 dB"), "t-v", ChangeState::Applied, None);
    assert_eq!(row(&plan, &last).state, ChangeState::Applied, "the change just made stays");
    let gone = row(&plan, &mutes[0]);
    assert_eq!(gone.state, ChangeState::Expired);
    let note = gone.note.unwrap();
    assert!(note.contains("neither can now") && note.contains("by hand"), "{note}");
    // (let go for the cap on changes only Kumi takes back, inside the 400: the note gives that reason)
    assert!(note.starts_with("Past the 200 changes only Kumi can take back"), "{note}");
    assert_eq!(row(&plan, &mutes[51]).state, ChangeState::Applied, "only as many go as are over");
    // More changes after the flood: each stays undoable (changes only Kumi takes back hold at most half the window).
    let held = |plan: &OwnPlan<kumi_daw_renoise::RenoiseRefs>, id: &str| {
        plan.history.entries.borrow().get(id).is_some_and(|entry| !entry.borrow().transaction_id.is_empty())
    };
    let more: Vec<String> = (0..3)
        .map(|n| plan.remember(volume, ChangeSummary::title("Kick volume"), &format!("t-v{n}"), ChangeState::Applied, None))
        .collect();
    for id in std::iter::once(&last).chain(&more) {
        assert!(row(&plan, id).state == ChangeState::Applied && held(&plan, id), "{id} is still Kumi's to undo");
    }
    // 400 mutes one by one (an A/B leaving each on), then three volumes: the same.
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    for n in 0..400 {
        plan.link().outside_daw_undo.borrow_mut().insert(format!("t-m{n}"));
        plan.remember(mute, ChangeSummary::title("Kick muted"), &format!("t-m{n}"), ChangeState::Applied, None);
    }
    let volumes: Vec<String> = (0..3)
        .map(|n| plan.remember(volume, ChangeSummary::title("Kick volume"), &format!("t-v{n}"), ChangeState::Applied, None))
        .collect();
    for id in &volumes {
        assert!(row(&plan, id).state == ChangeState::Applied && held(&plan, id), "{id} is still Kumi's to undo");
    }
    // 400 mutes, each undone by Kumi, then a volume: the undone ones go, and the volume stays.
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    for n in 0..400 {
        plan.link().outside_daw_undo.borrow_mut().insert(format!("t-m{n}"));
        plan.remember(mute, ChangeSummary::title("Kick muted"), &format!("t-m{n}"), ChangeState::Undone, None);
    }
    let first = plan.remember(volume, ChangeSummary::title("Kick volume −6 dB"), "t-v0", ChangeState::Applied, None);
    let second = plan.remember(volume, ChangeSummary::title("Kick volume −3 dB"), "t-v1", ChangeState::Applied, None);
    assert_eq!((row(&plan, &first).state, row(&plan, &second).state), (ChangeState::Applied, ChangeState::Applied));
    assert!(row(&plan, &first).note.is_none());
}

#[test]
fn each_way_kumis_undo_ends_says_who_can_still_take_the_change_back() {
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    let link = plan.link();
    link.outside_daw_undo.borrow_mut().insert("t-repeat".into());
    link.undo_unanswered.borrow_mut().insert("t-sent".into());
    link.in_plan.borrow_mut().insert("t-step".into());
    let applied = ChangeState::Applied;
    for ending in [Ending::Window, Ending::Restart, Ending::Unknown, Ending::Crowded] {
        let repeat = link.ending_note("t-repeat", applied, ending);
        assert!(repeat.contains("by hand") && !repeat.contains("may still take it back"), "{repeat}");
        let sent = link.ending_note("t-sent", ChangeState::Unsure, ending);
        assert!(sent.contains("check it in REAPER") && sent.contains("by hand") && !sent.contains("may still"), "{sent}");
        let step = link.ending_note("t-step", applied, ending);
        assert!(step.contains("takes it back only with the plan's other changes") && !step.contains("may still"), "{step}");
        assert!(link.ending_note("t-other", applied, ending).ends_with("may still take it back."));
        // An Unsure row whose apply Kumi never heard back from stays unsure, and says where to look.
        let unheard = link.ending_note("t-step", ChangeState::Unsure, ending);
        assert!(unheard.contains("isn't sure it went through") && unheard.contains("with the plan's other changes"), "{unheard}");
    }
    assert!(link.ending_note("t-other", applied, Ending::Restart).starts_with("Kumi's link to REAPER started again"));
}

#[tokio::test]
async fn a_change_kumi_just_undid_stays_for_the_daws_undo_of_that_undo_to_bring_it_back() {
    tokio::task::LocalSet::new().run_until(async { undone_stays() }).await;
}

fn undone_stays() {
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    let volume = plan.change_kind("track_volume").unwrap();
    for n in 0..UNDO_WINDOW - 1 {
        plan.remember(volume, ChangeSummary::title("Vocal volume"), &format!("t-{n}"), ChangeState::Applied, None);
    }
    // At the window, Kumi undoes its newest change, then makes one more.
    let undone = plan.remember(volume, ChangeSummary::title("Vocal volume −6 dB"), "t-undone", ChangeState::Applied, None);
    let entry = plan.history.entries.borrow().get(&undone).cloned().unwrap();
    plan.history.update(&entry, ChangeState::Undone, None);
    plan.remember(volume, ChangeSummary::title("Vocal volume −3 dB"), "t-next", ChangeState::Applied, None);
    assert_eq!(entry.borrow().transaction_id, "t-undone", "the change Kumi just undid is still held");
    // The producer's Cmd-Z of Kumi's undo step brings it back: HISTORY follows.
    plan.undone_in_daw(&json!({"undone": [], "redone": ["t-undone"]}));
    assert_eq!(entry.borrow().record.state, ChangeState::Applied);
}

#[test]
fn lines_the_daw_may_have_put_back_are_unsure_not_put_back() {
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    let notes = plan.change_kind("notes_write").unwrap();
    let id = plan.remember(notes, ChangeSummary::title("Bass lines"), "t-notes", ChangeState::Applied, None);
    let entry = plan.history.entries.borrow().get(&id).cloned().unwrap();
    plan.undone_in_daw(&json!({"undone": ["t-notes"], "redone": []}));
    assert_eq!(entry.borrow().record.state, ChangeState::Undone);
    plan.undone_in_daw(&json!({"undone": [], "redone": [], "unsure": ["t-notes"]}));
    let record = entry.borrow().record.clone();
    assert_eq!(record.state, ChangeState::Unsure);
    assert_eq!(record.note.as_deref(), Some("Renoise may have redone it; Kumi can't tell."));
    // The DAW's undo of it after that is told as ever.
    plan.undone_in_daw(&json!({"undone": ["t-notes"], "redone": []}));
    assert_eq!(entry.borrow().record.state, ChangeState::Undone);
}

#[test]
fn an_unsure_row_stays_unsure_when_kumi_can_no_longer_undo_it() {
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    let (notes, volume) = (plan.change_kind("notes_write").unwrap(), plan.change_kind("track_volume").unwrap());
    let row = |id: &str| plan.history.entries.borrow().get(id).map(|entry| entry.borrow().record.clone()).unwrap();
    // Renoise may have redone lines Kumi undid; an apply Kumi never heard back from; and lines the DAW settled.
    let redone = plan.remember(notes, ChangeSummary::title("Bass lines"), "t-redone", ChangeState::Applied, None);
    let unheard = plan.remember(volume, ChangeSummary::title("Kick volume −6 dB"), "t-unheard", ChangeState::Unsure, None);
    let settled = plan.remember(notes, ChangeSummary::title("Lead lines"), "t-settled", ChangeState::Applied, None);
    plan.undone_in_daw(&json!({"undone": ["t-redone", "t-settled"], "redone": []}));
    plan.undone_in_daw(&json!({"undone": [], "redone": [], "unsure": ["t-redone", "t-settled"]}));
    assert!(plan.link().maybe_redone.borrow().contains("t-settled"));
    // Renoise's undo of the settled one after that: what it does is known again.
    plan.undone_in_daw(&json!({"undone": ["t-settled"], "redone": []}));
    assert!(!plan.link().maybe_redone.borrow().contains("t-settled"), "the DAW's undo settles it");
    plan.undone_in_daw(&json!({"undone": [], "redone": ["t-settled"]}));
    assert_eq!(row(&settled).state, ChangeState::Applied);
    // Kumi's link starts again: each row says what Kumi knows of it.
    plan.retire_all(Ending::Restart);
    let note = row(&redone).note.unwrap();
    assert!(note.contains("Renoise may have redone it (Kumi can't tell)") && note.contains("by hand if it's there"), "{note}");
    let note = row(&unheard).note.unwrap();
    assert!(note.contains("Kumi isn't sure it went through") && note.contains("check it in Renoise"), "{note}");
    let note = row(&settled).note.unwrap();
    assert!(note.ends_with("may still take it back.") && !note.contains("isn't sure"), "{note}");
}

#[test]
fn an_undo_never_answered_is_settled_by_the_daws_undo_of_its_step() {
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    let volume = plan.change_kind("track_volume").unwrap();
    let id = plan.remember(volume, ChangeSummary::title("Vocal volume −6 dB"), "t-sent", ChangeState::Unsure, None);
    plan.link().undo_unanswered.borrow_mut().insert("t-sent".into());
    // REAPER's undo took Kumi's undo step back: the undo ran, and the change is back.
    plan.undone_in_daw(&json!({"undone": [], "redone": ["t-sent"]}));
    let record = plan.history.entries.borrow().get(&id).map(|entry| entry.borrow().record.clone()).unwrap();
    assert_eq!(record.state, ChangeState::Applied);
    assert!(!plan.link().undo_unanswered.borrow().contains("t-sent"), "Kumi may undo it again, as a fresh undo");
}

fn past_the_window() {
    let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
    let (mute, volume) = (plan.change_kind("track_mute").unwrap(), plan.change_kind("track_volume").unwrap());
    plan.link().outside_daw_undo.borrow_mut().insert("t-mute".into());
    let muted = plan.remember(mute, ChangeSummary::title("Kick muted"), "t-mute", ChangeState::Applied, None);
    let first = plan.remember(volume, ChangeSummary::title("Kick volume −6 dB"), "t-0", ChangeState::Applied, None);
    for n in 1..UNDO_WINDOW {
        plan.remember(volume, ChangeSummary::title("Kick volume −6 dB"), &format!("t-{n}"), ChangeState::Applied, None);
    }
    let row = |id: &str| plan.history.entries.borrow().get(id).map(|entry| entry.borrow().record.clone()).unwrap();
    assert_eq!(row(&muted).state, ChangeState::Applied, "the change only Kumi can take back stays");
    assert_eq!(row(&first).state, ChangeState::Expired, "the oldest other one goes");
    assert!(row(&first).note.unwrap().contains("may still take it back"));
    // Unsure: where to look, and who can take it back.
    let note = unsure_note(&DawProfile::RENOISE, "Kick muted", true, false);
    assert!(note.contains("by hand") && note.contains("doesn't take this back"), "{note}");
    let note = unsure_note(&DawProfile::REAPER, "Vocal volume −6 dB", false, true);
    assert!(note.contains("with the plan's other changes"), "{note}");
    assert!(unsure_undo_note(&DawProfile::REAPER, "Vocal volume −6 dB").contains("its undo went through"));
}

#[test]
fn each_step_line_says_what_the_step_does_to_what() {
    // (make_changes' line is all the model sees of a step that isn't a tool until it calls find_tools)
    let dangling = ["a", "an", "the", "its", "of", "into", "in", "at", "by", "and", "or", "with", "for"];
    for (app, set) in sets() {
        let kinds = set.kinds.iter().filter(|kind| !kind.core).map(|kind| (kind.tool, kind.description, kind.input_schema()));
        let actions =
            set.actions.iter().filter(|action| !action.core).map(|action| (action.tool, action.description, action.input_schema()));
        for (tool, description, schema) in kinds.chain(actions) {
            let line = own_step_line(tool, description, &schema);
            let what = line.strip_prefix(&format!("{tool}: ")).unwrap_or_else(|| panic!("{app}: {line}"));
            let what = if what.ends_with(')') { what.rsplit_once(" (").map_or(what, |(what, _)| what) } else { what };
            let last = what.rsplit(' ').next().unwrap_or("");
            assert!(what.split(' ').count() >= 3 && !what.contains('(') && !dangling.contains(&last), "{app}: {line}");
            let names = match tool {
                "doofer_make" | "doofer_macro" => "Splitter",
                "sample_slices" => "slice markers",
                "track_channels" => "channels",
                "item_import" => "on a track",
                _ => "",
            };
            assert!(what.contains(names), "{app}: {line}");
        }
    }
}

#[test]
fn every_kind_has_a_schema_the_model_can_follow_in_the_daws_own_words() {
    let live = Regex::new(LIVE_WORDS).unwrap();
    for (app, set) in sets() {
        for text in [set.plan_description, set.undo_description, set.instructions] {
            assert!(!live.is_match(text), "{app}: {text}");
        }
        // Every state a row of kumiChanges can be in (review K39).
        assert!(set.instructions.contains("(applied, undone, kept, unsure, or expired: "), "{app}");
        // Every action's words and schema, and every kind's schema text, too.
        for action in set.actions {
            assert!(!live.is_match(action.description) && !live.is_match(action.schema), "{app} {}", action.tool);
            assert!(action.input_schema().contains_key("properties"), "{app} {}", action.tool);
        }
        for kind in set.kinds {
            assert!(!live.is_match(kind.schema), "{app} {}: {}", kind.tool, kind.schema);
        }
        let mut tools = BTreeSet::new();
        for kind in set.kinds {
            assert!(tools.insert(kind.tool), "{app}: {} twice", kind.tool);
            assert_eq!(kind.tool.replace('_', "-"), kind.kind, "{app}: the tool is its kind's name");
            assert!(!live.is_match(kind.description), "{app} {}: {}", kind.tool, kind.description);
            let schema = kind.input_schema();
            let properties = schema["properties"].as_object().unwrap_or_else(|| panic!("{app} {}", kind.tool));
            for required in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
                assert!(properties.contains_key(required.as_str().unwrap()), "{app} {}: {required}", kind.tool);
            }
            // A field that holds a ref is named as refs are, so Kumi shortens and lengthens it; the bridge's other
            // names for them are what `wire` sends.
            for (ours, theirs) in kind.wire {
                assert!(properties.contains_key(*ours), "{app} {}: {ours}", kind.tool);
                assert!(ours.ends_with("Ref") || ours.ends_with("Refs"), "{app} {}: {ours}", kind.tool);
                assert!(!properties.contains_key(*theirs), "{app} {}: {theirs}", kind.tool);
            }
            for unnamed in ["track", "pattern", "group", "asFolderOf"] {
                assert!(!properties.contains_key(unnamed), "{app} {}: {unnamed} holds a ref", kind.tool);
            }
        }
        assert!(set.kinds.iter().filter(|kind| kind.core).count() <= 8, "{app}: few tools on every request");
    }
}

// m4-r-fxenv
/// A host that answers every call as REAPER's bridge answers an FX chain save, keeping what it was asked.
struct Saver(RefCell<Vec<(String, JsonObject)>>);
#[async_trait(?Send)]
impl McpEndpoint for Saver {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<kumi_agent::mcp::types::Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<kumi_agent::mcp::types::ListToolsResult, RuntimeError> {
        Err(RuntimeError::plain("not listed"))
    }
    async fn call(&self, tool: &str, args: JsonObject, _: Signal) -> Result<kumi_agent::mcp::types::CallToolResult, RuntimeError> {
        self.0.borrow_mut().push((tool.to_string(), args));
        let answer = json!({"epoch": "e1", "file": "/r/FXChains/Lead.RfxChain", "name": "Lead", "count": 2, "replaced": false});
        Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": answer.to_string()}]})).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> kumi_agent::mcp::client::StderrStatus {
        kumi_agent::mcp::client::StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[tokio::test]
async fn an_fx_chain_save_asks_with_the_long_refs_and_says_where_the_file_went() {
    let plan = engine::<kumi_daw_reaper::ReaperRefs>(&kumi_daw_reaper::DAW, None);
    let saver = Rc::new(Saver(RefCell::default()));
    *plan.link().endpoint.borrow_mut() = Some(saver.clone());
    let (track, fx) = {
        let mut book = plan.link().book.borrow_mut();
        (book.short_ref("r:track:{AAAA}"), book.short_ref("r:fx:{F1}"))
    };
    let save = plan.action_kind("fx_chain_save").unwrap();
    let input = json!({"ref": track, "fxRefs": [fx], "name": "Lead"});
    let done = plan.one_action(save, input.as_object().unwrap().clone(), Signal::new()).await;
    assert!(!done.is_error, "{}", done.text);
    {
        let asked = saver.0.borrow();
        assert_eq!(asked[0].0, "fx_chain_save");
        assert_eq!((asked[0].1["ref"].clone(), asked[0].1["fxRefs"].clone()), (json!("r:track:{AAAA}"), json!(["r:fx:{F1}"])));
    }
    let told: Value = serde_json::from_str(&done.text).unwrap();
    assert_eq!((told["file"].clone(), told["count"].clone()), (json!("/r/FXChains/Lead.RfxChain"), json!(2)));
    // A short ref this turn doesn't know: nothing is asked.
    let unknown = plan.one_action(save, json!({"ref": "track:99", "name": "x"}).as_object().unwrap().clone(), Signal::new()).await;
    assert!(unknown.is_error && unknown.text.contains("isn't one of this turn's refs"), "{}", unknown.text);
    assert_eq!(saver.0.borrow().len(), 1);
}

/// A host whose bridge refuses every undo with this code.
struct Refuses(&'static str);
#[async_trait(?Send)]
impl McpEndpoint for Refuses {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<kumi_agent::mcp::types::Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<kumi_agent::mcp::types::ListToolsResult, RuntimeError> {
        Err(RuntimeError::plain("not listed"))
    }
    async fn call(&self, _: &str, _: JsonObject, _: Signal) -> Result<kumi_agent::mcp::types::CallToolResult, RuntimeError> {
        let answer = json!({"error": "it isn't in the song any more", "code": self.0});
        Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": answer.to_string()}], "isError": true})).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> kumi_agent::mcp::client::StderrStatus {
        kumi_agent::mcp::client::StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[tokio::test]
async fn an_undo_refused_after_the_daws_own_redo_says_the_daw_put_it_back_not_that_it_went() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
            *plan.link().endpoint.borrow_mut() = Some(Rc::new(Refuses("gone")));
            plan.link().connected.set(true);
            let add = plan.change_kind("track_add").unwrap();
            let row = |id: &str| plan.history.entries.borrow().get(id).map(|entry| entry.borrow().record.clone()).unwrap();
            // Renoise's undo and redo of one track add (to its bridge, the redo made a new track); another never touched.
            let redone = plan.remember(add, ChangeSummary::title("Added track “Pad”"), "t-redone", ChangeState::Applied, None);
            let plain = plan.remember(add, ChangeSummary::title("Added track “Keys”"), "t-plain", ChangeState::Applied, None);
            plan.undone_in_daw(&json!({"undone": ["t-redone"], "redone": []}));
            plan.undone_in_daw(&json!({"undone": [], "redone": ["t-redone"]}));
            for id in [&redone, &plain] {
                let input = json!({"change": id});
                kumi_agent::integrations::shared::undo::undo_change_with(&plan.history, input.as_object().unwrap(), Signal::new(), false)
                    .await
                    .unwrap();
            }
            let put_back = row(&redone);
            assert_eq!(put_back.state, ChangeState::Kept);
            let note = put_back.note.unwrap();
            assert!(
                note.contains("Renoise put it back with its own undo or redo") && note.contains(DawProfile::RENOISE.vocab.own_undo),
                "{note}"
            );
            assert!(row(&plain).note.unwrap().contains("is gone from the song"), "a change nothing put back still says it went");
            assert!(!plan.link().put_back_by_daw.borrow().contains("t-redone"), "Kumi's undo settled it");
        })
        .await;
}

#[tokio::test]
async fn an_undo_refused_in_another_song_says_another_song_is_open() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let plan = engine::<kumi_daw_renoise::RenoiseRefs>(&kumi_daw_renoise::DAW, None);
            *plan.link().endpoint.borrow_mut() = Some(Rc::new(Refuses("stale")));
            plan.link().connected.set(true);
            let rename = plan.change_kind("track_rename").unwrap();
            let row = |id: &str| plan.history.entries.borrow().get(id).map(|entry| entry.borrow().record.clone()).unwrap();
            *plan.link().project.borrow_mut() = Some("song-1".into());
            let before = plan.remember(rename, ChangeSummary::title("Renamed track"), "t-1", ChangeState::Applied, None);
            let here = plan.remember(rename, ChangeSummary::title("Renamed track"), "t-2", ChangeState::Applied, None);
            let history = &plan.history;
            let undo = |id: String| async move {
                let input = json!({"change": id});
                kumi_agent::integrations::shared::undo::undo_change_with(history, input.as_object().unwrap(), Signal::new(), false)
                    .await
                    .unwrap();
            };
            // Renoise opened another song: the first change's undo is refused there.
            *plan.link().project.borrow_mut() = Some("song-2".into());
            undo(before.clone()).await;
            let note = row(&before).note.unwrap();
            assert!(note.starts_with("Another song is open in Renoise now"), "{note}");
            // In the song it was made in, a stale refusal still says it moved.
            *plan.link().project.borrow_mut() = Some("song-1".into());
            undo(here.clone()).await;
            assert!(row(&here).note.unwrap().contains("moved in the song"));
        })
        .await;
}
