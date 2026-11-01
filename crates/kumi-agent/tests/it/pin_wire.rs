//! What Kumi sends the app and keeps in saved conversations, byte for byte: one of every session event (each optional
//! field filled, and each value of the enums they carry), the views FOCUS draws, what the session controller hands the
//! app, and change records of every family, state and picture kind. Each golden reads back as the same value and writes
//! the same bytes.
use kumi_agent::core::{
    contracts::{
        ActionEvent, ArrangementLoop, ArrangementStrip, Attachment, BodyRequest, CatchUp, ChainNode, ClipView, ClipViewNote,
        ConnectionState, ConversationNow, ConversationSummary, DeviceNode, DeviceTree, DeviceType, DisconnectCause, FocusParameter,
        FocusTrack, KernelEvent, LessonAction, LibraryEvent, LiveDetail, LiveFocus, LiveTransport, LiveView, Locator, Memory, MemoryEvent,
        MemoryNote, MemoryScope, MemoryView, MusicEvent, NoteChange, Picked, PinKind, PinnedNode, PinnedTime, RecipeAction, RecipeEvent,
        Reference, SessionEvent, SessionSlot, SessionStatus, SessionStrip, SetAsk, SetChoice, SlotClip, StopReason, TechniqueAction,
        TechniqueEvent, TechniqueSummary, TrackKind, TranscriptLine, TranscriptRole, TurnResult, TurnState, WaitFor,
    },
    goal::{Best, GoalPhase, GoalStatus},
    goal_mode::{ObjectiveState, ObjectiveStatus, Verdict},
    jobs::{JobKind, JobState, JobStatus, WhenDone},
    loop_run::{LoopState, LoopStatus, LoopStop},
    match_run::{MatchState, MatchStatus, MatchStop},
    palette::{Gathered, PaletteFile, PaletteView},
    protocol::{op, Capabilities, Op, Secret},
    session_log::{LogHeader, LogLine, LogRequest, Replay},
};
use kumi_listening::{
    checklist::{Change as RowChange, Row},
    detect::{Problem, ProblemKind},
    music::learning::{MusicState, MusicStatus},
    round::{Next, Round, RoundKind},
};
use kumi_media::audio::matching::Focus;
use kumi_net::cause::NetCause;
use kumi_types::picture::{
    Dynamics, EqBand, EqShape, MixLevels, MixSend, PictureItem, PictureLine, PictureMark, PictureRoute, PictureTrack, PictureValue,
    PictureZone, RouteChange, TrackMark,
};
use kumi_types::{
    contracts::{
        AuditionCandidate, AuditionEvent, AuditionRequest, BestTake, ChainPlacement, ChangeFamily, ChangePicture, ChangeRecord,
        ChangeState, ClipNote, ClipPicture, ColorChange, DevicePlacement, HeardComparison, HeardEvent, LibraryState, LibraryStatus,
        SoundSpan, StructuralMove, TakeScore, Thumb, TrackChip, Usage, WatchedEvent, WatchedFrame, WebAction, WebEvent, WebWhere, Where,
        WordsSource,
    },
    errors::FailureKind,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::BTreeSet, fmt::Debug, path::Path};

const AT: i64 = 1_760_000_000_000;

fn golden_file(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/wire").join(name)
}

/// Where two texts first differ: the line's number, the lines before it, and the next few lines of each.
fn first_difference(actual: &str, expected: &str) -> String {
    if actual.replace("\r\n", "\n") == expected.replace("\r\n", "\n") {
        return "only the line endings differ (CRLF against LF)".into();
    }
    let (actual, expected): (Vec<&str>, Vec<&str>) = (actual.split('\n').collect(), expected.split('\n').collect());
    let line = (0..actual.len().max(expected.len())).find(|&i| actual.get(i) != expected.get(i)).unwrap_or(0);
    let mut text = format!("first difference at line {} (- expected, + actual):\n", line + 1);
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

/// Checks `actual` against the golden `name` (KUMI_BLESS=1 writes it instead).
fn golden(name: &str, actual: &str) {
    let file = golden_file(name);
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    assert!(
        actual == expected,
        "{} changed: check the difference, then run this test with KUMI_BLESS=1\n{}",
        file.display(),
        first_difference(actual, &expected)
    );
}

fn pretty<T: Serialize>(value: &T) -> String {
    let mut json = serde_json::to_string_pretty(value).unwrap();
    json.push('\n');
    json
}

/// `value` as JSON against its golden; the golden reads back as the same value and writes the same bytes.
fn pinned<T: Serialize + DeserializeOwned + PartialEq + Debug>(name: &str, value: &T) {
    golden(name, &pretty(value));
    let text = std::fs::read_to_string(golden_file(name)).unwrap();
    let back: T = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} doesn't read back: {e}"));
    assert!(back == *value, "{name} reads back as another value\n{}", first_difference(&format!("{back:#?}"), &format!("{value:#?}")));
    let again = pretty(&back);
    assert!(again == text, "{name} read back writes other bytes\n{}", first_difference(&again, &text));
}

const TURN_STATES: [TurnState; 4] = [TurnState::Idle, TurnState::Running, TurnState::Cancelling, TurnState::Closed];
const CONNECTION_STATES: [ConnectionState; 4] =
    [ConnectionState::Disconnected, ConnectionState::Connecting, ConnectionState::Connected, ConnectionState::Error];
const DISCONNECT_CAUSES: [DisconnectCause; 5] =
    [DisconnectCause::Live, DisconnectCause::Bridge, DisconnectCause::Set, DisconnectCause::AskedSet, DisconnectCause::Busy];
const FAILURE_KINDS: [FailureKind; 12] = [
    FailureKind::Auth,
    FailureKind::Billing,
    FailureKind::Model,
    FailureKind::Config,
    FailureKind::Live,
    FailureKind::RateLimit,
    FailureKind::UsageLimit,
    FailureKind::Request,
    FailureKind::Provider,
    FailureKind::Network,
    FailureKind::Protocol,
    FailureKind::Output,
];
const NET_CAUSES: [NetCause; 5] = [NetCause::Dns, NetCause::Tls, NetCause::Proxy, NetCause::Timeout, NetCause::Offline];
const PIN_KINDS: [PinKind; 8] = [
    PinKind::Device,
    PinKind::Instrument,
    PinKind::Chain,
    PinKind::Track,
    PinKind::Clip,
    PinKind::Scene,
    PinKind::ClipSlot,
    PinKind::Selection,
];
const TRACK_KINDS: [(TrackKind, &str); 5] = [
    (TrackKind::Midi, "Keys"),
    (TrackKind::Audio, "Vox"),
    (TrackKind::Group, "Drums"),
    (TrackKind::Return, "A-Reverb"),
    (TrackKind::Main, "Main"),
];
const FAMILIES: [ChangeFamily; 9] = [
    ChangeFamily::Tempo,
    ChangeFamily::Mixer,
    ChangeFamily::Rename,
    ChangeFamily::Structure,
    ChangeFamily::Clip,
    ChangeFamily::Device,
    ChangeFamily::Parameter,
    ChangeFamily::Locators,
    ChangeFamily::Color,
];
const STATES: [ChangeState; 6] =
    [ChangeState::Applied, ChangeState::Undone, ChangeState::Kept, ChangeState::Unsure, ChangeState::Expired, ChangeState::Heard];

fn note(start: f64, pitch: f64, duration: f64, velocity: f64) -> ClipNote {
    ClipNote { pitch, start, duration, velocity }
}

fn full_focus() -> LiveFocus {
    LiveFocus {
        track: Some(FocusTrack { name: "Bass".into(), color: Some("#f59a3c".into()), kind: Some(TrackKind::Midi) }),
        track_ref: Some("7:track:0".into()),
        slot_ref: Some("7:clip_slot:0:1".into()),
        scene_index: Some(1.0),
        device_ref: Some("7:device:0:0".into()),
        scene: Some("Verse".into()),
        clip: Some("Bassline".into()),
        device: Some("Operator".into()),
        parameter: Some(FocusParameter { name: "Filter Freq".into(), value: Some("1.20 kHz".into()), owner: Some("Operator".into()) }),
        chain: Some("Sub".into()),
        view: Some(LiveView::Session),
        detail: Some(LiveDetail::Clip),
        browser: Some(true),
        selected_notes: Some(3),
        ..Default::default()
    }
}

fn pin(node: PinKind, r: &str, name: &str) -> PinnedNode {
    PinnedNode {
        track_ref: "7:track:0".into(),
        r#ref: r.into(),
        node,
        name: name.into(),
        trail: vec![],
        siblings: vec![],
        track: None,
        live: None,
        time: None,
        layout: None,
    }
}

fn pins() -> Vec<PinnedNode> {
    PIN_KINDS
        .iter()
        .map(|&kind| match kind {
            PinKind::Device => PinnedNode {
                trail: vec!["Drum Rack".into(), "Kick".into()],
                siblings: vec!["Saturator".into(), "EQ Eight".into()],
                track: Some("Drums".into()),
                live: Some(true),
                ..pin(kind, "7:device:0:0:0:0", "Simpler")
            },
            PinKind::Chain => PinnedNode { trail: vec!["Drum Rack".into()], ..pin(kind, "7:chain:0:0:1", "Snare") },
            // Renoise's: on no track, its refs read at a layout.
            PinKind::Instrument => PinnedNode { track_ref: String::new(), layout: Some(4), ..pin(kind, "1:instrument:1", "808 Kit") },
            PinKind::Track => PinnedNode { track: Some("Drums".into()), live: Some(true), ..pin(kind, "7:track:0", "Drums") },
            PinKind::Clip => PinnedNode { live: Some(false), ..pin(kind, "7:clip:0:1", "Bassline") },
            PinKind::Scene => PinnedNode { track_ref: String::new(), live: Some(true), ..pin(kind, "7:scene:1", "Verse") },
            PinKind::ClipSlot => PinnedNode { live: Some(true), ..pin(kind, "7:clip_slot:0:2", "") },
            PinKind::Selection => PinnedNode {
                track: Some("Drums".into()),
                live: Some(true),
                time: Some(PinnedTime { from_beat: 16.0, to_beat: 32.5 }),
                ..pin(kind, "", "bars 5–8")
            },
        })
        .collect()
}

fn usage() -> Usage {
    Usage { input_tokens: 12_400.0, output_tokens: 830.0, cache_read_tokens: 9_000.0, cache_write_tokens: 1_200.0 }
}

fn remembered(scope: MemoryScope) -> MemoryNote {
    let id = if scope == MemoryScope::Set { "s2" } else { "p2" };
    MemoryNote { id: id.into(), text: "Likes the kick dry and short".into(), at: AT, pinned: true }
}

fn audition_request(focus: Focus) -> AuditionRequest {
    AuditionRequest {
        candidates: vec![
            AuditionCandidate { track: "Bass".into(), clip: Some("scene:2".into()), label: Some("A".into()), mix: None },
            AuditionCandidate { track: "the whole mix".into(), clip: None, label: Some("B".into()), mix: Some(true) },
        ],
        from_beat: Some(64.0),
        beats: Some(16.0),
        reference: Some("references/night-drive.wav".into()),
        reference_from: Some(12.5),
        reference_seconds: Some(8.0),
        focus: Some(focus),
    }
}

fn round(kind: RoundKind) -> Round {
    let row = |id: &str, change: RowChange, before: Option<f64>, after: Option<f64>| Row {
        id: id.into(),
        label: id[..1].to_uppercase() + &id[1..],
        unit: if id == "loudness" { "LUFS".into() } else { String::new() },
        wanted: "-14 ± 1".into(),
        before,
        after,
        gap_before: 1.5,
        gap_after: 0.5,
        change,
    };
    let problem = |kind: ProblemKind, id: &str| Problem {
        kind,
        id: id.into(),
        what: format!("{id} over the rest"),
        hz: None,
        at: vec![],
        excess: 3.0,
        steady: None,
        fix: "a cut".into(),
        source: None,
    };
    Round {
        round: 3,
        kind,
        heard: "bars 49–56, the loudest part".into(),
        target: Some("Harshness".into()),
        change: Some("cut 6.3 kHz by 2 dB on the Vox EQ".into()),
        changes: vec!["EQ Eight Band 4 Gain 0.0 → -2.0 dB".into()],
        rows: vec![
            row("loudness", RowChange::Better, Some(-16.2), Some(-14.4)),
            row("harshness", RowChange::Same, Some(4.5), Some(4.5)),
            row("width", RowChange::Worse, Some(0.8), Some(0.6)),
            row("punch", RowChange::Same, None, None),
        ],
        kept: Some(true),
        why: Some("it closed the harshness gap".into()),
        rebalanced: Some("Vox +0.5 dB".into()),
        listener: Some("brighter, less biting".into()),
        problems: vec![
            Problem {
                hz: Some([5600.0, 7100.0]),
                at: vec![[12.5, 14.0], [30.0, 31.25]],
                steady: Some(true),
                source: Some("Vox".into()),
                ..problem(ProblemKind::Harshness, "harshness 6.3 kHz")
            },
            problem(ProblemKind::Resonance, "resonance 180 Hz"),
            problem(ProblemKind::StereoLows, "stereo lows"),
            problem(ProblemKind::Rumble, "rumble"),
            problem(ProblemKind::DcOffset, "dc offset"),
            problem(ProblemKind::LoudNote, "loud note G1"),
            problem(ProblemKind::Overs, "overs"),
            problem(ProblemKind::Clipping, "clipping"),
            problem(ProblemKind::Masking, "masking Vox"),
        ],
        next: Some(Next {
            id: "width".into(),
            label: "Width".into(),
            gap: 0.2,
            wanted: "0.8".into(),
            now: Some(0.6),
            fix: Some("widen the pads".into()),
        }),
        met: false,
        listens: 4,
        elapsed_ms: 95_000,
    }
}

fn job(id: u32, kind: JobKind, state: JobState, when_done: WhenDone) -> JobStatus {
    JobStatus {
        id,
        kind,
        title: "Read the Operator manual".into(),
        state,
        when_done,
        doing: None,
        tool: None,
        started_at: AT,
        ended_at: None,
        steps: 2,
        sources: 1,
        usage: None,
        line: None,
        moved: false,
    }
}

fn change(id: &str, family: ChangeFamily, title: &str, state: ChangeState) -> ChangeRecord {
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
        at: AT,
        picture: None,
        batch: None,
    }
}

/// One of every session event, with each optional field filled at least once and each value of the enums they carry.
fn events() -> Vec<SessionEvent> {
    let mut events = vec![
        SessionEvent::Focus { focus: Some(full_focus()) },
        SessionEvent::Focus {
            focus: Some(LiveFocus {
                view: Some(LiveView::Arrangement),
                detail: Some(LiveDetail::Device),
                browser: Some(false),
                clip: Some(String::new()),
                ..LiveFocus::default()
            }),
        },
        SessionEvent::Focus { focus: Some(LiveFocus::default()) },
        SessionEvent::Focus { focus: None },
    ];
    for (kind, name) in TRACK_KINDS {
        let track = FocusTrack { name: name.into(), color: None, kind: Some(kind) };
        events.push(SessionEvent::Focus { focus: Some(LiveFocus { track: Some(track), ..LiveFocus::default() }) });
    }
    events.extend(pins().into_iter().map(|pin| SessionEvent::Pointed { pin }));
    events.push(SessionEvent::Change { change: full_changes()[0].clone() });
    events.push(SessionEvent::CatchUp {
        catch_up: CatchUp {
            set: "Night Drive".into(),
            last_seen_at: AT - 86_400_000,
            lines: vec!["Tempo 120 → 124 BPM".into(), "Added track “Pads”".into()],
            more: 2,
            after_reconnect: Some(true),
        },
    });
    events.push(SessionEvent::CatchUp {
        catch_up: CatchUp { set: "Night Drive".into(), last_seen_at: AT, lines: vec![], more: 0, after_reconnect: None },
    });
    events.push(SessionEvent::Resumed {
        saved_at: AT,
        lines: vec![
            TranscriptLine { role: TranscriptRole::User, text: "make the kick punchier".into(), tools: None },
            TranscriptLine {
                role: TranscriptRole::Assistant,
                text: "Shortened the kick's decay.".into(),
                tools: Some(vec!["make_changes".into(), "listen".into()]),
            },
        ],
        changes: Some(vec![change("c1", ChangeFamily::Parameter, "Decay 400 → 250 ms", ChangeState::Expired)]),
        chosen: Some(true),
        unreadable: Some(false),
        replay: None,
    });
    events.push(SessionEvent::Resumed { saved_at: AT, lines: vec![], changes: None, chosen: None, unreadable: Some(true), replay: None });
    // Taken up with its log: the words from before it began, then its last requests as they happened.
    events.push(SessionEvent::Resumed {
        saved_at: AT,
        lines: vec![TranscriptLine { role: TranscriptRole::User, text: "make the kick punchier".into(), tools: None }],
        changes: None,
        chosen: None,
        unreadable: None,
        replay: Some(Replay {
            before: 3,
            earlier: vec![TranscriptLine { role: TranscriptRole::User, text: "make the kick punchier".into(), tools: None }],
            lines: vec![
                LogLine::Request(LogRequest { at: AT, text: "and the snare".into(), files: vec!["snare.wav".into()], pinned: None }),
                LogLine::Event { at: AT + 1, event: SessionEvent::Kernel(KernelEvent::Text { text: "Tightened the snare.".into() }) },
            ],
        }),
    });
    events.push(SessionEvent::SetAsked {
        ask: Some(SetAsk {
            id: "mgj6k3cw0a0b0c".into(),
            title: "make the kick punchier".into(),
            set: Some("Night Drive".into()),
            open: Some("Sunrise".into()),
            daw: "Live".into(),
            noun: "Set".into(),
            by_hand: false,
        }),
    });
    events.push(SessionEvent::SetAsked {
        ask: Some(SetAsk {
            id: "mgj6wybk1a2b3c".into(),
            title: "warmer pads".into(),
            set: None,
            open: None,
            daw: "Live".into(),
            noun: "Set".into(),
            by_hand: false,
        }),
    });
    events.push(SessionEvent::SetAsked { ask: None });
    events.push(SessionEvent::Resend { text: "make the kick punchier".into() });
    events.extend(TURN_STATES.map(|state| SessionEvent::State { state }));
    events.extend(CONNECTION_STATES.map(|state| SessionEvent::Connection { state }));
    events.push(SessionEvent::Observation { label: "Night Drive · 8 tracks · 124 BPM".into(), name: None });
    events.push(SessionEvent::Notice { message: "Kumi lost Live for a moment; it's back.".into() });
    events.extend(FAILURE_KINDS.map(|kind| SessionEvent::Error {
        message: "The model said no.".into(),
        kind: Some(kind),
        provider: Some("openai".into()),
    }));
    events.push(SessionEvent::Error { message: "Something went wrong.".into(), kind: None, provider: None });
    events.push(SessionEvent::TurnComplete {
        result: TurnResult { stop_reason: StopReason::Completed, usage: Some(usage()) },
        elapsed_ms: 4_250,
    });
    events.extend(
        [StopReason::Completed, StopReason::Cancelled, StopReason::MaxSteps]
            .map(|stop_reason| SessionEvent::TurnComplete { result: TurnResult { stop_reason, usage: None }, elapsed_ms: 0 }),
    );
    events.extend([LessonAction::Learned, LessonAction::Updated, LessonAction::Forgot].map(|action| SessionEvent::Lesson {
        action,
        id: "l3".into(),
        line: "Reese bass: two saws detuned 12 cents, then a low-pass at 800 Hz".into(),
    }));
    let best = Best { label: "B".into(), score: 81.5 };
    events.push(SessionEvent::Match(MatchStatus {
        state: MatchState::Running,
        check: 2,
        first: Some(54.0),
        best: Some(best.clone()),
        elapsed_ms: 120_000,
        rounds_left: 10,
        stop: None,
    }));
    events.extend([MatchStop::Reached, MatchStop::Plateau, MatchStop::Budget, MatchStop::NoAudition].map(|stop| {
        SessionEvent::Match(MatchStatus {
            state: MatchState::Done,
            check: 4,
            first: None,
            best: None,
            elapsed_ms: 0,
            rounds_left: 0,
            stop: Some(stop),
        })
    }));
    events.push(SessionEvent::Goal(GoalStatus {
        state: GoalPhase::Running,
        goal: "match the reference's lead".into(),
        generation: 6,
        rendered: 48,
        best: Some(best.clone()),
        first: Some(41.0),
        trend: vec![41.0, 55.5, 63.0, 81.5],
        leader: Some("Lead 2".into()),
        idea: Some("add a second oscillator an octave up".into()),
        elapsed_ms: 900_000,
        candidates: 8,
        best_track: Some("Kumi · Goal best".into()),
        why: Some("closest on brightness and attack".into()),
    }));
    events.extend([GoalPhase::Running, GoalPhase::Paused, GoalPhase::Done, GoalPhase::Starting].map(|state| {
        SessionEvent::Goal(GoalStatus {
            state,
            goal: "match the reference's lead".into(),
            generation: 0,
            rendered: 0,
            best: None,
            first: None,
            trend: vec![],
            leader: None,
            idea: None,
            elapsed_ms: 0,
            candidates: 0,
            best_track: None,
            why: None,
        })
    }));
    let objective = |state: ObjectiveState, verdict: Option<Verdict>| ObjectiveStatus {
        objective: "master to -14 LUFS without harshness".into(),
        state,
        turns: 3,
        turn_budget: 12,
        elapsed_ms: 600_000,
        budget_ms: 3_600_000,
        verdict,
        reason: verdict.map(|_| "loudness is at -15.2 LUFS".into()),
        next: verdict.map(|_| "raise the limiter's gain by 1 dB".into()),
        measured: verdict.is_some(),
    };
    events.extend(
        [ObjectiveState::Running, ObjectiveState::Paused, ObjectiveState::Done]
            .map(|state| SessionEvent::Objective(objective(state, None))),
    );
    events.extend(
        [Verdict::Complete, Verdict::Continue, Verdict::Blocked, Verdict::Stuck, Verdict::Budget]
            .map(|verdict| SessionEvent::Objective(objective(ObjectiveState::Running, Some(verdict)))),
    );
    events.push(SessionEvent::ObjectiveCleared);
    events.push(SessionEvent::Heard(HeardEvent {
        file: "renders/Bass.wav".into(),
        summary: "C2 · warm, round · attack 12 ms".into(),
        bands: vec![-12.5, -8.0, -6.25, -9.0, -14.0, -20.5],
        compared: Some(HeardComparison {
            reference: "references/night-drive.wav".into(),
            summary: "darker than the reference".into(),
            differences: vec![1.5, 0.0, -2.0, -3.5, -4.0, -6.0],
            headlines: vec!["less top end".into(), "softer attack".into()],
        }),
    }));
    events.push(SessionEvent::Heard(HeardEvent {
        file: "renders/Mix.wav".into(),
        summary: "full mix".into(),
        bands: vec![],
        compared: None,
    }));
    events.push(SessionEvent::Auditioned(AuditionEvent {
        round: 2,
        best: Some(BestTake { label: "B".into(), score: 81.5 }),
        previous: Some(74.0),
        takes: vec![
            TakeScore {
                label: "B".into(),
                score: Some(81.5),
                silent: None,
                r#where: Some(Where { track: "Bass".into(), clip: Some("scene:2".into()) }),
            },
            TakeScore { label: "A".into(), score: None, silent: Some(true), r#where: Some(Where { track: "Lead".into(), clip: None }) },
        ],
        gaps: vec!["less low end".into(), "a slower attack".into()],
        request: Some(audition_request(Focus::Sound)),
        structural: Some(StructuralMove { gap: "missing low".into(), r#move: "add a sub oscillator".into() }),
        reference: Some("C2 · bright, rich · attack 15 ms".into()),
    }));
    events.push(SessionEvent::Auditioned(AuditionEvent {
        round: 1,
        best: None,
        previous: None,
        takes: vec![],
        gaps: vec![],
        request: Some(audition_request(Focus::Section)),
        structural: None,
        reference: None,
    }));
    events.extend([RoundKind::Start, RoundKind::Judged, RoundKind::Done].map(|kind| SessionEvent::Judged(round(kind))));
    events.push(SessionEvent::Judged(Round {
        target: None,
        change: None,
        changes: vec![],
        kept: Some(false),
        why: Some("it made the vocal harsher".into()),
        rebalanced: None,
        listener: None,
        problems: vec![],
        next: Some(Next { id: "loudness".into(), label: "Loudness".into(), gap: 1.0, wanted: "-14".into(), now: None, fix: None }),
        met: true,
        ..round(RoundKind::Judged)
    }));
    let loop_status = |state: LoopState, stop: Option<LoopStop>| LoopStatus {
        state,
        request: "make the mix less muddy".into(),
        rounds: 5,
        kept: 3,
        reverted: 2,
        listens: 7,
        elapsed_ms: 420_000,
        rounds_left: 11,
        next: stop.is_none().then(|| "low mids".into()),
        last: Some("kept: cut 250 Hz on the pads".into()),
        stop,
    };
    events.extend([LoopState::Running, LoopState::Paused, LoopState::Done].map(|state| SessionEvent::Loop(loop_status(state, None))));
    events.extend(
        [LoopStop::Met, LoopStop::Stalled, LoopStop::Budget, LoopStop::Unjudged, LoopStop::Ended]
            .map(|stop| SessionEvent::Loop(loop_status(LoopState::Done, Some(stop)))),
    );
    events.push(SessionEvent::Watched(WatchedEvent {
        title: "Reese bass in 5 minutes".into(),
        channel: Some("Sound Design Club".into()),
        url: "https://www.youtube.com/watch?v=abc123".into(),
        duration: Some(312.0),
        from: 60.0,
        to: 125.5,
        chapters: vec!["Oscillators".into(), "Filter".into()],
        words: WordsSource::Captions,
        lines: 42,
        frames: vec![
            WatchedFrame {
                at: 72.5,
                zoom: Some("the filter section".into()),
                thumb: Thumb { width: 2, height: 1, rgb: vec![255, 0, 0, 0, 0, 255] },
            },
            WatchedFrame { at: 90.0, zoom: None, thumb: Thumb { width: 1, height: 1, rgb: vec![16, 32, 48] } },
        ],
        sound: Some(SoundSpan { from: 80.0, to: 95.0 }),
        notes: vec!["The captions were automatic.".into()],
    }));
    events.extend([WordsSource::Captions, WordsSource::Automatic, WordsSource::Transcribed, WordsSource::None].map(|words| {
        SessionEvent::Watched(WatchedEvent {
            title: "Mixing drums".into(),
            channel: None,
            url: "https://www.youtube.com/watch?v=def456".into(),
            duration: None,
            from: 0.0,
            to: 30.0,
            chapters: vec![],
            words,
            lines: 0,
            frames: vec![],
            sound: None,
            notes: vec![],
        })
    }));
    events.push(SessionEvent::Web(WebEvent {
        action: WebAction::Searched,
        title: "reese bass ableton".into(),
        url: None,
        r#where: Some(WebWhere::Web),
        via: Some("Exa".into()),
        results: Some(8),
        kind: None,
        files: None,
    }));
    events.push(SessionEvent::Web(WebEvent {
        action: WebAction::Searched,
        title: "max for live lfo".into(),
        url: None,
        r#where: Some(WebWhere::Github),
        via: None,
        results: Some(0),
        kind: None,
        files: None,
    }));
    events.push(SessionEvent::Web(WebEvent {
        action: WebAction::Read,
        title: "Operator reference".into(),
        url: Some("https://example.com/operator".into()),
        r#where: None,
        via: Some("Parallel".into()),
        results: None,
        kind: Some("a GitHub repository".into()),
        files: Some(12),
    }));
    events.push(SessionEvent::Library(LibraryEvent {
        status: LibraryStatus {
            state: LibraryState::Learning,
            sounds: 1200,
            presets: 340,
            sets: 25,
            todo: Some(80),
            done: Some(12),
            learned_at: Some(AT),
        },
    }));
    events.extend([LibraryState::New, LibraryState::Learning, LibraryState::Paused, LibraryState::Ready].map(|state| {
        SessionEvent::Library(LibraryEvent {
            status: LibraryStatus { state, sounds: 0, presets: 0, sets: 0, todo: None, done: None, learned_at: None },
        })
    }));
    events.push(SessionEvent::Music(MusicEvent {
        status: MusicStatus {
            state: MusicState::Learning,
            folders: vec!["~/Music".into()],
            songs: 2400,
            light: 900,
            full: 300,
            skipped: 12,
            cloud: 0,
            learned_at: Some(AT),
            said: Some("Kumi is fetching its style model, LAION-CLAP…".into()),
        },
    }));
    events.extend([MusicState::Off, MusicState::Learning, MusicState::Paused, MusicState::Done, MusicState::Elsewhere].map(|state| {
        SessionEvent::Music(MusicEvent {
            status: MusicStatus { state, folders: vec![], songs: 0, light: 0, full: 0, skipped: 0, cloud: 0, learned_at: None, said: None },
        })
    }));
    events.extend(
        [RecipeAction::Saved, RecipeAction::Updated, RecipeAction::Running, RecipeAction::Forgotten]
            .map(|action| SessionEvent::Recipe(RecipeEvent { action, name: "sidechain-pump".into(), steps: 3 })),
    );
    events.push(SessionEvent::Technique(TechniqueEvent {
        action: TechniqueAction::Offered,
        technique: TechniqueSummary {
            id: String::new(),
            name: "Parallel drum bus".into(),
            fits: "drums that need weight".into(),
            source: Some("https://www.youtube.com/watch?v=abc123".into()),
        },
    }));
    events.extend(
        [TechniqueAction::Offered, TechniqueAction::Kept, TechniqueAction::Updated, TechniqueAction::Used, TechniqueAction::Forgot].map(
            |action| {
                SessionEvent::Technique(TechniqueEvent {
                    action,
                    technique: TechniqueSummary { id: "t2".into(), name: "Parallel drum bus".into(), fits: "drums".into(), source: None },
                })
            },
        ),
    );
    events.push(SessionEvent::Doing { text: "looking at 2:05".into() });
    events.push(SessionEvent::Action(ActionEvent { title: "Playing from bar 17".into(), playing: Some(true), recording: Some(false) }));
    events.push(SessionEvent::Action(ActionEvent { title: "Showed the Clip view".into(), playing: None, recording: None }));
    events.push(SessionEvent::Transport {
        transport: Some(LiveTransport {
            playing: true,
            tempo: Some(124.0),
            beat: Some(65.25),
            at: 1234.5,
            beats_per_bar: Some(4.0),
            meter: None,
            line: None,
        }),
    });
    events.push(SessionEvent::Transport {
        transport: Some(LiveTransport { playing: false, tempo: None, beat: None, at: 0.0, beats_per_bar: None, meter: None, line: None }),
    });
    events.push(SessionEvent::Transport { transport: None });
    events.push(SessionEvent::Watching { on: true });
    events.push(SessionEvent::Watching { on: false });
    events.push(SessionEvent::Job(JobStatus {
        doing: Some("reading page 3".into()),
        tool: Some("read_web".into()),
        usage: Some(usage()),
        moved: true,
        ..job(1, JobKind::Worker, JobState::Running, WhenDone::Continue)
    }));
    events.push(SessionEvent::Job(JobStatus {
        ended_at: Some(AT + 30_000),
        line: Some("Found three Reese bass recipes.".into()),
        ..job(2, JobKind::Tool, JobState::Done, WhenDone::Tell)
    }));
    events.extend(
        [JobState::Running, JobState::Done, JobState::Failed, JobState::Stopped, JobState::Interrupted]
            .map(|state| SessionEvent::Job(job(3, JobKind::Worker, state, WhenDone::Tell))),
    );
    events.push(SessionEvent::CarryOn { job: 1, title: "Read the Operator manual".into() });
    let file = |path: &str, name: &str, class: &str| PaletteFile {
        path: path.into(),
        size: 48_204,
        mtime: AT - 86_400_000,
        name: name.into(),
        song: false,
        class: Some(class.into()),
    };
    events.push(SessionEvent::Palette {
        palette: Some(PaletteView {
            id: "01JB9Z6P1Q2R3S4T5V6W7X8Y9Z".into(),
            name: "Drums".into(),
            reach: 5,
            files: vec![file("/Samples/Kicks/Kick 808.wav", "Kick 808", "kick"), file("/Samples/Hats/Hat 12.wav", "Hat 12", "hat")],
            excluded: vec![file("/Samples/Kicks/Kick Boom.wav", "Kick Boom", "kick")],
            reached: vec!["/Samples/Kicks/Kick 909.wav".into()],
        }),
    });
    events.push(SessionEvent::Palette { palette: None });
    events.push(SessionEvent::TakenBack {
        requests: 2,
        replay: Some(Replay {
            before: 0,
            earlier: vec![],
            lines: vec![LogLine::Request(LogRequest { at: AT, text: "make the kick punchier".into(), files: vec![], pinned: None })],
        }),
    });
    events.push(SessionEvent::TakenBack { requests: 1, replay: None });
    events.push(SessionEvent::Forked {
        from: "01JB9Z6P1Q".into(),
        replay: Some(Replay {
            before: 0,
            earlier: vec![],
            lines: vec![LogLine::Request(LogRequest { at: AT, text: "make the kick punchier".into(), files: vec![], pinned: None })],
        }),
    });
    events.push(SessionEvent::Forked { from: "01JB9Z6P1Q".into(), replay: None });
    events.push(SessionEvent::Aside { id: 2, text: "The kick and the bass share 60 Hz, so".into() });
    events.push(SessionEvent::SlotsProgress { id: 3, line: "Fetching the stem model: 12 of 80 MB".into() });
    events.push(SessionEvent::SignIn { id: 4, url: Some("https://auth.openai.com/authorize?state=kumi".into()), code: None });
    events.push(SessionEvent::SignIn {
        id: 5,
        url: None,
        code: Some(kumi_ai::auth::openai_codex::DevicePrompt {
            url: "https://auth.openai.com/codex/device".into(),
            code: "KUMI-4821".into(),
        }),
    });
    events.extend(
        [
            KernelEvent::Text { text: "Shortened the kick's decay.".into() },
            KernelEvent::ToolInput { id: "call_1".into(), name: "make_changes".into() },
            KernelEvent::ToolStart { id: "call_1".into(), name: "make_changes".into() },
            KernelEvent::ToolEnd { id: "call_1".into(), name: "make_changes".into(), is_error: false, elapsed_ms: 850 },
            KernelEvent::ToolEnd { id: "call_2".into(), name: "listen".into(), is_error: true, elapsed_ms: 0 },
            KernelEvent::Steer { text: "and keep the snare".into() },
            KernelEvent::Retry { reason: "The provider is busy.".into(), wait_ms: 2_000, wait: None, cause: None, proxy_down: false },
            KernelEvent::Retry {
                reason: "Your plan's limit is reached.".into(),
                wait_ms: 3_600_000,
                wait: Some(WaitFor::Limit),
                cause: None,
                proxy_down: false,
            },
            KernelEvent::ModelSwitch { model: "openai/gpt-5.5".into(), reason: "the plan's limit".into(), back: false },
            KernelEvent::ModelSwitch { model: "openai/gpt-5.4".into(), reason: "it's back".into(), back: true },
            KernelEvent::Reasoning { text: "**Weighing the kick's 55 Hz against the bass**\n\n".into() },
            KernelEvent::Usage(usage()),
        ]
        .map(SessionEvent::Kernel),
    );
    events.extend(NET_CAUSES.map(|cause| {
        SessionEvent::Kernel(KernelEvent::Retry {
            reason: "No connection.".into(),
            wait_ms: 5_000,
            wait: Some(WaitFor::Offline),
            cause: Some(cause),
            proxy_down: cause == NetCause::Proxy,
        })
    }));
    for scope in [MemoryScope::Producer, MemoryScope::Set] {
        events.push(SessionEvent::Memory(MemoryEvent::Remembered {
            scope,
            note: remembered(scope),
            replaced: Some(MemoryNote { pinned: false, text: "Likes the kick dry".into(), at: AT - 1_000, ..remembered(scope) }),
            pending: Some(scope == MemoryScope::Set),
        }));
        events.push(SessionEvent::Memory(MemoryEvent::Remembered {
            scope,
            note: MemoryNote { pinned: false, ..remembered(scope) },
            replaced: None,
            pending: None,
        }));
        events.push(SessionEvent::Memory(MemoryEvent::Forgot { scope, note: remembered(scope) }));
    }
    events
}

/// Each event's variant, to check every one is pinned: a variant added later has to be named here.
fn variant(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::Focus { .. } => "Focus",
        SessionEvent::Pointed { .. } => "Pointed",
        SessionEvent::Change { .. } => "Change",
        SessionEvent::CatchUp { .. } => "CatchUp",
        SessionEvent::Resumed { .. } => "Resumed",
        SessionEvent::SetAsked { .. } => "SetAsked",
        SessionEvent::Resend { .. } => "Resend",
        SessionEvent::State { .. } => "State",
        SessionEvent::Connection { .. } => "Connection",
        SessionEvent::Observation { .. } => "Observation",
        SessionEvent::Notice { .. } => "Notice",
        SessionEvent::Error { .. } => "Error",
        SessionEvent::TurnComplete { .. } => "TurnComplete",
        SessionEvent::Lesson { .. } => "Lesson",
        SessionEvent::Match(_) => "Match",
        SessionEvent::Goal(_) => "Goal",
        SessionEvent::Objective(_) => "Objective",
        SessionEvent::ObjectiveCleared => "ObjectiveCleared",
        SessionEvent::Heard(_) => "Heard",
        SessionEvent::Auditioned(_) => "Auditioned",
        SessionEvent::Judged(_) => "Judged",
        SessionEvent::Loop(_) => "Loop",
        SessionEvent::Watched(_) => "Watched",
        SessionEvent::Web(_) => "Web",
        SessionEvent::Library(_) => "Library",
        SessionEvent::Music(_) => "Music",
        SessionEvent::Recipe(_) => "Recipe",
        SessionEvent::Technique(_) => "Technique",
        SessionEvent::Doing { .. } => "Doing",
        SessionEvent::Action(_) => "Action",
        SessionEvent::Transport { .. } => "Transport",
        SessionEvent::Watching { .. } => "Watching",
        SessionEvent::Job(_) => "Job",
        SessionEvent::CarryOn { .. } => "CarryOn",
        SessionEvent::Palette { .. } => "Palette",
        SessionEvent::TakenBack { .. } => "TakenBack",
        SessionEvent::Forked { .. } => "Forked",
        SessionEvent::Aside { .. } => "Aside",
        SessionEvent::SlotsProgress { .. } => "SlotsProgress",
        SessionEvent::SignIn { .. } => "SignIn",
        SessionEvent::Kernel(event) => match event {
            KernelEvent::Text { .. } => "Kernel::Text",
            KernelEvent::ToolInput { .. } => "Kernel::ToolInput",
            KernelEvent::ToolStart { .. } => "Kernel::ToolStart",
            KernelEvent::ToolEnd { .. } => "Kernel::ToolEnd",
            KernelEvent::Steer { .. } => "Kernel::Steer",
            KernelEvent::Retry { .. } => "Kernel::Retry",
            KernelEvent::ModelSwitch { .. } => "Kernel::ModelSwitch",
            KernelEvent::Reasoning { .. } => "Kernel::Reasoning",
            KernelEvent::Usage(_) => "Kernel::Usage",
        },
        SessionEvent::Memory(event) => match event {
            MemoryEvent::Remembered { .. } => "Memory::Remembered",
            MemoryEvent::Forgot { .. } => "Memory::Forgot",
        },
    }
}

const VARIANTS: [&str; 51] = [
    "Focus",
    "Pointed",
    "Change",
    "CatchUp",
    "Resumed",
    "SetAsked",
    "Resend",
    "State",
    "Connection",
    "Observation",
    "Notice",
    "Error",
    "TurnComplete",
    "Lesson",
    "Match",
    "Goal",
    "Objective",
    "ObjectiveCleared",
    "Heard",
    "Auditioned",
    "Judged",
    "Loop",
    "Watched",
    "Web",
    "Library",
    "Music",
    "Recipe",
    "Technique",
    "Doing",
    "Action",
    "Transport",
    "Watching",
    "Job",
    "CarryOn",
    "Palette",
    "TakenBack",
    "Forked",
    "Aside",
    "SlotsProgress",
    "SignIn",
    "Kernel::Text",
    "Kernel::ToolInput",
    "Kernel::ToolStart",
    "Kernel::ToolEnd",
    "Kernel::Steer",
    "Kernel::Retry",
    "Kernel::ModelSwitch",
    "Kernel::Reasoning",
    "Kernel::Usage",
    "Memory::Remembered",
    "Memory::Forgot",
];

#[test]
fn every_session_event_keeps_its_wire_shape() {
    let events = events();
    let variants: BTreeSet<&str> = events.iter().map(variant).collect();
    assert_eq!(variants, BTreeSet::from(VARIANTS), "every session event is pinned");
    pinned::<Vec<SessionEvent>>("session-events.json", &events);
}

/// A conversation's log is saved data that this Kumi and later ones read back, line by line: a line that no longer reads
/// is skipped, so each kind keeps its shape (a header with and without the conversation it copies, a request with and
/// without its files and what it pointed at, an event, a model message, a save and requests taken back).
#[test]
fn every_line_of_a_conversations_log_keeps_its_shape() {
    let header = |from: Option<&str>| {
        LogLine::Header(LogHeader {
            v: 1,
            id: "mv66me0m3fa91c".into(),
            at: AT,
            kumi: "1.25.0".into(),
            before: 12,
            from: from.map(Into::into),
        })
    };
    let lines = vec![
        header(None),
        header(Some("mv66me0m3fa91b")),
        LogLine::Request(LogRequest { at: AT + 1, text: "make the bass wider".into(), files: vec![], pinned: None }),
        LogLine::Request(LogRequest {
            at: AT + 2,
            text: "and this one darker".into(),
            files: vec!["bass.wav".into()],
            pinned: Some(pins()[0].clone()),
        }),
        LogLine::Event { at: AT + 3, event: SessionEvent::Kernel(KernelEvent::Text { text: "Widened it.".into() }) },
        LogLine::Message { at: AT + 4, message: serde_json::json!({"role":"assistant","content":[{"type":"text","text":"Widened it."}]}) },
        LogLine::Saved { at: AT + 5, saved_at: AT + 4, turns: 2 },
        LogLine::TakenBack { at: AT + 6, to: AT + 2 },
    ];
    let kinds: BTreeSet<&str> = lines
        .iter()
        .map(|line| match line {
            LogLine::Header(_) => "header",
            LogLine::Request(_) => "request",
            LogLine::Event { .. } => "event",
            LogLine::Message { .. } => "message",
            LogLine::Saved { .. } => "saved",
            LogLine::TakenBack { .. } => "taken-back",
        })
        .collect();
    assert_eq!(kinds.len(), 6, "every kind of line is pinned");
    pinned::<Vec<LogLine>>("log-lines.json", &lines);
}

/// The views FOCUS draws, as the integration hands them to the app, and the connection's states and causes.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Views {
    device_trees: Vec<DeviceTree>,
    clip_views: Vec<ClipView>,
    session_strips: Vec<SessionStrip>,
    arrangement_strips: Vec<ArrangementStrip>,
    connection_states: Vec<ConnectionState>,
    disconnect_causes: Vec<DisconnectCause>,
}

fn device(r: &str, name: &str, class_name: &str, device_type: DeviceType) -> DeviceNode {
    DeviceNode {
        r#ref: r.into(),
        name: name.into(),
        class_name: Some(class_name.into()),
        can_have_chains: Some(false),
        can_have_drum_pads: Some(false),
        device_type: Some(device_type),
        chains: None,
        role: None,
        note: None,
    }
}

#[test]
fn the_views_focus_draws_keep_their_wire_shape() {
    let drum_rack = DeviceNode {
        can_have_chains: Some(true),
        can_have_drum_pads: Some(true),
        chains: Some(vec![
            ChainNode {
                r#ref: "7:chain:0:0:0".into(),
                name: "Kick".into(),
                devices: Some(vec![
                    device("7:device:0:0:0:0", "Simpler", "OriginalSimpler", DeviceType::Instrument),
                    device("7:device:0:0:0:1", "Saturator", "Saturator", DeviceType::AudioEffect),
                ]),
            },
            ChainNode { r#ref: "7:chain:0:0:1".into(), name: "Snare".into(), devices: None },
            ChainNode { r#ref: "7:chain:0:0:2".into(), name: "Hat".into(), devices: Some(vec![]) },
        ]),
        ..device("7:device:0:0", "Drum Rack", "DrumGroupDevice", DeviceType::Instrument)
    };
    let effect_rack = DeviceNode {
        can_have_chains: Some(true),
        chains: Some(vec![ChainNode {
            r#ref: "7:chain:0:2:0".into(),
            name: "Wet".into(),
            devices: Some(vec![device("7:device:0:2:0:0", "Reverb", "Reverb", DeviceType::AudioEffect)]),
        }]),
        ..device("7:device:0:2", "Audio Effect Rack", "AudioEffectGroupDevice", DeviceType::AudioEffect)
    };
    let views = Views {
        device_trees: vec![
            DeviceTree {
                owner: None,
                layout: None,
                track_ref: "7:track:0".into(),
                devices: vec![
                    device("7:device:0:-1", "Arpeggiator", "MidiArpeggiator", DeviceType::MidiEffect),
                    drum_rack,
                    device("7:device:0:1", "Glue Compressor", "GlueCompressor", DeviceType::AudioEffect),
                    effect_rack,
                    DeviceNode {
                        r#ref: "7:device:0:3".into(),
                        name: "Utility".into(),
                        class_name: None,
                        can_have_chains: None,
                        can_have_drum_pads: None,
                        device_type: None,
                        chains: None,
                        role: None,
                        note: None,
                    },
                ],
            },
            DeviceTree { track_ref: "7:track:3".into(), devices: vec![], owner: None, layout: None },
        ],
        clip_views: vec![
            ClipView {
                slot_ref: "7:clip_slot:0:1".into(),
                name: "Bassline".into(),
                length: 8.0,
                notes: vec![
                    ClipViewNote { note: note(0.0, 36.0, 0.5, 100.0), selected: Some(true) },
                    ClipViewNote { note: note(0.5, 38.0, 0.25, 87.5), selected: Some(false) },
                    ClipViewNote { note: note(1.75, 43.0, 1.0, 64.0), selected: None },
                ],
                beats_per_bar: None,
                lanes: vec![],
                note_count: None,
            },
            ClipView {
                slot_ref: "7:clip_slot:1:0".into(),
                name: String::new(),
                length: 4.0,
                notes: vec![],
                beats_per_bar: None,
                lanes: vec![],
                note_count: None,
            },
        ],
        session_strips: vec![SessionStrip {
            track_ref: "7:track:0".into(),
            scene: 1.0,
            slots: vec![
                SessionSlot { index: 0.0, clip: Some(SlotClip { name: "Intro".into(), audio: false }), playing: Some(true), queued: None },
                SessionSlot { index: 1.0, clip: Some(SlotClip { name: "Vox".into(), audio: true }), playing: None, queued: Some(true) },
                SessionSlot { index: 2.0, clip: None, playing: None, queued: None },
                SessionSlot {
                    index: 3.0,
                    clip: Some(SlotClip { name: "Outro".into(), audio: false }),
                    playing: Some(false),
                    queued: Some(false),
                },
            ],
        }],
        arrangement_strips: vec![
            ArrangementStrip {
                length: 256.0,
                position: 64.5,
                playing: true,
                r#loop: Some(ArrangementLoop { start: 64.0, length: 16.0, enabled: true }),
                locators: vec![
                    Locator { name: "Drop".into(), position: 64.0, end: None },
                    Locator { name: "Break".into(), position: 128.0, end: None },
                ],
                beats_per_bar: None,
                lanes: None,
                track_count: None,
                lanes_from: None,
                meter: vec![],
                time_selection: None,
                window: None,
            },
            ArrangementStrip {
                length: 0.0,
                position: 0.0,
                playing: false,
                r#loop: Some(ArrangementLoop { start: 0.0, length: 4.0, enabled: false }),
                locators: vec![],
                beats_per_bar: None,
                lanes: None,
                track_count: None,
                lanes_from: None,
                meter: vec![],
                time_selection: None,
                window: None,
            },
            ArrangementStrip {
                length: 32.0,
                position: 0.0,
                playing: false,
                r#loop: None,
                locators: vec![],
                beats_per_bar: None,
                lanes: None,
                track_count: None,
                lanes_from: None,
                meter: vec![],
                time_selection: None,
                window: None,
            },
        ],
        connection_states: CONNECTION_STATES.to_vec(),
        disconnect_causes: DISCONNECT_CAUSES.to_vec(),
    };
    pinned("views.json", &views);
}

/// What the session controller hands the app beside its events: kept conversations, the one going on now, the answers
/// to a conversation of another Set, what Kumi remembers, and the session's status.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Controller {
    conversations: Vec<ConversationSummary>,
    now: ConversationNow,
    set_choices: Vec<SetChoice>,
    memory: MemoryView,
    status: Vec<SessionStatus>,
}

#[test]
fn what_the_session_hands_the_app_keeps_its_wire_shape() {
    let controller = Controller {
        conversations: vec![
            ConversationSummary {
                id: "mgj6k3cw0a0b0c".into(),
                saved_at: AT,
                first: "make the kick punchier".into(),
                turns: 4,
                current: true,
                place: "a2dfd5f2cf329c593c5fdc7448524087".into(),
                name: Some("Kick work".into()),
                set: Some("Night Drive".into()),
                pinned: true,
                forked_from: Some("mgj6wybk1a2b3c".into()),
                created: AT - 3_600_000,
                imported: true,
            },
            ConversationSummary {
                id: "mgj79ta82b3c4d".into(),
                saved_at: AT - 60_000,
                first: "warmer pads".into(),
                turns: 1,
                current: false,
                place: "unsaved".into(),
                name: None,
                set: None,
                pinned: false,
                forked_from: None,
                created: 0,
                imported: false,
            },
        ],
        now: ConversationNow { id: "mgj6k3cw0a0b0c".into(), title: "Kick work".into(), kept: true, touched: false },
        set_choices: vec![SetChoice::Open, SetChoice::Copy, SetChoice::Cancel],
        memory: MemoryView {
            memory: Memory {
                producer: vec![MemoryNote { id: "p1".into(), text: "Works in D minor a lot".into(), at: AT, pinned: false }],
                set: vec![remembered(MemoryScope::Set)],
            },
            set_name: Some("Night Drive".into()),
            saved: true,
        },
        status: vec![
            SessionStatus {
                state: TurnState::Running,
                connection: ConnectionState::Connected,
                turns: 3,
                max_turns: Some(40),
                observation: Some("Night Drive · 8 tracks · 124 BPM".into()),
            },
            SessionStatus {
                state: TurnState::Idle,
                connection: ConnectionState::Disconnected,
                turns: 0,
                max_turns: None,
                observation: None,
            },
        ],
    };
    pinned("controller.json", &controller);
}

/// Changes that fill every optional field of a record between them, with a picture of each kind.
fn full_changes() -> Vec<ChangeRecord> {
    let value = |name: &str, from: f64, to: f64| PictureValue {
        name: name.into(),
        from: Some(from),
        to: Some(to),
        min: Some(0.0),
        max: Some(1.0),
        from_text: Some(format!("{from}")),
        to_text: Some(format!("{to}")),
        asked: None,
    };
    let band = |shape: EqShape, freq: f64, gain_db: f64| EqBand { shape, freq, gain_db, q: 0.71, on: true, slope: None };
    let levels = |volume: f64, mute: bool| MixLevels {
        volume: Some(volume),
        volume_text: Some(format!("{volume} dB")),
        pan: Some(-0.25),
        pan_text: Some("13L".into()),
        sends: vec![
            MixSend { name: "A-Reverb".into(), level: 0.3, text: Some("-12.0 dB".into()) },
            MixSend { name: "B-Delay".into(), level: 0.0, text: None },
        ],
        mute: Some(mute),
        solo: Some(false),
    };
    vec![
        ChangeRecord {
            from: Some(120.0),
            to: Some(124.0),
            range: Some([20.0, 999.0]),
            picture: Some(ChangePicture::Values {
                values: vec![
                    PictureValue {
                        min: Some(20.0),
                        max: Some(999.0),
                        from_text: Some("120.00 BPM".into()),
                        to_text: Some("124.00 BPM".into()),
                        asked: Some("124 BPM".into()),
                        ..value("Tempo", 120.0, 124.0)
                    },
                    PictureValue { name: "Swing".into(), ..PictureValue::default() },
                ],
            }),
            ..change("c1", ChangeFamily::Tempo, "Tempo 120 → 124 BPM", ChangeState::Applied)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Bass".into(), color: Some("#f59a3c".into()) }),
            from: Some(0.85),
            to: Some(0.7),
            range: Some([0.0, 1.0]),
            picture: Some(ChangePicture::Mix { before: levels(0.0, false), after: levels(-3.0, true) }),
            ..change("c2", ChangeFamily::Mixer, "Bass volume 0.0 → -3.0 dB", ChangeState::Applied)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Vox".into(), color: None }),
            picture: Some(ChangePicture::Eq {
                before: vec![
                    EqBand { slope: Some(24.0), ..band(EqShape::LowCut, 80.0, 0.0) },
                    band(EqShape::LowShelf, 200.0, -1.5),
                    band(EqShape::Bell, 2500.0, 0.0),
                    EqBand { on: false, q: 8.0, ..band(EqShape::Notch, 6300.0, 0.0) },
                    band(EqShape::HighShelf, 10000.0, 2.0),
                    EqBand { slope: Some(12.0), ..band(EqShape::HighCut, 18000.0, 0.0) },
                ],
                after: vec![EqBand { slope: Some(48.0), ..band(EqShape::LowCut, 100.0, 0.0) }, band(EqShape::Bell, 2500.0, -2.0)],
                values: vec![value("1 Frequency A", 0.25, 0.3)],
            }),
            ..change("c3", ChangeFamily::Parameter, "EQ Eight on Vox: 2 bands", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Eq {
                before: vec![band(EqShape::Bell, 1000.0, 0.0)],
                after: vec![band(EqShape::Bell, 1000.0, 3.0)],
                values: vec![],
            }),
            ..change("c4", ChangeFamily::Parameter, "EQ Three on Keys: Mid +3 dB", ChangeState::Undone)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Dynamics {
                before: Dynamics { threshold_db: -12.0, ratio: 2.0, knee_db: 6.0, makeup_db: 0.0 },
                after: Dynamics { threshold_db: -18.0, ratio: 4.0, knee_db: 0.0, makeup_db: 3.5 },
                values: vec![value("Threshold", 0.6, 0.45)],
            }),
            ..change("c5", ChangeFamily::Parameter, "Compressor on Drums: threshold -12 → -18 dB", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Dynamics {
                before: Dynamics { threshold_db: 0.0, ratio: 1000.0, knee_db: 0.0, makeup_db: 0.0 },
                after: Dynamics { threshold_db: -1.0, ratio: 1000.0, knee_db: 0.0, makeup_db: 0.0 },
                values: vec![],
            }),
            ..change("c6", ChangeFamily::Parameter, "Limiter on Main: ceiling -1 dB", ChangeState::Kept)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Bass".into(), color: Some("#f59a3c".into()) }),
            clip: Some(ClipPicture { length: 4.0, notes: vec![note(0.0, 36.0, 0.5, 100.0), note(1.5, 38.0, 0.25, 87.5)] }),
            picture: Some(ChangePicture::Notes {
                length: 4.0,
                before: vec![],
                after: vec![note(0.0, 36.0, 0.5, 100.0), note(1.5, 38.0, 0.25, 87.5), note(3.125, 43.0, 0.333, 64.0)],
                partial: true,
            }),
            ..change("c7", ChangeFamily::Clip, "New clip “Bassline” on Bass", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Notes {
                length: 2.0,
                before: vec![note(0.0, 60.0, 1.0, 100.0)],
                after: vec![note(0.0, 60.0, 0.5, 90.0)],
                partial: false,
            }),
            ..change("c8", ChangeFamily::Clip, "Notes in “Lead”: 1 changed", ChangeState::Undone)
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
            ..change("c9", ChangeFamily::Parameter, "Automation: Filter Freq on Lead", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Automation {
                parameter: String::new(),
                min: 0.0,
                max: 1.0,
                before: vec![[0.0, 0.5]],
                after: vec![[0.0, 0.0], [4.0, 1.0]],
                words: None,
            }),
            ..change("c10", ChangeFamily::Mixer, "Automation: Volume on Pads", ChangeState::Unsure)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Drums".into(), color: Some("#ffa500".into()) }),
            devices: Some(DevicePlacement {
                devices: Some(vec!["Simpler".into(), "Saturator".into(), "EQ Eight".into()]),
                index: Some(1.0),
                rack: Some("Drum Rack".into()),
                chains: Some(vec![
                    ChainPlacement { name: "Kick".into(), devices: vec!["Simpler".into(), "Saturator".into(), "EQ Eight".into()] },
                    ChainPlacement { name: "Snare".into(), devices: vec![] },
                ]),
                chain: Some(0.0),
            }),
            ..change("c11", ChangeFamily::Device, "Saturator on Drums › Drum Rack › Kick", ChangeState::Applied)
        },
        ChangeRecord {
            devices: Some(DevicePlacement {
                devices: Some(vec!["Operator".into(), "Reverb".into()]),
                index: Some(1.0),
                ..DevicePlacement::default()
            }),
            ..change("c12", ChangeFamily::Device, "Reverb on Keys", ChangeState::Expired)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Pads".into(), color: Some("#00bfaf".into()) }),
            colors: Some(ColorChange { from: Some("#ff5a00".into()), to: "#00bfaf".into() }),
            ..change("c13", ChangeFamily::Color, "Pads colour", ChangeState::Applied)
        },
        ChangeRecord {
            colors: Some(ColorChange { from: None, to: "#1e90ff".into() }),
            ..change("c14", ChangeFamily::Color, "Clip “Intro” colour", ChangeState::Applied)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Bass".into(), color: None }),
            score: Some(87.5),
            ..change("c15", ChangeFamily::Device, "Auditioned 3 candidates against the reference", ChangeState::Heard)
        },
        ChangeRecord {
            note: Some("Live didn't confirm it; check Live.".into()),
            ..change("c16", ChangeFamily::Locators, "Locator “Drop” at bar 17", ChangeState::Unsure)
        },
        ChangeRecord {
            track: Some(TrackChip { name: "Lead 2".into(), color: None }),
            note: Some("Live can't undo a rename made while it played.".into()),
            ..change("c17", ChangeFamily::Rename, "Renamed “Lead” to “Lead 2”", ChangeState::Kept)
        },
        change("c18", ChangeFamily::Structure, "Added track “Pads”", ChangeState::Applied),
    ]
}

/// (m4-tui) REAPER's and Renoise's pictures: a tracker's lines, a route, a keyzone, items on the timeline. Pinned after the
/// others, so their ids don't move.
fn daw_pictures() -> Vec<ChangeRecord> {
    vec![
        ChangeRecord {
            picture: Some(ChangePicture::Lines {
                lines: vec![PictureLine { line: 9, before: "E-4 01 40 .. ..".into(), after: "OFF .. .. .. ..".into() }],
                more: 3,
            }),
            ..change("c901", ChangeFamily::Clip, "Wrote 4 lines in “Drop” on “Bass”", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Routes {
                routes: vec![PictureRoute {
                    from: "Drums".into(),
                    to: "Reverb".into(),
                    change: RouteChange::Made,
                    values: vec![PictureValue {
                        name: "Volume".into(),
                        to: Some(-6.0),
                        to_text: Some("−6 dB".into()),
                        ..Default::default()
                    }],
                }],
                more: 0,
            }),
            ..change("c902", ChangeFamily::Mixer, "Added send “Drums” → “Reverb”", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Zones {
                before: vec![PictureZone { name: String::new(), notes: [48, 59], velocities: None, base: Some(48) }],
                after: vec![PictureZone { name: "Snare".into(), notes: [60, 71], velocities: Some([64, 127]), base: None }],
                values: vec![],
            }),
            ..change("c903", ChangeFamily::Parameter, "“Snare” keyzone: notes 60-71", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Items {
                before: vec![PictureItem { name: "vox.wav".into(), start: 8.0, length: 4.0, fade_in: 0.0, fade_out: 0.0, muted: false }],
                after: vec![PictureItem { name: "vox.wav".into(), start: 10.0, length: 4.0, fade_in: 0.05, fade_out: 0.5, muted: true }],
                values: vec![],
            }),
            ..change("c904", ChangeFamily::Clip, "Moved “vox.wav” to bar 6", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Tracks {
                tracks: vec![
                    PictureTrack { name: "Drums".into(), color: Some("#ff5a00".into()), mark: None },
                    PictureTrack { name: "Pads".into(), color: None, mark: Some(TrackMark::Made) },
                    PictureTrack { name: "Old Bass".into(), color: None, mark: Some(TrackMark::Removed) },
                ],
                more: 5,
            }),
            ..change("c905", ChangeFamily::Structure, "Added MIDI track “Pads”", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Marks {
                before: vec![PictureMark { name: "Intro".into(), at: 0.0 }],
                after: vec![PictureMark { name: "Intro".into(), at: 0.0 }, PictureMark { name: "Drop".into(), at: 64.0 }],
                values: vec![PictureValue { name: "Drop".into(), to_text: Some("bar 17".into()), ..Default::default() }],
            }),
            ..change("c906", ChangeFamily::Locators, "Locators “Drop” and “Outro”", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Warp {
                before: vec![[0.0, 0.0], [4.0, 2.0]],
                after: vec![[0.0, 0.0], [4.5, 2.0]],
                values: vec![PictureValue {
                    name: "Warp marker".into(),
                    from_text: Some("bar 2".into()),
                    to_text: Some("bar 2 beat 1.5".into()),
                    ..Default::default()
                }],
            }),
            ..change("c907", ChangeFamily::Clip, "Vox audio clip: warp marker moved", ChangeState::Applied)
        },
        ChangeRecord {
            picture: Some(ChangePicture::Wave {
                seconds: 12.5,
                peaks: vec![0.0, 0.25, 1.0, 0.5],
                before: Some([0.0, 12.5]),
                after: Some([2.0, 8.0]),
                values: vec![],
            }),
            ..change("c908", ChangeFamily::Clip, "Vox audio clip: loop", ChangeState::Applied)
        },
    ]
}

fn picture_kind(picture: &ChangePicture) -> &'static str {
    match picture {
        ChangePicture::Values { .. } => "values",
        ChangePicture::Eq { .. } => "eq",
        ChangePicture::Dynamics { .. } => "dynamics",
        ChangePicture::Notes { .. } => "notes",
        ChangePicture::Automation { .. } => "automation",
        ChangePicture::Mix { .. } => "mix",
        ChangePicture::Lines { .. } => "lines",
        ChangePicture::Routes { .. } => "routes",
        ChangePicture::Zones { .. } => "zones",
        ChangePicture::Items { .. } => "items",
        ChangePicture::Tracks { .. } => "tracks",
        ChangePicture::Marks { .. } => "marks",
        ChangePicture::Warp { .. } => "warp",
        ChangePicture::Wave { .. } => "wave",
    }
}

#[test]
fn change_records_of_every_family_state_and_picture_keep_their_wire_shape() {
    let mut records = full_changes();
    let daw = daw_pictures();
    let kinds: BTreeSet<&str> = records.iter().chain(&daw).filter_map(|record| record.picture.as_ref()).map(picture_kind).collect();
    assert_eq!(
        kinds,
        BTreeSet::from([
            "automation",
            "dynamics",
            "eq",
            "items",
            "lines",
            "marks",
            "mix",
            "notes",
            "routes",
            "tracks",
            "values",
            "warp",
            "wave",
            "zones"
        ]),
        "every picture kind is pinned"
    );
    for family in FAMILIES {
        for state in STATES {
            let id = format!("c{}", records.len() + 1);
            records.push(change(&id, family, "A change", state));
        }
    }
    records.extend(daw);
    pinned::<Vec<ChangeRecord>>("change-records.json", &records);
}

#[test]
fn older_and_later_pictures_still_read() {
    // Notes kept as objects (before the compact form) read with their velocity, or 100 without one; a picture of a kind
    // this Kumi doesn't know reads as none, and the record still loads.
    let record: ChangeRecord = serde_json::from_str(
        r#"{"id":"c1","family":"clip","title":"New clip","state":"applied","at":1760000000000,
            "picture":{"kind":"notes","length":4,"before":[],"after":[{"pitch":36,"start":0,"duration":0.5,"velocity":90},[1,38,0.25]]}}"#,
    )
    .unwrap();
    assert_eq!(
        record.picture,
        Some(ChangePicture::Notes {
            length: 4.0,
            before: vec![],
            after: vec![note(0.0, 36.0, 0.5, 90.0), note(1.0, 38.0, 0.25, 100.0)],
            partial: false
        })
    );
    let later: ChangeRecord = serde_json::from_str(
        r#"{"id":"c2","family":"device","title":"Something new","state":"applied","at":1760000000000,"picture":{"kind":"hologram","depth":3}}"#,
    )
    .unwrap();
    assert_eq!(later, change("c2", ChangeFamily::Device, "Something new", ChangeState::Applied));
}

/// One of every request a front end makes: each optional field filled and left out, and each value of the enums they
/// carry. (The protocol's tests send each of them too.)
pub(crate) fn requests() -> Vec<Op> {
    let pinned = pins().remove(0);
    let mut requests: Vec<Op> = vec![
        op::Start {}.into(),
        op::Submit {
            input: "make the kick punchier".into(),
            pinned: Some(pinned.clone()),
            attachments: vec![Attachment {
                path: "/Users/me/Desktop/reference.png".into(),
                name: "reference.png".into(),
                media_type: "image/png".into(),
                bytes: 48_210,
            }],
            references: vec![],
        }
        .into(),
        op::Submit { input: "louder".into(), pinned: None, attachments: vec![], references: vec![] }.into(),
        // What the message names with `@`: one of each kind of reference.
        op::Submit {
            input: "send @Drums to @Big Hall like @Kick 808.wav, from @Warm Pads with @Sidechain pump".into(),
            pinned: None,
            attachments: vec![],
            references: vec![
                Reference::Daw { node: pinned.clone() },
                Reference::Sample { path: "/Users/me/Samples/Kick 808.wav".into(), name: "Kick 808.wav".into() },
                Reference::Palette { id: "p1".into(), name: "Warm Pads".into() },
                Reference::Technique { id: "t1".into(), name: "Sidechain pump".into() },
            ],
        }
        .into(),
        op::References { query: "dru".into(), limit: 50 }.into(),
        op::Steer { text: "and leave the snare".into() }.into(),
        op::Aside { id: 2, question: "why the low shelf?".into() }.into(),
        op::StopAside { id: 2 }.into(),
        op::Cancel {}.into(),
        op::Close {}.into(),
        op::Undo { id: Some("c3".into()) }.into(),
        op::Undo { id: None }.into(),
        op::TakeBackPlan {}.into(),
        op::TakeBack { to: AT + 4 }.into(),
        op::Refresh {}.into(),
        op::NewConversation {}.into(),
        op::Reconnect {}.into(),
        op::Reconfigure {}.into(),
        op::Instant { text: "tempo 124".into() }.into(),
        op::Pick { pick: Picked::Answer { question: "Which kick?".into(), options: vec!["808".into(), "909".into()], index: 1 } }.into(),
        op::Pick { pick: Picked::Technique { name: "Parallel drum bus".into(), keep: true } }.into(),
        op::Conversations {}.into(),
        op::AllConversations {}.into(),
        op::FindConversation { word: "kick".into() }.into(),
        op::ResumeConversation { id: "01JB9Z6P1Q".into() }.into(),
        op::RenameConversation { id: Some("01JB9Z6P1Q".into()), name: Some("Kick work".into()) }.into(),
        op::RenameConversation { id: None, name: None }.into(),
        op::PinConversation { id: "01JB9Z6P1Q".into(), pinned: true }.into(),
        op::DeleteConversation { id: "01JB9Z6P1Q".into() }.into(),
        op::ForkConversation { name: Some("Kick, darker".into()), before: None }.into(),
        op::ForkConversation { name: None, before: None }.into(),
        op::ForkConversation { name: None, before: Some(AT + 4) }.into(),
        op::ConversationNow {}.into(),
        op::ConversationLines { id: "01JB9Z6P1Q".into() }.into(),
        op::ConversationToExport {}.into(),
        op::Memory {}.into(),
        op::Remembered {}.into(),
        op::Taste {}.into(),
        op::ForgetTaste { id: "t2".into() }.into(),
        op::Forget { id: "p3".into() }.into(),
        op::ChangeNote { id: "p3".into(), change: NoteChange::Text("Likes the kick dry".into()) }.into(),
        op::ChangeNote { id: "s1".into(), change: NoteChange::Pinned(true) }.into(),
        op::Recipes {}.into(),
        op::PinForRecipe { pin: pinned }.into(),
        op::RunRecipe { name: "Drum bus".into(), with: serde_json::from_value(serde_json::json!({"track": "Drums"})).unwrap() }.into(),
        op::ForgetRecipe { name: "Drum bus".into() }.into(),
        op::Techniques {}.into(),
        op::ForgetTechnique { id: "k1".into() }.into(),
        op::AnswerTechnique { keep: false }.into(),
        op::Lessons {}.into(),
        op::ForgetLesson { id: "l4".into() }.into(),
        op::Goal { text: Some("a kick that cuts through at -14 LUFS".into()) }.into(),
        op::Goal { text: None }.into(),
        op::StopGoal {}.into(),
        op::StopLoop {}.into(),
        op::StopDaw {}.into(),
        op::DawMessage { text: "Kumi · done".into() }.into(),
        op::DawRecording {}.into(),
        op::StopJob { id: 3 }.into(),
        op::RunJobAgain { id: 3 }.into(),
        op::Background {}.into(),
        op::CarryOn { job: 3 }.into(),
        op::Palette {}.into(),
        op::PaletteGather {
            files: vec![
                Gathered { path: "/Samples/Kicks/Kick 808.wav".into(), name: "Kick 808".into(), song: false, class: Some("kick".into()) },
                Gathered { path: "/Music/Night Drive.wav".into(), name: "Night Drive".into(), song: true, class: None },
            ],
        }
        .into(),
        op::PaletteRemove { path: "/Samples/Kicks/Kick 808.wav".into(), size: 48_210, mtime: AT }.into(),
        op::PaletteCommand { words: "use Drums".into() }.into(),
        op::PaletteNames {}.into(),
        op::Music {}.into(),
        op::MusicFolders {}.into(),
        op::LearnMusic { folders: vec!["~/Music/Bounces".into()] }.into(),
        op::SetMusicFolders { folders: vec![] }.into(),
        op::HoldMusic { held: true }.into(),
        op::ForgetMusic {}.into(),
        op::Map {}.into(),
        op::Slots { id: 3, words: "listening off".into() }.into(),
        op::StopSlots { id: 3 }.into(),
        op::Providers {}.into(),
        op::LocalModels {}.into(),
        op::ChooseDefault {}.into(),
        op::Models { provider: "anthropic".into(), refresh: true }.into(),
        op::ChooseModel { model: "anthropic/claude-sonnet-5-5".into() }.into(),
        op::SetEffort { effort: Some(kumi_ai::providers::Effort::High) }.into(),
        op::SetEffort { effort: None }.into(),
        op::SetFast { on: true }.into(),
        op::SetFallback { model: Some("openai/gpt-5.5".into()) }.into(),
        op::SetFallback { model: None }.into(),
        op::SaveKey { provider: kumi_ai::providers::ProviderId::parse("anthropic").unwrap(), key: Secret("test-key".into()) }.into(),
        op::SignOut { provider: kumi_ai::providers::ProviderId::parse("openai").unwrap() }.into(),
        op::SignInChatgpt { id: 4, by_code: false }.into(),
        op::StopSignIn { id: 4 }.into(),
        op::MissingModel {}.into(),
    ];
    requests.extend([SetChoice::Open, SetChoice::Copy, SetChoice::Cancel].map(|choice| op::AnswerSet { choice }.into()));
    requests.extend(
        [
            BodyRequest::Tree { track_ref: "7:track:0".into() },
            BodyRequest::Notes { item_ref: "7:clip_slot:0:2".into() },
            BodyRequest::Grid { track_ref: "7:track:0".into(), index: 2.0 },
            BodyRequest::Timeline,
            BodyRequest::Tracker { item_ref: Some("pattern:3".into()) },
            BodyRequest::Tracker { item_ref: None },
            BodyRequest::Matrix,
            BodyRequest::Keyzones { item_ref: "instrument:1".into() },
            BodyRequest::Routing { track_ref: "track:2".into() },
            BodyRequest::Mixer { track_ref: Some("track:2".into()) },
            BodyRequest::Mixer { track_ref: None },
        ]
        .map(|request| op::FocusBody { request }.into()),
    );
    requests
}

#[test]
fn every_request_keeps_its_wire_shape() {
    let requests = requests();
    // Every request in the protocol's table has one here: a request added later has to be.
    let names: BTreeSet<&str> = requests.iter().map(Op::name).collect();
    assert_eq!(names, Op::NAMES.iter().copied().collect::<BTreeSet<_>>(), "every request is pinned");
    pinned::<Vec<Op>>("requests.json", &requests);
}

#[test]
fn capabilities_keep_their_wire_shape() {
    // Most of what a session can do, and some of it not (steering, jobs, palettes, an answer to a technique offered,
    // /slots).
    let capabilities = Capabilities {
        attachments: true,
        aside: true,
        reconnect: true,
        conversations: true,
        resume_conversation: true,
        watch: true,
        memory: true,
        library: true,
        taste: true,
        forget_taste: true,
        forget: true,
        change_note: true,
        recipes: true,
        instant: true,
        run_recipe: true,
        forget_recipe: true,
        techniques: true,
        forget_technique: true,
        goal: true,
        stop_goal: true,
        stop_loop: true,
        goal_status: true,
        lessons: true,
        forget_lesson: true,
        stop_daw: true,
        daw_message: true,
        device_tree: true,
        clip_view: true,
        session_strip: true,
        arrangement_strip: true,
        reconfigure: true,
        music: true,
        map: true,
        models: true,
        take_back: true,
        ..Capabilities::default()
    };
    pinned::<Capabilities>("capabilities.json", &capabilities);
}
