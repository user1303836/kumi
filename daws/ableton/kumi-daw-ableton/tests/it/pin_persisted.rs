//! What today's stores write to disk, byte for byte, and that reading each file back and saving it again changes
//! nothing: conversations and their index, notes, goals, objectives and jobs, recipes, techniques, lessons, gaps,
//! timings, the library's Sets, baselines and kumi.db's rows about a Set. Hermetic, in temporary folders, with fixed
//! times; what a writer stamps itself (file times, now, a fresh id, Kumi's version) is normalised on both sides.
use kumi_common::{abort, js::json::stringify};
use kumi_daw_ableton::project::{create_project_store, project_id_of, Baseline, ProjectStore, PROJECT_KEY};
use kumi_runtime::{
    audio::matching::Focus,
    core::{
        contracts::{
            AuditionCandidate, AuditionRequest, ChainPlacement, ChangeFamily, ChangeRecord, ChangeState, ClipNote, ClipPicture,
            ColorChange, ConversationStore, DevicePlacement, KernelCheckpoint, MemoryScope, MemoryStore, NoteChange, Picked,
            SavedConversation, TrackChip, Usage,
        },
        conversations::{create_conversation_store_keeping, IMPORTED},
        evolve::{Knob, Slot},
        gaps::gap_tools,
        goal::{create_goal_store, Best, GoalRun, GoalState, GoalStore},
        goal_mode::{create_objective_store, Check, Objective, ObjectiveBudget, ObjectiveState, ObjectiveStore, Verdict},
        jobs::{create_job_store, JobKind, JobSpec, JobState, JobStatus, JobStore, KeptJob, WhenDone},
        memory::{create_memory_store, MemoryStoreOptions, MAX_NOTES},
        picture::{ChangePicture, Dynamics, EqBand, EqShape, MixLevels, MixSend, PictureValue},
        playbook::{create_playbook_store, Lesson, PlaybookStore, Reaction},
        recipes::{create_recipe_store, Recipe, RecipeParam, RecipeStore},
        store_backed::SqliteMemoryStore,
        store_client::StoreClient,
        taste_log::{TasteLog, UndoneBy, Whereabouts},
        techniques::{create_technique_store, TechniqueBody, TechniqueDraft, TechniqueSource, TechniqueStore},
        timing::{self, TurnTiming},
    },
    library::{
        learn::SetEntry,
        sets::{Clips, DeviceRole, SetDevice, SetSummary, SetTrack, TrackKind},
        store::Entry,
    },
    library_logs, KUMI_VERSION,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
};

/// The project id "/Music/Night Drive.als" gives: the place every Set-scoped file below is kept under.
const A: &str = "a2dfd5f2cf329c593c5fdc7448524087";
const AT: i64 = 1_760_000_000_000;
/// The conversations' ids: the time each began in base 36, then six hex digits.
const ID1: &str = "mgj6k3cw0a0b0c";
const ID2: &str = "mgj6wybk1a2b3c";
const ID3: &str = "mgj79ta82b3c4d";
const ID4: &str = "mgj7mo8w3c4d5e";
const ID5: &str = "mgj7zj7k4d5e6f";
const BUDGET: ObjectiveBudget = ObjectiveBudget { turns: 10, ms: 1_800_000, idle: 2 };

fn goldens(area: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/persisted").join(area)
}

/// Every file under `dir`, by its path from there with "/" between folders.
fn files(dir: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, prefix: &str, found: &mut BTreeMap<String, String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &format!("{name}/"), found);
            } else {
                let text = std::fs::read_to_string(entry.path()).unwrap_or_else(|_| panic!("{name} isn't text"));
                found.insert(name, text);
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(dir, "", &mut found);
    found
}

/// Copies files (as `files` names them) into `dir`.
fn put(dir: &Path, files: &BTreeMap<String, String>) {
    for (name, text) in files {
        let file = dir.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

/// Where two texts first differ: the line's number, the lines before it, and the next few lines of each.
fn first_difference(actual: &str, expected: &str) -> String {
    if actual.replace("\r\n", "\n") == expected.replace("\r\n", "\n") {
        return "only the line endings differ (CRLF against LF)".into();
    }
    let (actual, expected): (Vec<&str>, Vec<&str>) = (actual.split('\n').collect(), expected.split('\n').collect());
    let line = (0..actual.len().max(expected.len())).find(|&i| actual.get(i) != expected.get(i)).unwrap_or(0);
    let mut text = format!("first difference at line {} (- golden, + now):\n", line + 1);
    for (i, same) in expected.iter().enumerate().take(line).skip(line.saturating_sub(3)) {
        text += &format!("    {:>5} {same}\n", i + 1);
    }
    for (mark, lines) in [('-', &expected), ('+', &actual)] {
        for (i, differs) in lines.iter().enumerate().skip(line).take(3) {
            text += &format!("  {mark} {:>5} {differs}\n", i + 1);
        }
    }
    text
}

/// Checks `actual` against the area's goldens (KUMI_BLESS=1 writes them instead).
fn golden(area: &str, actual: &BTreeMap<String, String>) {
    let folder = goldens(area);
    if std::env::var_os("KUMI_BLESS").is_some() {
        let _ = std::fs::remove_dir_all(&folder);
        put(&folder, actual);
        return;
    }
    same(area, actual, "changed");
}

/// Checks `actual` against the area's goldens, as they are.
fn same(area: &str, actual: &BTreeMap<String, String>, what: &str) {
    let folder = goldens(area);
    let expected = files(&folder);
    assert!(!expected.is_empty(), "{} is missing: run this test with KUMI_BLESS=1", folder.display());
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the files under {} {what}: check the difference, then run this test with KUMI_BLESS=1",
        folder.display()
    );
    for (name, text) in actual {
        assert!(
            *text == expected[name],
            "{}/{name} {what}: check the difference, then run this test with KUMI_BLESS=1\n{}",
            folder.display(),
            first_difference(text, &expected[name])
        );
    }
}

fn change(id: &str, family: ChangeFamily, title: &str, state: ChangeState, at: i64) -> ChangeRecord {
    ChangeRecord {
        id: id.into(),
        family,
        title: title.into(),
        track: None,
        from: None,
        to: None,
        range: None,
        colors: None,
        clip: None,
        devices: None,
        state,
        score: None,
        note: None,
        at,
        picture: None,
    }
}

fn note(start: f64, pitch: f64, duration: f64, velocity: f64) -> ClipNote {
    ClipNote { pitch, start, duration, velocity }
}

fn band(shape: EqShape, freq: f64, gain_db: f64, q: f64, on: bool, slope: Option<f64>) -> EqBand {
    EqBand { shape, freq, gain_db, q, on, slope }
}

/// One change of every family, every state and every picture kind, as HISTORY keeps them.
fn changes() -> Vec<ChangeRecord> {
    let bass = Some(TrackChip { name: "Bass".into(), color: Some("#ff5500".into()) });
    let lead = Some(TrackChip { name: "Lead".into(), color: None });
    let hook = vec![note(0.0, 60.0, 0.5, 100.0), note(0.5, 63.0, 0.5, 90.0), note(1.0, 67.0, 1.0, 110.0)];
    vec![
        ChangeRecord {
            from: Some(120.0),
            to: Some(124.0),
            range: Some([20.0, 999.0]),
            picture: Some(ChangePicture::Values {
                values: vec![PictureValue {
                    name: "Tempo".into(),
                    from: Some(120.0),
                    to: Some(124.0),
                    min: Some(20.0),
                    max: Some(999.0),
                    from_text: Some("120.00 BPM".into()),
                    to_text: Some("124.00 BPM".into()),
                    asked: Some("124".into()),
                }],
            }),
            ..change("c1", ChangeFamily::Tempo, "Tempo 120 → 124 BPM", ChangeState::Applied, AT + 1_000)
        },
        ChangeRecord {
            track: bass.clone(),
            from: Some(0.85),
            to: Some(0.775),
            range: Some([0.0, 1.0]),
            picture: Some(ChangePicture::Mix {
                before: MixLevels {
                    volume: Some(0.85),
                    volume_text: Some("-6.0 dB".into()),
                    pan: Some(0.0),
                    pan_text: Some("C".into()),
                    sends: vec![MixSend { name: "A-Reverb".into(), level: 0.3, text: Some("-12.0 dB".into()) }],
                    mute: Some(false),
                    solo: Some(false),
                },
                after: MixLevels {
                    volume: Some(0.775),
                    volume_text: Some("-9.0 dB".into()),
                    pan: Some(-0.25),
                    pan_text: Some("25L".into()),
                    sends: vec![MixSend { name: "A-Reverb".into(), level: 0.45, text: None }],
                    mute: None,
                    solo: Some(true),
                },
            }),
            ..change("c2", ChangeFamily::Mixer, "Bass volume -6 → -9 dB", ChangeState::Undone, AT + 2_000)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Vox".into(), color: Some("#22cc88".into()) }),
            note: Some("Live doesn't let Kumi undo this.".into()),
            ..change("c3", ChangeFamily::Rename, "Renamed “Audio 1” to “Vox”", ChangeState::Kept, AT + 3_000)
        },
        change("c4", ChangeFamily::Structure, "Added track “Pad”", ChangeState::Unsure, AT + 4_000),
        ChangeRecord {
            track: lead,
            clip: Some(ClipPicture { length: 4.0, notes: hook.clone() }),
            picture: Some(ChangePicture::Notes { length: 4.0, before: vec![], after: hook, partial: true }),
            ..change("c5", ChangeFamily::Clip, "New clip “Hook” on Lead", ChangeState::Expired, AT + 5_000)
        },
        ChangeRecord {
            track: bass.clone(),
            devices: Some(DevicePlacement {
                devices: Some(vec!["Wavetable".into(), "EQ Eight".into()]),
                index: Some(1.0),
                rack: Some("Bass Rack".into()),
                chains: Some(vec![
                    ChainPlacement { name: "Sub".into(), devices: vec!["Operator".into()] },
                    ChainPlacement { name: "Top".into(), devices: vec!["Wavetable".into(), "EQ Eight".into()] },
                ]),
                chain: Some(1.0),
            }),
            picture: Some(ChangePicture::Eq {
                before: vec![band(EqShape::LowCut, 30.0, 0.0, 0.71, true, Some(24.0)), band(EqShape::Bell, 250.0, 0.0, 1.0, false, None)],
                after: vec![
                    band(EqShape::LowCut, 40.0, 0.0, 0.71, true, Some(48.0)),
                    band(EqShape::Bell, 250.0, -3.5, 1.4, true, None),
                    band(EqShape::HighShelf, 8000.0, 2.0, 0.71, true, None),
                ],
                values: vec![PictureValue { name: "2 Gain A".into(), from: Some(0.0), to: Some(-3.5), ..PictureValue::default() }],
            }),
            ..change("c6", ChangeFamily::Device, "Loaded EQ Eight on Bass", ChangeState::Applied, AT + 6_000)
        },
        ChangeRecord {
            track: bass.clone(),
            from: Some(-10.0),
            to: Some(-18.0),
            range: Some([-60.0, 0.0]),
            picture: Some(ChangePicture::Dynamics {
                before: Dynamics { threshold_db: -10.0, ratio: 2.0, knee_db: 6.0, makeup_db: 0.0 },
                after: Dynamics { threshold_db: -18.0, ratio: 4.0, knee_db: 6.0, makeup_db: 3.5 },
                values: vec![],
            }),
            ..change("c7", ChangeFamily::Parameter, "Glue Compressor Threshold -10 → -18 dB", ChangeState::Applied, AT + 7_000)
        },
        change("c8", ChangeFamily::Locators, "Added locator “Drop” at bar 33", ChangeState::Applied, AT + 8_000),
        ChangeRecord {
            track: bass.clone(),
            colors: Some(ColorChange { from: Some("#ff5500".into()), to: "#00aaff".into() }),
            ..change("c9", ChangeFamily::Color, "Bass is blue now", ChangeState::Applied, AT + 9_000)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Automation {
                parameter: "Filter Freq".into(),
                min: 20.0,
                max: 20000.0,
                before: vec![],
                after: vec![[0.0, 200.0], [16.0, 8000.0], [32.0, 200.0]],
                words: None,
            }),
            ..change("c10", ChangeFamily::Parameter, "Filter sweep on Bass", ChangeState::Applied, AT + 10_000)
        },
        ChangeRecord {
            score: Some(87.5),
            ..change("c11", ChangeFamily::Device, "Heard 3 candidates against the reference", ChangeState::Heard, AT + 11_000)
        },
    ]
}

fn conversation(saved_at: i64, messages: Value) -> SavedConversation {
    SavedConversation {
        saved_at,
        checkpoint: KernelCheckpoint {
            version: 1,
            messages: serde_json::from_value(messages).unwrap(),
            origin: Some("openai/gpt-5.5".into()),
            tools: Some("Zq3vY0b8mXH2c9KdLwPr1A".into()),
        },
        changes: None,
        first: None,
        turns: None,
    }
}

fn baseline() -> Baseline {
    Baseline {
        version: 1,
        path: "/Music/Night Drive.als".into(),
        name: "Night Drive".into(),
        saved_at: AT - 60_000,
        artifact_id: "artifact-1".into(),
        pages: vec![json!({"records":[
            {"snapshotId":"t1","kind":"track","name":"Bass","parentSnapshotId":"set"},
            {"snapshotId":"d1","kind":"device","name":"Wavetable","parentSnapshotId":"t1"},
            {"snapshotId":"l1","kind":"locator","name":"Drop","time":128}
        ]})
        .as_object()
        .unwrap()
        .clone()],
    }
}

/// The conversation index with the file times it keeps (a stamp's ns, a baseline's) as 1 when there is one, and the
/// times conversations went (now) as 0.
fn normalised_index(text: &str) -> String {
    let mut index: Value = serde_json::from_str(text).unwrap();
    let some = |value: &mut Value| {
        if value.as_u64().is_some_and(|n| n != 0) {
            *value = json!(1);
        }
    };
    for entry in index["conversations"].as_object_mut().unwrap().values_mut() {
        some(&mut entry["stamp"]["modified"]);
        some(&mut entry["stamp"]["meta"]);
    }
    for place in index["places"].as_object_mut().unwrap().values_mut() {
        some(&mut place["modified"]);
    }
    for at in index["gone"].as_object_mut().unwrap().values_mut() {
        *at = json!(0);
    }
    stringify(&index)
}

fn projects_tree(dir: &Path) -> BTreeMap<String, String> {
    let mut found = files(dir);
    if let Some(index) = found.get_mut("conversation-index.json") {
        *index = normalised_index(index);
    }
    found
}

#[tokio::test]
async fn conversations_their_index_names_pins_and_baselines_keep_their_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    let baselines = create_project_store(&projects);
    baselines.save(A, &baseline()).await.unwrap();
    baselines.save_track_keepers(A, &HashMap::from([("t1".to_string(), "track-identity-7".to_string())])).await.unwrap();

    let store = create_conversation_store_keeping(&projects, Some(120));
    let first = SavedConversation {
        changes: Some(changes()),
        first: Some("Make the bass darker".into()),
        turns: Some(1),
        ..conversation(
            AT + 12_000,
            json!([
                {"role":"user","content":[
                    {"type":"text","text":"Make the bass darker"},
                    {"type":"file","filename":"bass.png","mediaType":"image/png","data":"iVBORw0KGgo="}
                ]},
                {"role":"assistant","content":[
                    {"type":"text","text":"Taking the top off the bass."},
                    {"type":"tool-call","toolCallId":"call_1","toolName":"make_changes","input":{"steps":[{"tool":"set_parameter"}]}}
                ]},
                {"role":"tool","content":[
                    {"type":"tool-result","toolCallId":"call_1","toolName":"make_changes","output":{"type":"text","value":"done"}}
                ]},
                {"role":"assistant","content":[{"type":"text","text":"Done: the bass is darker."}]}
            ]),
        )
    };
    store.save(A, ID1, &first).await.unwrap();
    let copy = conversation(
        AT + 900_000,
        json!([
            {"role":"user","content":"Try a sub under it"},
            {"role":"assistant","content":"Added a sine sub an octave down."},
            {"role":"user","content":"Louder"},
            {"role":"assistant","content":"Up 3 dB."}
        ]),
    );
    store.create(A, ID2, &copy, Some("Bass ideas"), Some(ID1)).await.unwrap();
    assert!(store.pin(A, ID2, true).await.unwrap());
    // One an earlier Kumi let go: the index remembers it went.
    store.save("unsaved", ID5, &conversation(AT + 2_500_000, json!([{"role":"user","content":"Hello"}]))).await.unwrap();
    std::fs::remove_file(projects.join("unsaved/conversations").join(format!("{ID5}.json"))).unwrap();
    let unsaved = conversation(
        AT + 1_500_000,
        json!([{"role":"user","content":"Make a beat"},{"role":"assistant","content":"Four on the floor at 124 BPM."}]),
    );
    store.save("unsaved", ID3, &unsaved).await.unwrap();
    // Saved before the time its id names: it began when it was saved.
    let imported =
        conversation(AT + 30_000, json!([{"role":"user","content":"What key is it in?"},{"role":"assistant","content":"A minor."}]));
    store.create(IMPORTED, ID4, &imported, None, None).await.unwrap();
    assert!(store.note_set(IMPORTED, ID4, "Old Song").await.unwrap());
    drop(store);
    golden("conversations", &projects_tree(&projects));

    // Read back and saved again, by today's readers and writers: the same bytes.
    let again = tempfile::tempdir().unwrap();
    let projects = again.path().join("projects");
    put(&projects, &files(&goldens("conversations")));
    let store = create_conversation_store_keeping(&projects, Some(120));
    for (place, id) in [(A, ID1), (A, ID2), ("unsaved", ID3), (IMPORTED, ID4)] {
        let kept = store.load(place, id).await.unwrap().unwrap();
        store.save(place, id, &kept).await.unwrap();
    }
    assert!(store.rename(A, ID2, Some("Bass ideas")).await.unwrap());
    assert!(store.note_set(IMPORTED, ID4, "Old Song").await.unwrap());
    let baselines = create_project_store(&projects);
    let seen = baselines.load(A).await.unwrap().unwrap();
    assert_eq!(seen, baseline());
    baselines.save(A, &seen).await.unwrap();
    let keepers = baselines.load_track_keepers(A).await.unwrap();
    baselines.save_track_keepers(A, &keepers).await.unwrap();
    drop(store);
    same("conversations", &projects_tree(&projects), "changed when read and saved again");
}

#[tokio::test]
async fn notes_about_the_producer_and_a_set_keep_their_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let options = |root: &Path| MemoryStoreOptions { projects_dir: root.join("projects"), producer_file: root.join("memory.json") };
    let store = create_memory_store(options(dir.path()));
    store.remember(MemoryScope::Producer, None, "Likes short, dark reverbs on drums", None, AT).await.unwrap();
    store.remember(MemoryScope::Producer, None, "Works in A minor mostly", None, AT + 1_000).await.unwrap();
    store.change(MemoryScope::Producer, None, "p1", NoteChange::Pinned(true), AT + 2_000).await.unwrap();
    store.remember(MemoryScope::Set, Some(A), "The Reese is the main bass", None, AT + 3_000).await.unwrap();
    store.remember(MemoryScope::Set, Some(A), "Drop at bar 33", None, AT + 4_000).await.unwrap();
    store.remember(MemoryScope::Set, Some(A), "The Reese and the sub are the bass", Some("s1"), AT + 5_000).await.unwrap();
    golden("memory", &files(dir.path()));

    let again = tempfile::tempdir().unwrap();
    put(again.path(), &files(&goldens("memory")));
    let store = create_memory_store(options(again.path()));
    let memory = store.load(Some(A)).await.unwrap();
    store.save(MemoryScope::Producer, None, &memory.producer).await.unwrap();
    store.save(MemoryScope::Set, Some(A), &memory.set).await.unwrap();
    same("memory", &files(again.path()), "changed when read and saved again");
}

fn goal_state() -> GoalState {
    let knob = |r: &str, name: &str, value: f64| Knob {
        r#ref: r.into(),
        device: "Wavetable".into(),
        name: name.into(),
        min: 0.0,
        max: 1.0,
        step: None,
        value,
    };
    GoalState {
        version: 1,
        goal: "a Reese like the reference".into(),
        request: AuditionRequest {
            candidates: vec![
                AuditionCandidate {
                    track: "Kumi · Goal · Reese".into(),
                    clip: Some("scene:2".into()),
                    label: Some("Reese".into()),
                    mix: None,
                },
                AuditionCandidate { track: "the whole mix".into(), clip: None, label: None, mix: Some(true) },
            ],
            from_beat: Some(64.0),
            beats: Some(16.0),
            reference: Some("references/reese.wav".into()),
            reference_from: Some(12.5),
            reference_seconds: Some(8.0),
            focus: Some(Focus::Sound),
        },
        clips: Some(true),
        slots: vec![Slot {
            name: "Kumi · Goal · Reese".into(),
            label: "Reese".into(),
            chain: "Wavetable > Auto Filter".into(),
            knobs: vec![knob("t5/d0/p3", "Filter Freq", 0.42), Knob { step: Some(1.0), ..knob("t5/d0/p7", "Osc 1 Pos", 0.25) }],
            elite: vec![0.4, 0.3],
            score: Some(71.5),
            sigma: 0.12,
            stale: 2,
            heard: Some(3),
        }],
        generation: 9,
        rendered: 54,
        trend: vec![52.0, 60.5, 71.5],
        first: Some(52.0),
        elapsed_ms: 1_234_567,
        status: GoalRun::Paused,
        idea: Some("parallel detuned saws".into()),
        best_track: Some("Kumi · Goal best".into()),
        why: Some("paused by the producer".into()),
        lesson: Some("l0a1b2c3d".into()),
    }
}

fn kept_job() -> KeptJob {
    KeptJob {
        status: JobStatus {
            id: 3,
            kind: JobKind::Worker,
            title: "Find a breakbeat tutorial".into(),
            state: JobState::Done,
            when_done: WhenDone::Continue,
            doing: None,
            tool: None,
            started_at: AT,
            ended_at: Some(AT + 95_000),
            steps: 7,
            sources: 2,
            usage: Some(Usage { input_tokens: 12_000.0, output_tokens: 850.0, cache_read_tokens: 9_000.0, cache_write_tokens: 0.0 }),
            line: Some("Two tutorials show the Amen chop.".into()),
            moved: true,
        },
        spec: JobSpec {
            title: "Find a breakbeat tutorial".into(),
            task: Some("Find a tutorial on chopping the Amen break".into()),
            tool: None,
            args: None,
            when_done: WhenDone::Continue,
            request: Some("chop a break like in that video".into()),
        },
        report: Some("Two tutorials show the Amen chop: slice at 1/16, then re-sequence.".into()),
    }
}

fn tool_job() -> KeptJob {
    KeptJob {
        status: JobStatus {
            id: 4,
            kind: JobKind::Tool,
            title: "Read the manual".into(),
            state: JobState::Interrupted,
            when_done: WhenDone::Tell,
            doing: None,
            tool: None,
            started_at: AT + 1_000,
            ended_at: Some(AT + 2_000),
            steps: 0,
            sources: 0,
            usage: None,
            line: Some("Kumi closed while it ran: /jobs runs it again.".into()),
            moved: false,
        },
        spec: JobSpec {
            title: "Read the manual".into(),
            task: None,
            tool: Some("live_manual".into()),
            args: Some(json!({"topic":"Wavetable"}).as_object().unwrap().clone()),
            when_done: WhenDone::Tell,
            request: None,
        },
        report: None,
    }
}

#[tokio::test]
async fn goals_objectives_and_jobs_keep_their_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let goals = dir.path().join("goals");
    create_goal_store(&goals).save(A, &goal_state()).await.unwrap();
    let objectives = create_objective_store(&goals);
    let mut objective = Objective::new("make the drop hit harder", BUDGET);
    objective.turns = 4;
    objective.elapsed_ms = 600_000;
    objective.idle = 1;
    objective.state = ObjectiveState::Paused;
    objective.conversation = Some(ID1.into());
    objective.last = Some(Check {
        verdict: Verdict::Blocked,
        reason: "the kick has no room".into(),
        next: Some("cut 60 Hz on the bass".into()),
        measured: true,
    });
    objectives.save(A, &objective).await.unwrap();
    let mut unsaved = Objective::new("a warmer pad", BUDGET);
    unsaved.last = Some(Check { verdict: Verdict::Continue, reason: "not met yet".into(), next: None, measured: false });
    objectives.save("unsaved:7f3a", &unsaved).await.unwrap();
    create_job_store(&goals).save(A, &[kept_job(), tool_job()]).await.unwrap();
    golden("goals", &files(dir.path()));

    let again = tempfile::tempdir().unwrap();
    put(again.path(), &files(&goldens("goals")));
    let goals = again.path().join("goals");
    let store = create_goal_store(&goals);
    let state = store.load(A).await.unwrap().unwrap();
    assert_eq!(state, goal_state());
    store.save(A, &state).await.unwrap();
    let objectives = create_objective_store(&goals);
    for place in [A, "unsaved:7f3a"] {
        let objective = objectives.load(place).await.unwrap().unwrap();
        objectives.save(place, &objective).await.unwrap();
    }
    let jobs = create_job_store(&goals);
    let kept = jobs.take(A).await.unwrap();
    assert_eq!(kept, [kept_job(), tool_job()]);
    jobs.save(A, &kept).await.unwrap();
    same("goals", &files(again.path()), "changed when read and saved again");
}

fn recipe() -> Recipe {
    Recipe {
        version: 1,
        name: "Dark Reese bass".into(),
        about: "A detuned Reese with a slow filter".into(),
        params: vec![
            RecipeParam { name: "track".into(), about: "The bass track".into() },
            RecipeParam { name: "cutoff".into(), about: "Where the filter sits".into() },
        ],
        steps: vec![
            json!({"tool":"load_device","trackRef":"$track","name":"Wavetable"}).as_object().unwrap().clone(),
            json!({"tool":"set_parameter","trackRef":"$track","device":"Wavetable","name":"Filter Freq","value":"$cutoff"})
                .as_object()
                .unwrap()
                .clone(),
        ],
        created: (AT - 86_400_000) as f64,
        used: 3.0,
        last_used: Some(AT as f64),
        daw: None,
    }
}

#[tokio::test]
async fn recipes_techniques_and_lessons_keep_their_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let recipes = create_recipe_store(root.join("recipes"));
    recipes.save(&recipe()).await.unwrap();
    recipes
        .save(&Recipe { name: "Sidechain pump".into(), about: String::new(), params: vec![], last_used: None, used: 0.0, ..recipe() })
        .await
        .unwrap();

    let techniques = create_technique_store(root.join("techniques.json"));
    let body = |name: &str| TechniqueBody {
        name: name.into(),
        fits: "dark basses".into(),
        idea: "Two saws detuned 7 cents into a 24 dB low-pass, its cutoff moved by a slow LFO.".into(),
        settings: None,
        substitutes: None,
        recipe: None,
        source: None,
    };
    let (reese, _) =
        techniques.keep(TechniqueDraft { body: body("Reese"), replaces: None }, Some("make a Reese".into()), AT as f64).await.unwrap();
    let full = TechniqueBody {
        settings: Some("Cutoff 300 Hz, resonance 20%".into()),
        substitutes: Some("Serum: two saws, unison 2".into()),
        recipe: Some("Dark Reese bass".into()),
        source: Some(TechniqueSource { title: Some("Reese bass in 5 minutes".into()), url: Some("https://example.com/reese".into()) }),
        ..body("Wobble")
    };
    techniques.keep(TechniqueDraft { body: full, replaces: None }, None, (AT + 1_000) as f64).await.unwrap();
    techniques.record(&reese.id, false, (AT + 2_000) as f64).await.unwrap();
    techniques.record(&reese.id, true, (AT + 3_000) as f64).await.unwrap();
    techniques
        .keep(TechniqueDraft { body: body("Reese"), replaces: Some(reese.id.clone()) }, Some("darker".into()), (AT + 4_000) as f64)
        .await
        .unwrap();

    let playbook = create_playbook_store(root.join("playbook.json"));
    let lesson = |id: &str, at: i64, reaction: Option<Reaction>| Lesson {
        id: id.into(),
        at: at as f64,
        matched: "C2 · bright, rich · attack 15 ms".into(),
        winner: "Wavetable > Auto Filter".into(),
        from: 52.0,
        to: 88.0,
        moves: vec![Best { label: "detune".into(), score: 71.0 }, Best { label: "filter".into(), score: 88.0 }],
        reaction,
    };
    playbook.put(&lesson("l0a1b2c3d", AT, None)).await.unwrap();
    playbook.put(&lesson("l1b2c3d4e", AT + 1_000, None)).await.unwrap();
    playbook.react("l0a1b2c3d", Reaction::Liked).await.unwrap();
    golden("learned", &files(root));

    let again = tempfile::tempdir().unwrap();
    let root = again.path();
    put(root, &files(&goldens("learned")));
    let recipes = create_recipe_store(root.join("recipes"));
    for recipe in recipes.list().await.unwrap() {
        recipes.save(&recipe).await.unwrap();
    }
    let techniques = create_technique_store(root.join("techniques.json"));
    techniques.save(&techniques.list().await.unwrap()).await.unwrap();
    let playbook = create_playbook_store(root.join("playbook.json"));
    playbook.save(&playbook.list().await.unwrap()).await.unwrap();
    same("learned", &files(root), "changed when read and saved again");
}

/// A log line with the time it was written (now) and Kumi's version (which moves with every release) fixed.
fn normalised_line(line: &str) -> String {
    let mut value: Value = serde_json::from_str(line).unwrap();
    value["at"] = json!("2025-10-09T08:53:20.000Z");
    if value["kumi"] == KUMI_VERSION {
        value["kumi"] = json!("<version>");
    }
    stringify(&value)
}

fn normalised_log(text: &str) -> String {
    text.lines().map(|line| format!("{}\n", normalised_line(line))).collect()
}

#[tokio::test]
async fn gaps_and_timings_logs_keep_their_keys() {
    let dir = tempfile::tempdir().unwrap();
    let gaps = dir.path().join("gaps.jsonl");
    let tool = gap_tools(&gaps, None).remove(0);
    for input in [
        json!({"missing":"setting Operator's voice count","asked":"make the pad mono","workaround":"used a Utility instead"}),
        json!({"missing":"reading a clip's warp markers"}),
    ] {
        let result = tool.execute(input.as_object().unwrap().clone(), abort::never()).await.unwrap();
        assert!(!result.is_error, "{}", result.text);
    }
    let timings = dir.path().join("timings.jsonl");
    let turn = TurnTiming {
        model: Some("openai/gpt-5.5".into()),
        models: vec![("ollama/qwen3".into(), 1), ("openai/gpt-5.5".into(), 2)],
        effort: Some("high".into()),
        tier: Some("priority".into()),
        model_calls: 3,
        model_ms: 4_200,
        first_part_ms: vec![800, 650, 700],
        call_tokens: vec![(12_000, 9_000), (13_000, 12_000), (13_500, 13_000)],
        tools: 4,
        tool_ms: 1_900,
        by_tool: BTreeMap::from([
            ("make_changes".to_string(), (2, 1_500)),
            ("observe".to_string(), (1, 300)),
            ("listen".to_string(), (1, 100)),
            ("undo_change".to_string(), (0, 0)),
        ]),
        live_requests: 6,
        live_bytes: 21_480,
        set_reused: Some(true),
        set_drift: Some(2),
        sent_bytes: 64_000,
        files_ms: Some(12),
        look_ms: Some(140),
        look_requests: 2,
        look_bytes: 9_000,
    };
    let usage = Usage { input_tokens: 38_500.0, output_tokens: 1_200.0, cache_read_tokens: 34_000.0, cache_write_tokens: 4_000.0 };
    timing::append(&timings, &timing::line(&turn, 6_500, json!("completed"), Some(&usage)));
    timing::append(&timings, &timing::line(&TurnTiming { set_reused: Some(false), ..TurnTiming::default() }, 20, json!("error"), None));
    let mut written = files(dir.path());
    for text in written.values_mut() {
        *text = normalised_log(text);
    }
    golden("logs", &written);

    // The gap log as `kumi report` reads it back, each gap as the log writes it.
    let again = tempfile::tempdir().unwrap();
    put(again.path(), &files(&goldens("logs")));
    let db = again.path().join("kumi.db");
    let _open = kumi_store::Store::open(&db).unwrap();
    let logged = kumi_runtime::core::gaps::logged_gaps(&db, &again.path().join("gaps.jsonl")).unwrap();
    let read = logged.iter().map(|line| format!("{line}\n")).collect::<String>();
    assert!(read == files(&goldens("logs"))["gaps.jsonl"], "gaps.jsonl changed when read back\n{read}");
}

fn set_entry() -> SetEntry {
    let track = |name: &str, kind: TrackKind| SetTrack { name: name.into(), kind, ..SetTrack::default() };
    SetEntry {
        file: Entry { path: "/Music/Night Drive Project/Night Drive.als".into(), size: 48_213, mtime: AT - 60_000, gone: None },
        set: Some(SetSummary {
            name: "Night Drive".into(),
            tracks: vec![
                SetTrack {
                    devices: vec![
                        SetDevice {
                            name: "Instrument Rack".into(),
                            role: DeviceRole::Rack,
                            plugin: None,
                            preset: Some("Reese Stack".into()),
                            inside: Some(vec!["Wavetable".into(), "Operator".into()]),
                        },
                        SetDevice {
                            name: "Serum 2".into(),
                            role: DeviceRole::Instrument,
                            plugin: Some("vst3".into()),
                            preset: None,
                            inside: None,
                        },
                        SetDevice { name: "EQ Eight".into(), role: DeviceRole::Audio, ..SetDevice::default() },
                    ],
                    clips: Clips { session: 4, arrangement: 12 },
                    color: Some(12.0),
                    group: Some("Low End".into()),
                    plugins: Some(vec!["Serum 2".into()]),
                    frozen: Some(true),
                    ..track("Bass", TrackKind::Midi)
                },
                SetTrack {
                    samples: vec!["Samples/Imported/vox take 3.wav".into()],
                    devices: vec![SetDevice { name: "Arpeggiator".into(), role: DeviceRole::Midi, ..SetDevice::default() }],
                    ..track("Vox", TrackKind::Audio)
                },
                track("Low End", TrackKind::Group),
            ],
            returns: vec![track("A-Reverb", TrackKind::Return)],
            scenes: 8,
            tempo: Some(124.0),
            signature: Some("4/4".into()),
            key: Some("A minor".into()),
            main: Some(track("Main", TrackKind::Main)),
            arrangement_beats: Some(512.0),
            live: Some("12.2".into()),
            daw: None,
        }),
        error: None,
    }
}

/// The log's header with its generation (a fresh id) and when it was made (now) fixed.
fn normalised_sets(text: &str) -> String {
    let (header, rest) = text.split_once('\n').unwrap();
    let mut header: Value = serde_json::from_str(header).unwrap();
    header["generation"] = json!("00000000-0000-0000-0000-000000000000");
    header["created"] = json!(0);
    format!("{}\n{rest}", stringify(&header))
}

#[tokio::test]
async fn the_library_log_of_sets_keeps_its_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let logs = library_logs(dir.path().join("library").to_str().unwrap());
    logs.sets.write([&set_entry()]).await.unwrap();
    let broken = SetEntry {
        file: Entry { path: "/Music/Broken.als".into(), size: 10, mtime: AT, gone: None },
        set: None,
        error: Some("not a Live Set".into()),
    };
    logs.sets.append(&[broken]).await.unwrap();
    let mut written = files(dir.path());
    for text in written.values_mut() {
        *text = normalised_sets(text);
    }
    golden("library", &written);

    let again = tempfile::tempdir().unwrap();
    put(again.path(), &files(&goldens("library")));
    let logs = library_logs(again.path().join("library").to_str().unwrap());
    let entries = logs.sets.load().await;
    assert_eq!(entries.values().next(), Some(&set_entry()));
    logs.sets.write(entries.values()).await.unwrap();
    let mut read = files(again.path());
    for text in read.values_mut() {
        *text = normalised_sets(text);
    }
    same("library", &read, "changed when read and written again");
    logs.sets.append(&[Entry::gone("/Music/Broken.als")]).await.unwrap();
    let text = std::fs::read_to_string(again.path().join("library/sets.jsonl")).unwrap();
    assert_eq!(text.lines().last(), Some(r#"{"path":"/Music/Broken.als","size":0,"mtime":0,"gone":true}"#));
}

/// kumi.db's schema version, then its notes and observations, oldest first, each row's own id (a fresh one) left out.
fn rows(client: &StoreClient) -> String {
    client
        .store()
        .read(|c| {
            let version: i64 = c.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let mut lines = vec![stringify(&json!({"userVersion":version}))];
            let mut notes = c.prepare(
                "SELECT scope_kind, scope_id, label, text, pinned, created_at, updated_at, archived_at FROM notes
                 ORDER BY scope_kind, scope_id, created_at, rowid",
            )?;
            let found = notes.query_map([], |row| {
                Ok(json!({"table":"notes","scopeKind":row.get::<_, String>(0)?,"scopeId":row.get::<_, String>(1)?,
                    "label":row.get::<_, String>(2)?,"text":row.get::<_, String>(3)?,"pinned":row.get::<_, i64>(4)?,
                    "createdAt":row.get::<_, i64>(5)?,"updatedAt":row.get::<_, i64>(6)?,"archivedAt":row.get::<_, Option<i64>>(7)?}))
            })?;
            for row in found {
                lines.push(stringify(&row?));
            }
            let mut observations = c.prepare(
                "SELECT at, session, project, kind, weight, heard, json(subject), json(facts), json(context) FROM observations
                 ORDER BY at, rowid",
            )?;
            let found = observations.query_map([], |row| {
                let parsed = |at: usize| row.get::<_, String>(at).map(|text| serde_json::from_str::<Value>(&text).unwrap());
                Ok(json!({"table":"observations","at":row.get::<_, i64>(0)?,"session":row.get::<_, String>(1)?,
                    "project":row.get::<_, Option<String>>(2)?,"kind":row.get::<_, String>(3)?,"weight":row.get::<_, Option<f64>>(4)?,
                    "heard":row.get::<_, Option<i64>>(5)?,"subject":parsed(6)?,"facts":parsed(7)?,"context":parsed(8)?}))
            })?;
            for row in found {
                lines.push(stringify(&row?));
            }
            Ok(lines.into_iter().map(|line| format!("{line}\n")).collect())
        })
        .unwrap()
}

#[tokio::test]
async fn kumi_db_rows_about_a_set_keep_their_shape() {
    let dir = tempfile::tempdir().unwrap();
    let client = StoreClient::new(kumi_store::Store::open(dir.path().join("kumi.db")).unwrap());
    let memory = SqliteMemoryStore::new(client.clone());
    memory.remember(MemoryScope::Producer, None, "Likes short, dark reverbs on drums", None, AT).await.unwrap();
    memory.change(MemoryScope::Producer, None, "p1", NoteChange::Pinned(true), AT + 500).await.unwrap();
    // A full list of notes about the Set: the next one sets the oldest aside.
    for n in 0..=MAX_NOTES {
        memory
            .remember(MemoryScope::Set, Some(A), &format!("Track {n} is part of the drop"), None, AT + 1_000 * (n as i64 + 1))
            .await
            .unwrap();
    }
    memory.remember(MemoryScope::Set, Some(A), "Track 2 is the lead", Some("s3"), AT + 100_000).await.unwrap();
    memory.change(MemoryScope::Set, Some(A), "s4", NoteChange::Text("Track 3 is the pad".into()), AT + 101_000).await.unwrap();

    let fingerprint = json!({"name":"Night Drive","tempo":124,"meter":"4/4","roles":["bass","drums"]});
    let log = TasteLog::with_clock(
        client.clone(),
        Rc::new(move || Whereabouts { session: ID1.into(), project: Some(A.into()), set: Some(fingerprint.clone()), daw: None }),
        Rc::new(|_: i64| None),
        Rc::new(|| AT + 200_000),
    );
    log.turn_started("make the bass darker", true);
    log.picked(&Picked::Answer { question: "Which bass?".into(), options: vec!["Reese".into(), "Sub".into()], index: 1 });
    log.picked(&Picked::Technique { name: "Reese".into(), keep: true });
    log.change(&ChangeRecord {
        track: Some(TrackChip { name: "Bass".into(), color: None }),
        from: Some(0.85),
        to: Some(0.775),
        range: Some([0.0, 1.0]),
        ..change("c2", ChangeFamily::Mixer, "Bass volume -6 → -9 dB", ChangeState::Applied, AT + 150_000)
    });
    log.undone("c2", UndoneBy::Producer);
    // The log writes without waiting: a write after it commits once they have.
    client.write(|_| Ok(())).await.unwrap();
    let written = rows(&client);
    golden("kumi-db", &BTreeMap::from([("rows.jsonl".to_string(), written.clone())]));

    let kept = memory.load(Some(A)).await.unwrap();
    memory.save(MemoryScope::Producer, None, &kept.producer).await.unwrap();
    memory.save(MemoryScope::Set, Some(A), &kept.set).await.unwrap();
    assert!(rows(&client) == written, "kumi.db's notes changed when read and saved again\n{}", first_difference(&rows(&client), &written));
}

#[test]
fn a_set_s_project_id_and_where_it_keeps_it_stay_the_same() {
    assert_eq!(project_id_of("/Music/Night Drive.als"), A);
    assert_eq!(PROJECT_KEY, "kumi.project");
    assert_eq!(IMPORTED, "imported");
}

#[tokio::test]
async fn conversations_are_kept_only_under_a_project_id_unsaved_or_imported() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store_keeping(dir.path(), None);
    let said = conversation(AT, json!([{"role":"user","content":"Hello"}]));
    for place in [A, "unsaved", "imported"] {
        store.save(place, ID1, &said).await.unwrap_or_else(|e| panic!("{place}: {e}"));
        assert!(dir.path().join(place).join("conversations").join(format!("{ID1}.json")).is_file(), "{place}");
    }
    for place in [
        "",
        "Unsaved",
        "UNSAVED",
        "unsaved:7f3a",
        "A2DFD5F2CF329C593C5FDC7448524087",
        "a2dfd5f2cf329c593c5fdc744852408",
        "a2dfd5f2cf329c593c5fdc7448524087a",
        "reaper-unsaved",
        "renoise-a2dfd5f2cf329c593c5fdc7448524087",
        "../unsaved",
        "projects",
    ] {
        assert!(store.save(place, ID1, &said).await.is_err(), "{place:?} was taken for a place");
    }
    // A place's Set name comes from its baseline; "unsaved" has none.
    let index: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("conversation-index.json")).unwrap()).unwrap();
    assert_eq!(index["places"]["unsaved"], json!({"modified":0}));
}

#[tokio::test]
async fn set_notes_are_kept_only_under_a_project_id_and_goals_of_no_place_go_to_unsaved() {
    let dir = tempfile::tempdir().unwrap();
    let memory = create_memory_store(MemoryStoreOptions {
        projects_dir: dir.path().join("projects"),
        producer_file: dir.path().join("memory.json"),
    });
    for project in ["unsaved", "A2DFD5F2CF329C593C5FDC7448524087", "reaper-a2dfd5f2cf329c593c5fdc7448524087"] {
        assert!(memory.save(MemoryScope::Set, Some(project), &[]).await.is_err(), "{project}");
    }
    memory.save(MemoryScope::Set, Some(A), &[]).await.unwrap();
    assert!(dir.path().join("projects").join(A).join("memory.json").is_file());
    let goals = create_goal_store(dir.path().join("goals"));
    goals.save("", &goal_state()).await.unwrap();
    assert!(dir.path().join("goals/unsaved.json").is_file());
}
