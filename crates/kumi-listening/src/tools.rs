//! The judge, tune, groove, form and sound tools as the model meets them: what each asks, how an input becomes a
//! request, what a round answers, and each tool's definition in the DAW's words.
use crate::{
    checklist::{Explicit, Quantity, Target},
    round::{Round, RoundKind},
};
use kumi_common::js::string::{head, trim};
use kumi_types::{
    contracts::{AuditionCandidate, AuditionRequest, JsonObject, MIX_CANDIDATE},
    daw::{asset_in_words, parenthetical, with_article, AssetPhrase, DawProfile},
};
use serde_json::{json, Value};
use std::sync::LazyLock;

pub const JUDGE_TOOL: &str = "judge";
pub const TUNE_TOOL: &str = "tune";
pub const SOUND_TOOL: &str = "sound";
pub const FORM_TOOL: &str = "form";
pub const GROOVE_TOOL: &str = "groove";

/// What the model asks of the judge: a goal (which starts a run), what to hear, the change since the last call, or
/// the end.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JudgeRequest {
    pub goal: Option<GoalRequest>,
    /// A track to judge instead of the mix.
    pub track: Option<String>,
    pub from_beat: Option<f64>,
    pub beats: Option<f64>,
    pub change: Option<String>,
    pub done: bool,
}

/// A goal as the model gives it: targets, a reference (a file or a clip), whether to clear what the detectors find,
/// and the element that must cut through.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoalRequest {
    pub loudness: Option<f64>,
    pub true_peak: Option<f64>,
    pub reference: Option<String>,
    pub problems: bool,
    pub focus: Option<String>,
    pub targets: Vec<Explicit>,
    /// A single sound against a reference sound (its envelope and tone on the checklist too).
    pub sound: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneHow {
    /// An EQ calculated from what was measured.
    Fit,
    /// One knob, homed in on.
    Home,
    /// A few knobs that interact, searched in generations heard side by side.
    Search,
    /// One knob heard across its range in one pass, what it does saved, then set where it meets the target.
    Probe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TuneRequest {
    pub device: String,
    pub how: TuneHow,
    /// The checklist item it's for (its id or label); the run's next target without one.
    pub target: Option<String>,
    pub knobs: Vec<String>,
    pub change: Option<String>,
}

/// What the model asks of the sound tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundRequest {
    pub tracks: Vec<String>,
    pub against: Option<String>,
    pub key: Option<String>,
    pub from_beat: Option<f64>,
    pub beats: Option<f64>,
}

/// What the model asks of the form tool: a stretch (the whole song without one) and a reference to compare with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormRequest {
    pub from_beat: Option<f64>,
    pub beats: Option<f64>,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GrooveRequest {
    /// The part: a MIDI clip.
    pub clip: Option<String>,
    /// The reference: a MIDI clip (a reference's drums through Drums to MIDI, say), or an audio file of a pitched part
    /// that isn't in Live.
    pub reference: Option<String>,
    /// An audio file reference's tempo (BPM); estimated when left out.
    pub reference_tempo: Option<f64>,
    /// The reference's drum stem (an audio clip), to move its hits onto their onsets.
    pub audio: Option<String>,
    pub change: Option<String>,
    /// Code moves the notes toward the reference, by `amount`.
    pub apply: bool,
    pub amount: Option<f64>,
    pub done: bool,
}

/// A tool's description and input schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub description: String,
    pub schema: JsonObject,
}

static JUDGE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/judge.json")).unwrap());
static TUNE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/tune.json")).unwrap());
static SOUND: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/sound.json")).unwrap());
static FORM: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/form.json")).unwrap());
static GROOVE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/groove.json")).unwrap());

/// What each asset says in Live's words, and how many times; a DAW says them its own way.
pub(crate) const JUDGE_PHRASES: &[AssetPhrase] = &[
    AssetPhrase { live: "Live's devices", count: 1, said: |p| format!("{}'s devices", p.words.app) },
    AssetPhrase { live: "the last Live Limiter on the track, else a Utility it adds", count: 1, said: |p| p.vocab.gain_stage.into() },
    AssetPhrase { live: "a static EQ (EQ Eight)", count: 1, said: |p| format!("a static EQ{}", parenthetical(p.vocab.eq_name)) },
    AssetPhrase {
        live: "a dynamic band (Multiband Dynamics)",
        count: 1,
        said: |p| format!("a dynamic band{}", parenthetical(p.vocab.multiband)),
    },
    AssetPhrase {
        live: "mono below a frequency (Utility's Bass Mono)",
        count: 1,
        said: |p| format!("mono below a frequency{}", parenthetical(p.vocab.bass_mono)),
    },
    AssetPhrase { live: "starts in the Arrangement", count: 1, said: |p| format!("starts in the {}", p.words.timeline) },
    AssetPhrase { live: "an audio file or clip whose", count: 1, said: |p| format!("an audio file or {} whose", p.vocab.clip) },
    AssetPhrase { live: "an audio clip's clipRef", count: 1, said: |p| format!("{}'s {}", p.vocab.audio_clip, p.vocab.clip_ref) },
];
pub(crate) const TUNE_PHRASES: &[AssetPhrase] = &[
    AssetPhrase { live: "calculates an EQ Eight's bands", count: 1, said: |p| format!("calculates {}'s bands", with_article(p.vocab.eq)) },
    AssetPhrase { live: "on EQ Eight, the nearest band", count: 1, said: |p| format!("on {}, the nearest band", p.vocab.eq) },
    AssetPhrase {
        live: ", and turns Adaptive Q off and Scale to 100% so they land as calculated",
        count: 1,
        said: |p| p.vocab.eq_fit_settings.into(),
    },
];
pub(crate) const SOUND_PHRASES: &[AssetPhrase] =
    &[AssetPhrase { live: "Measure sounds in Live:", count: 1, said: |p| format!("Measure sounds in {}:", p.words.app) }];
pub(crate) const FORM_PHRASES: &[AssetPhrase] = &[
    AssetPhrase {
        live: "plays the whole Arrangement (or from_beat for beats), listening as it plays",
        count: 1,
        said: |p| {
            if p.caps.renders_to_listen {
                format!("renders the whole {} offline (or from_beat for beats)", p.words.timeline)
            } else {
                format!("plays the whole {} (or from_beat for beats), listening as it plays", p.words.timeline)
            }
        },
    },
    AssetPhrase { live: "the Arrangement's clips", count: 1, said: |p| format!("the {}'s {}", p.words.timeline, p.vocab.clips) },
    AssetPhrase { live: "a clip in the Set", count: 2, said: |p| format!("{} in the {}", with_article(p.vocab.clip), p.words.place) },
    AssetPhrase { live: "(its clipRef)", count: 1, said: |p| format!("(its {})", p.vocab.clip_ref) },
    AssetPhrase { live: "the song's last clip", count: 1, said: |p| format!("the song's last {}", p.vocab.clip) },
    AssetPhrase {
        live: "A whole-song listen takes as long as the song.",
        count: 1,
        said: |p| {
            if p.caps.renders_to_listen {
                "A whole song renders faster than it plays."
            } else {
                "A whole-song listen takes as long as the song."
            }
            .into()
        },
    },
    AssetPhrase { live: "this Set's tempo", count: 1, said: |p| format!("this {}'s tempo", p.words.place) },
    AssetPhrase { live: "this Set's bars", count: 1, said: |p| format!("this {}'s bars", p.words.place) },
    AssetPhrase { live: "the Arrangement's start", count: 1, said: |p| format!("the {}'s start", p.words.timeline) },
];
pub(crate) const GROOVE_PHRASES: &[AssetPhrase] = &[
    AssetPhrase {
        live: "a part on a Drum Rack in lanes by its pads",
        count: 1,
        said: |p| format!("a part on {} in lanes by its pads", p.vocab.drum_kit),
    },
    AssetPhrase { live: "put the track in the Arrangement", count: 1, said: |p| format!("put the track in the {}", p.words.timeline) },
    AssetPhrase {
        live: ", separate its stems (live_command separate_stems), convert the drum stem to MIDI (live_command convert_drums_to_midi)",
        count: 1,
        said: |p| format!(", {}", p.vocab.drum_stem_steps),
    },
    AssetPhrase { live: "isn't in Live", count: 2, said: |p| format!("isn't in {}", p.words.app) },
    AssetPhrase { live: "an Arrangement audio clip's clipRef", count: 1, said: |p| p.vocab.stem_audio.into() },
    AssetPhrase {
        live: "with clip (the part, a MIDI clip) and reference (a MIDI clip with",
        count: 1,
        said: |p| format!("with {} (the part, {m}) and reference ({m} with", p.vocab.clip_field, m = p.vocab.midi_clip),
    },
    AssetPhrase {
        live: "give that MIDI clip as reference and the drum stem's clip as audio",
        count: 1,
        said: |p| format!("give that {} as reference and {} as audio", without_article(p.vocab.midi_clip), p.vocab.stem_as_audio),
    },
    AssetPhrase { live: "a MIDI clip's clipRef", count: 1, said: |p| p.vocab.part_ref.into() },
    AssetPhrase { live: "the reference's MIDI clip (its clipRef)", count: 1, said: |p| p.vocab.reference_part.into() },
    AssetPhrase { live: "(change_notes, transform_midi swing, quantize)", count: 1, said: |p| format!("({})", p.vocab.note_tools) },
];

/// A name without its article: "MIDI clip" from "a MIDI clip".
fn without_article(name: &str) -> &str {
    name.strip_prefix("a ").or_else(|| name.strip_prefix("an ")).unwrap_or(name)
}

fn definition(data: &Value, profile: &DawProfile, phrases: &[AssetPhrase]) -> ToolDef {
    let data = asset_in_words(data.clone(), profile, phrases);
    ToolDef { description: data["description"].as_str().unwrap().into(), schema: data["schema"].as_object().unwrap().clone() }
}

/// The judge tool in the DAW's words.
pub fn judge_tool(profile: &DawProfile) -> ToolDef {
    definition(&JUDGE, profile, JUDGE_PHRASES)
}

/// The tune tool in the DAW's words.
pub fn tune_tool(profile: &DawProfile) -> ToolDef {
    definition(&TUNE, profile, TUNE_PHRASES)
}

/// The sound tool in the DAW's words.
pub fn sound_tool(profile: &DawProfile) -> ToolDef {
    definition(&SOUND, profile, SOUND_PHRASES)
}

/// The form tool in the DAW's words.
pub fn form_tool(profile: &DawProfile) -> ToolDef {
    definition(&FORM, profile, FORM_PHRASES)
}

/// The groove tool in the DAW's words.
pub fn groove_tool(profile: &DawProfile) -> ToolDef {
    let mut def = definition(&GROOVE, profile, GROOVE_PHRASES);
    // The part is taken by the DAW's word for it, where the schema has it.
    let field = profile.vocab.clip_field;
    if field != kumi_types::live_word!(clip_field) {
        if let Some(properties) = def.schema.get_mut("properties").and_then(Value::as_object_mut) {
            *properties = std::mem::take(properties)
                .into_iter()
                .map(|(key, value)| if key == "clip" { (field.to_owned(), value) } else { (key, value) })
                .collect();
        }
    }
    def
}

/// The sound tool's input as a request, or what's wrong with it.
pub fn sound_request(input: &JsonObject) -> Result<SoundRequest, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let tracks: Vec<String> = input
        .get("tracks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if tracks.is_empty() {
        return Err("Name the tracks to measure (tracks).".into());
    }
    Ok(SoundRequest {
        tracks,
        against: text("against"),
        key: text("key"),
        from_beat: input.get("from_beat").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.),
        beats: input.get("beats").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.),
    })
}

/// The form tool's input as a request.
pub fn form_request(input: &JsonObject) -> FormRequest {
    FormRequest {
        from_beat: input.get("from_beat").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.),
        beats: input.get("beats").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.),
        reference: input.get("reference").and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned),
    }
}

/// The groove tool's input as a request, or what's wrong with it.
pub fn groove_request(profile: &DawProfile, input: &JsonObject) -> Result<GrooveRequest, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let field = profile.vocab.clip_field;
    let request = GrooveRequest {
        clip: text(field),
        reference: text("reference"),
        reference_tempo: input
            .get("reference_tempo")
            .and_then(Value::as_f64)
            .filter(|tempo| tempo.is_finite() && (20. ..=400.).contains(tempo)),
        audio: text("audio"),
        change: text("change"),
        apply: input.get("apply") == Some(&Value::Bool(true)),
        amount: input.get("amount").and_then(Value::as_f64).filter(|v| v.is_finite()),
        done: input.get("done") == Some(&Value::Bool(true)),
    };
    let starting = request.clip.is_some() || request.reference.is_some();
    if starting && (request.clip.is_none() || request.reference.is_none()) {
        return Err(format!(
            "A run starts with both {field} (the part) and reference (the reference's {}, or an audio file of a pitched part).",
            without_article(profile.vocab.midi_clip)
        ));
    }
    if starting && (request.change.is_some() || request.apply || request.done) {
        return Err("Start a run first; judge a change (or apply, or end it) in a later call.".into());
    }
    if !starting && request.change.is_none() && !request.apply && !request.done {
        return Err(format!("Give {field} and reference to start, the change you made, apply: true, or done: true."));
    }
    Ok(request)
}

/// The tune tool's input as a request, or what's wrong with it.
pub fn tune_request(input: &JsonObject) -> Result<TuneRequest, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let how =
        match input.get("how").and_then(Value::as_str) {
            Some("fit") => TuneHow::Fit,
            Some("home") => TuneHow::Home,
            Some("search") => TuneHow::Search,
            Some("probe") => TuneHow::Probe,
            _ => return Err(
                "how is fit (an EQ calculated from what was measured), home (one knob homed in on), search (2 to 5 knobs that interact) or probe (one knob heard across its range)."
                    .into(),
            ),
        };
    let device = text("device").ok_or("Name the device to tune (its ref).")?;
    let knobs: Vec<String> = input
        .get("knobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if matches!(how, TuneHow::Home | TuneHow::Probe) && knobs.len() != 1 {
        return Err("home and probe move one knob: give its name in knobs.".into());
    }
    if how == TuneHow::Search && !(2..=5).contains(&knobs.len()) {
        return Err("search tunes 2 to 5 knobs that interact: give their names in knobs.".into());
    }
    Ok(TuneRequest { device, how, target: text("target"), knobs, change: text("change") })
}

/// The tool's input as a request, or what's wrong with it.
pub fn judge_request(input: &JsonObject) -> Result<JudgeRequest, String> {
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let input = Value::Object(input.clone());
    let goal = match input.get("goal") {
        Some(goal) if goal.is_object() => {
            let mut targets = vec![];
            for wanted in goal.get("targets").and_then(Value::as_array).into_iter().flatten() {
                let measure = match wanted.get("measure").and_then(Value::as_str).unwrap_or("") {
                    "dynamics" => Quantity::Plr,
                    "density" => Quantity::Psr,
                    "range" => Quantity::Range,
                    "punch" => Quantity::Crest,
                    "pumping" => Quantity::Pumping,
                    "distortion" => Quantity::Distortion,
                    "low_width" => Quantity::LowWidth,
                    "rumble" => Quantity::Rumble,
                    "brightness" => Quantity::Tilt,
                    "decay_time" => Quantity::DecayTime,
                    "tail_darkening" => Quantity::Darkening,
                    "echo_time" => Quantity::EchoTime,
                    "echo_fall" => Quantity::EchoFalls,
                    "swing" => Quantity::Swing,
                    "sweep" => Quantity::Sweep,
                    "tail_share" => Quantity::TailShare,
                    other => return Err(format!("{other} isn't a measure Kumi can target.")),
                };
                // Within a noticeable step unless asked.
                let step = match measure {
                    Quantity::DecayTime => 0.1,
                    Quantity::EchoTime => 10.,
                    Quantity::EchoFalls => 1.5,
                    Quantity::Swing => 1.,
                    Quantity::Darkening | Quantity::Sweep => 0.25,
                    Quantity::TailShare => 5.,
                    _ => 0.5,
                };
                // A tolerance of nothing, or a range of one point, can't be met: a point is that value within a step.
                let target = match (number(wanted, "value"), number(wanted, "at_least"), number(wanted, "at_most")) {
                    (Some(_), _, _) if number(wanted, "within").is_some_and(|within| within <= 0.) => {
                        return Err("within is how far from value still counts: give it above 0 (a noticeable step, say).".into())
                    }
                    (Some(value), _, _) => Target::Exactly { value, within: number(wanted, "within").unwrap_or(step) },
                    (None, Some(low), Some(high)) if low == high => Target::Exactly { value: low, within: step },
                    (None, Some(low), Some(high)) if low < high => Target::Between { low, high },
                    (None, Some(value), None) => Target::AtLeast { value },
                    (None, None, Some(value)) => Target::AtMost { value },
                    _ => return Err("Give each target a value (and within), at_least or at_most.".into()),
                };
                targets.push(Explicit { measure, target });
            }
            Some(GoalRequest {
                loudness: number(goal, "loudness"),
                true_peak: number(goal, "true_peak"),
                reference: text(goal, "reference"),
                problems: goal.get("problems").and_then(Value::as_bool).unwrap_or(true),
                focus: text(goal, "focus"),
                targets,
                sound: goal.get("sound").and_then(Value::as_bool).unwrap_or(false),
            })
        }
        Some(_) => return Err("goal is an object: what to reach.".into()),
        None => None,
    };
    let request = JudgeRequest {
        goal,
        track: text(&input, "track"),
        from_beat: number(&input, "from_beat"),
        beats: number(&input, "beats").filter(|beats| *beats > 0.),
        change: text(&input, "change"),
        done: input.get("done") == Some(&Value::Bool(true)),
    };
    if request.goal.is_some() && (request.change.is_some() || request.done) {
        return Err("A goal starts a run; judge a change (or end the run) in a later call.".into());
    }
    if request.goal.is_none() && request.change.is_none() && !request.done {
        return Err("Give a goal to start, the change you made, or done: true to end.".into());
    }
    Ok(request)
}

/// A round as the model reads it: the log's lines, and the checklist itself when the run starts or ends.
pub fn judge_reply(round: &Round) -> Value {
    let mut reply = json!({"round":round.round,"log":round.lines(),"met":round.met,"listens":round.listens});
    if let Some(next) = &round.next {
        reply["next"] = json!(next);
    }
    if round.kind != RoundKind::Judged {
        reply["checklist"] = json!(round.rows);
    }
    if !round.problems.is_empty() {
        reply["problems"] = json!(round.problems);
    }
    if let Some(kept) = round.kept {
        reply["kept"] = json!(kept);
    }
    let unread = round.unread();
    let note: String = match (round.kind, round.met) {
        (_, true) if !unread.is_empty() => format!(
            "Every item Kumi could read is within tolerance, but it couldn't read {}: say so, then stop, or end the run with done: true for a last whole listen.",
            unread.join(", ")
        ),
        (_, true) => "Every item is within tolerance: say so and stop, or end the run with done: true for a last whole listen.".into(),
        (RoundKind::Done, false) => "The run is over. Say what changed, before → after, and what's still off.".into(),
        (RoundKind::Start, false) => "Make one change toward next, then call judge with change (a tune is judged already).".into(),
        (RoundKind::Judged, false) if round.kept == Some(false) => {
            "Kumi took that change back. Try another way to close next (another device, a different setting), then judge it (a tune is judged already).".into()
        }
        (RoundKind::Judged, false) => "Kept. Make one change toward next, then judge it (a tune is judged already).".into(),
    };
    reply["note"] = json!(note);
    reply
}

pub const AUDITION_TOOL: &str = "audition";
pub const RENDER_TOOL: &str = "render";
static AUDITION: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/audition.json")).unwrap());

/// A phrase with its first letter capitalised.
fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    letters.next().map(|first| first.to_uppercase().chain(letters).collect()).unwrap_or_default()
}

pub(crate) const AUDITION_PHRASES: &[AssetPhrase] = &[
    AssetPhrase {
        live: "Kumi plays the part and hears",
        count: 1,
        said: |p| if p.caps.renders_to_listen { "Kumi renders the part offline and hears" } else { "Kumi plays the part and hears" }.into(),
    },
    AssetPhrase {
        live: "first candidate in the Set",
        count: 1,
        said: |p| {
            if p.caps.renders_to_listen {
                "first candidate out loud once it's rendered".into()
            } else {
                format!("first candidate in the {}", p.words.place)
            }
        },
    },
    AssetPhrase {
        live: "in one real-time pass",
        count: 1,
        said: |p| if p.caps.renders_to_listen { "in one render" } else { "in one real-time pass" }.into(),
    },
    AssetPhrase {
        live: "in the Arrangement, or a Session clip per candidate",
        count: 1,
        said: |p| format!("in the {}, or {} per candidate", p.words.timeline, p.vocab.launcher_clip),
    },
    AssetPhrase {
        live: "(clip), which Kumi plays from the Arrangement for the render and removes after",
        count: 1,
        said: |p| {
            if p.caps.renders_to_listen {
                format!("({}), which Kumi renders where it is", p.vocab.clip_field)
            } else {
                format!("({}), which Kumi plays from the {} for the render and removes after", p.vocab.clip_field, p.words.timeline)
            }
        },
    },
    AssetPhrase { live: "what Main plays at from_beat", count: 1, said: |p| format!("what {} plays at from_beat", p.words.master) },
    AssetPhrase { live: "(Main's output)", count: 1, said: |p| format!("({}'s output)", p.words.master) },
    AssetPhrase {
        live: "A Session clip on that track to play (its clipRef); left out, the Arrangement at from_beat",
        count: 1,
        said: |p| {
            format!(
                "{} on that track to {} (its {}); left out, the {} at from_beat",
                capitalized(p.vocab.launcher_clip),
                if p.caps.renders_to_listen { "render" } else { "play" },
                p.vocab.clip_ref,
                p.words.timeline
            )
        },
    },
    AssetPhrase {
        live: "a clip's length when left out",
        count: 1,
        said: |p| format!("{}'s length when left out", with_article(p.vocab.clip)),
    },
    AssetPhrase { live: "an audio clip's clipRef", count: 1, said: |p| format!("{}'s {}", p.vocab.audio_clip, p.vocab.clip_ref) },
    AssetPhrase {
        live: "Where the part starts in the Arrangement",
        count: 1,
        said: |p| format!("Where the part starts in the {}", p.words.timeline),
    },
    AssetPhrase { live: "Where to start in the Arrangement", count: 1, said: |p| format!("Where to start in the {}", p.words.timeline) },
];

/// The audition tool in the DAW's words: a candidate takes the DAW's own thing to play under the DAW's word for it
/// (`clip` in Live, REAPER's `item`, Renoise's `pattern`).
pub fn audition_tool(profile: &DawProfile) -> ToolDef {
    let mut def = definition(&AUDITION, profile, AUDITION_PHRASES);
    let field = profile.vocab.clip_field;
    if field != kumi_types::live_word!(clip_field) {
        if let Some(properties) =
            def.schema.get_mut("properties").and_then(|p| p.pointer_mut("/candidates/items/properties")).and_then(Value::as_object_mut)
        {
            if let Some(clip) = properties.shift_remove("clip") {
                properties.insert(field.into(), clip);
            }
        }
    }
    def
}

/// What the render tool's description says in Live's words (it isn't one of the asset's "description" keys).
pub(crate) const RENDER_PHRASES: &[AssetPhrase] = &[
    AssetPhrase { live: "an audio track's own clips", count: 1, said: |p| format!("an audio track's own {}", p.vocab.clips) },
    AssetPhrase {
        live: "audition, or record by resampling",
        count: 1,
        said: |p| if p.caps.renders_to_listen { "audition, or listen with track" } else { "audition, or record by resampling" }.into(),
    },
];

/// The render tool in the DAW's words.
pub fn render_tool(profile: &DawProfile) -> ToolDef {
    let data = asset_in_words(AUDITION.clone(), profile, AUDITION_PHRASES);
    let described = asset_in_words(json!({"description": data["renderDescription"]}), profile, RENDER_PHRASES);
    ToolDef { description: described["description"].as_str().unwrap().into(), schema: data["renderSchema"].as_object().unwrap().clone() }
}

/// The audition tool's input as a request, or what's wrong with it.
pub fn audition_request(profile: &DawProfile, input: &JsonObject) -> Result<AuditionRequest, String> {
    let mut candidates = Vec::new();
    for item in input.get("candidates").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        let label = item.get("label").and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(|s| head(s, 60));
        let field = profile.vocab.clip_field;
        if item.get("mix") == Some(&Value::Bool(true)) {
            if item.get("track").is_some() || item.get(field).is_some() {
                return Err(format!("A mix candidate is the whole mix: give it no track or {field}."));
            }
            candidates.push(AuditionCandidate { track: MIX_CANDIDATE.into(), mix: Some(true), label, clip: None });
            continue;
        }
        let track = item
            .get("track")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("Each candidate needs its track (a trackRef from discovery), or is the whole mix ({\"mix\": true}).")?;
        let clip = item.get(field).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        candidates.push(AuditionCandidate { track: track.into(), mix: None, label, clip });
    }
    if candidates.is_empty() || candidates.len() > 8 {
        return Err("Give 1 to 8 candidates.".into());
    }
    let mix = candidates.iter().any(|c| c.mix == Some(true));
    if mix && candidates.len() > 1 {
        return Err("The whole mix renders on its own: every other candidate would play into it. Audition the mix alone (and tracks in another call).".into());
    }
    let mut tracks = std::collections::HashSet::new();
    if !candidates.iter().all(|c| tracks.insert(c.track.clone())) {
        return Err("Each candidate is a track of its own; put ideas on separate tracks to hear them side by side.".into());
    }
    let number = |key: &str| input.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
    let from_beat = number("from_beat");
    let beats = number("beats");
    if from_beat.is_none() && candidates.iter().any(|c| c.clip.is_none()) {
        return Err(format!(
            "Say where the part is: from_beat (and beats) in the {}, or {} for each candidate.",
            profile.words.timeline, profile.vocab.launcher_clip
        ));
    }
    if beats.is_some_and(|v| !(v > 0.0)) {
        return Err("beats is more than 0.".into());
    }
    let focus = input.get("focus").and_then(Value::as_str).filter(|s| ["sound", "section"].contains(s)).or(if mix {
        Some("section")
    } else {
        None
    });
    Ok(AuditionRequest {
        candidates,
        from_beat,
        beats,
        reference: input.get("reference").and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned),
        reference_from: number("reference_from_seconds"),
        reference_seconds: number("reference_seconds"),
        focus: focus.map(|s| serde_json::from_value(Value::String(s.into())).unwrap()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::daw::check_asset_phrases;

    const ASSETS: [(&LazyLock<Value>, &[AssetPhrase]); 6] = [
        (&JUDGE, JUDGE_PHRASES),
        (&TUNE, TUNE_PHRASES),
        (&SOUND, SOUND_PHRASES),
        (&FORM, FORM_PHRASES),
        (&GROOVE, GROOVE_PHRASES),
        (&AUDITION, AUDITION_PHRASES),
    ];

    #[test]
    fn each_assets_phrases_are_in_it_as_many_times_as_they_say_and_live_says_them_as_they_are() {
        for (data, phrases) in ASSETS {
            assert_eq!(check_asset_phrases(data, phrases), Ok(()));
            assert_eq!(asset_in_words((**data).clone(), &DawProfile::ABLETON, phrases), **data);
        }
        // The render tool's description is the asset's own key, read with phrases of its own.
        let render = json!({"description": AUDITION["renderDescription"]});
        assert_eq!(check_asset_phrases(&render, RENDER_PHRASES), Ok(()));
        assert_eq!(render_tool(&DawProfile::ABLETON).description, AUDITION["renderDescription"].as_str().unwrap());
        assert_eq!(audition_tool(&DawProfile::ABLETON).schema, AUDITION["schema"].as_object().unwrap().clone());
    }

    /// Every word a DAW of Kumi's own must not see in a tool it's offered: Live's, and a clip (REAPER has items and takes,
    /// Renoise samples and patterns).
    #[test]
    fn reaper_and_renoise_tools_say_their_own_things_never_a_clip_or_lives_words() {
        let live = regex::Regex::new(
            r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Main|Drum Rack|Operator|EQ Eight|Utility|Limiter|Multiband Dynamics|Adaptive Q|live_command|[Cc]lips?)\b",
        )
        .unwrap();
        for profile in [&DawProfile::REAPER, &DawProfile::RENOISE] {
            for def in [
                audition_tool(profile),
                render_tool(profile),
                judge_tool(profile),
                tune_tool(profile),
                sound_tool(profile),
                form_tool(profile),
                groove_tool(profile),
            ] {
                let text = format!("{} {}", def.description, serde_json::to_string(&def.schema).unwrap());
                let found: Vec<&str> = live.find_iter(&text).map(|m| m.as_str()).collect();
                assert!(found.is_empty(), "{:?}: {found:?} in {text}", profile.id);
                for tool in ["change_notes", "transform_midi", "quantize"] {
                    assert!(!text.contains(tool), "{:?}: Live's {tool} in {text}", profile.id);
                }
            }
            // groove takes its part by the DAW's word, and says how to name it.
            let groove = groove_tool(profile);
            let properties = &groove.schema["properties"];
            assert!(properties.get(profile.vocab.clip_field).is_some() && properties.get("clip").is_none(), "{properties}");
            assert_eq!(properties[profile.vocab.clip_field]["description"], format!("Starts a run: the part, {}", profile.vocab.part_ref));
            let input = json!({profile.vocab.clip_field: "a", "reference": "b"}).as_object().cloned().unwrap();
            assert_eq!(groove_request(profile, &input).unwrap().clip.as_deref(), Some("a"));
            let half = json!({"reference": "b"}).as_object().cloned().unwrap();
            assert!(groove_request(profile, &half)
                .unwrap_err()
                .starts_with(&format!("A run starts with both {} (the part)", profile.vocab.clip_field)));
            let audition = audition_tool(profile);
            let candidate = &audition.schema["properties"]["candidates"]["items"]["properties"];
            assert!(candidate.get(profile.vocab.clip_field).is_some() && candidate.get("clip").is_none(), "{candidate}");
            assert!(audition.description.contains("renders the part offline"), "{}", audition.description);
        }
        // A candidate's part is taken by the DAW's word for it.
        let input = json!({"candidates": [{"track": "track:1", "item": "item:2"}]}).as_object().cloned().unwrap();
        assert_eq!(audition_request(&DawProfile::REAPER, &input).unwrap().candidates[0].clip.as_deref(), Some("item:2"));
        let mix = json!({"candidates": [{"mix": true, "pattern": "pattern:1"}], "from_beat": 0}).as_object().cloned().unwrap();
        assert_eq!(
            audition_request(&DawProfile::RENOISE, &mix).unwrap_err(),
            "A mix candidate is the whole mix: give it no track or pattern."
        );
    }

    #[test]
    fn a_dawss_tools_never_say_lives_words() {
        let live = [
            "Live",
            "Ableton",
            "Arrangement",
            "Set",
            "Main",
            "EQ Eight",
            "Utility",
            "Limiter",
            "Multiband Dynamics",
            "Drum Rack",
            "live_command",
        ];
        for profile in [&DawProfile::UNKNOWN] {
            for def in [
                audition_tool(profile),
                render_tool(profile),
                judge_tool(profile),
                tune_tool(profile),
                sound_tool(profile),
                form_tool(profile),
                groove_tool(profile),
            ] {
                let text = format!("{} {}", def.description, serde_json::to_string(&def.schema).unwrap());
                for word in live {
                    let found = text.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|piece| piece == word);
                    assert!(!found, "{word} in {text}");
                }
            }
        }
    }
}
