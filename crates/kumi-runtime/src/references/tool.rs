//! The reference tool: a reference in any form in, its measured profile out (or the one question that settles what
//! it is), kept so it's measured once.

use super::{
    fetch::Fetcher,
    sources::{stamp, Choice, Kind, Resolved, Sources, Wanted},
    store::{KeptReference, KeptTrack, ReferenceStore},
};
use crate::{
    audio::tools::ResolveAudio,
    core::{
        contracts::{JsonObject, KernelTool, ToolResult},
        errors::RuntimeError,
    },
    listening::{
        checklist::{Profile, Spread, REGIONS},
        embed,
        measure::{measure_file, MeasureOptions},
    },
    video::programs::ProgramOptions,
};
use async_trait::async_trait;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{json::stringify, string::head},
    time::now_ms,
};
use serde_json::{json, Value};
use std::rc::Rc;

pub const REFERENCE_TOOL: &str = "reference";
const DESCRIPTION: &str = "Turn a reference into measured targets, once: an audio file or a folder of them, a YouTube video or playlist, a Spotify link (track, album, playlist or artist), or words: an artist (\"bladee\"), an album (\"Kid A\") or a genre or style (\"dub techno\"). Kumi finds example tracks (for words, the artist's most listened recordings, an album's tracks or a genre's main artists, through MusicBrainz and ListenBrainz; the audio from a YouTube search, matched by length), measures each, and keeps a profile with each measure's typical value and the range the tracks keep to: loudness, dynamics, punch, tonal balance by region, stereo below 120 Hz and brightness. Silent tracks, and (in a folder or a search) ones under 30 seconds, are passed over. It's kept, so asking again costs nothing (a file or folder is measured again once its files change). When the words could mean more than one thing it answers with a question and options: ask the producer that one question, then call again with the `what` of the option they pick. Work toward the profile with judge (goal.reference: what the reply's note says). For one element of a reference (its bass, its drums), put the track in the Set, separate its stems (live_command separate_stems) and give a stem's clipRef. Fetching and measuring takes a minute or two.";
/// How long a track is heard for its profile: long enough for its loud and quiet parts.
const MEASURED: f64 = 360.;
/// A track measured for a style is a finished track: at least this long (but one file the producer gives is taken as
/// it is) ...
const SHORTEST: f64 = 30.;
/// ... and at least this loud (LUFS), not silent or muted.
const QUIETEST: f64 = -45.;

pub struct ReferenceTool {
    pub store: Rc<ReferenceStore>,
    pub sources: Rc<Sources>,
    pub fetcher: Rc<Fetcher>,
    /// A clip in the Set (a separated stem, say) as its audio file.
    pub resolve: Option<ResolveAudio>,
}

pub fn reference_tools(store: Rc<ReferenceStore>, programs: ProgramOptions, resolve: Option<ResolveAudio>) -> Vec<Rc<dyn KernelTool>> {
    let fetcher = Rc::new(Fetcher::new(store.audio_folder(), programs));
    vec![Rc::new(ReferenceTool { store, sources: Rc::new(Sources::default()), fetcher, resolve })]
}

#[async_trait(?Send)]
impl KernelTool for ReferenceTool {
    fn name(&self) -> &str {
        REFERENCE_TOOL
    }
    fn description(&self) -> &str {
        DESCRIPTION
    }
    fn input_schema(&self) -> JsonObject {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["what"],
            "properties": {
                "what": {"type": "string", "minLength": 1, "maxLength": 1024, "description": "The reference: a file or folder path, a clip in the Set (its clipRef), a YouTube or Spotify link, an artist, album or genre in plain words, or a question's answer (its option's what: artist:<id> or album:<id>)"},
                "tracks": {"type": "integer", "minimum": 3, "maximum": 10, "description": "How many example tracks to measure (6 when left out)"},
                "again": {"type": "boolean", "description": "Measure it again even though it's kept"}
            }
        })
        .as_object()
        .unwrap()
        .clone()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let mut what = input.get("what").and_then(Value::as_str).unwrap_or("").trim().to_string();
        // A clip in the Set: its file.
        if let Some(resolve) = self.resolve.as_ref().filter(|_| what.contains(":clip") && !what.contains(' ')) {
            if let Ok(Some(file)) = resolve(what.clone(), signal.clone()).await {
                what = file;
            }
        }
        let count = input.get("tracks").and_then(Value::as_u64).map_or(6, |count| count.clamp(3, 10) as usize);
        let again = input.get("again").and_then(Value::as_bool) == Some(true);
        if !again {
            if let Some(kept) = self.store.load(&what).await {
                // Kept before the style model could hear it: measured again once the model can be had, else said.
                let fetched = std::cell::RefCell::new(vec![]);
                let style = match kept.profile.vibe.is_none() && embeddings_on() {
                    true => {
                        let say = |said: &str| fetched.borrow_mut().push(said.to_string());
                        embed::style_model(crate::slots::kept().model_file(crate::slots::Job::Embeddings), &say, &signal).await.err()
                    }
                    false => None,
                };
                if kept.profile.vibe.is_some() || !embeddings_on() || style.is_some() {
                    signal.check()?;
                    let mut said = reply(&kept, true, &what);
                    if let Some(why) = style {
                        said["style"] = json!(unstyled(&why));
                    }
                    if !fetched.borrow().is_empty() {
                        said["fetched"] = json!(fetched.take());
                    }
                    return Ok(ToolResult::text(stringify(&said)));
                }
            }
        }
        match self.measure(&what, count, signal.clone()).await {
            Ok(Measured::Kept(kept, missed, fetched, style)) => {
                let mut said = reply(&kept, false, &what);
                if !missed.is_empty() {
                    said["passedOver"] = json!(missed);
                }
                if let Some(why) = style.filter(|_| kept.profile.vibe.is_none()) {
                    said["style"] = json!(unstyled(&why));
                }
                if !fetched.is_empty() {
                    said["fetched"] = json!(fetched);
                }
                Ok(ToolResult::text(stringify(&said)))
            }
            Ok(Measured::Ask { question, options }) => Ok(ToolResult::text(stringify(&json!({
                "question": question,
                "options": options,
                "note": "Ask the producer this one question (with these options' labels when there are some), then call reference again with what: the picked option's what (or, when they name something else, their words)."
            })))),
            Err(why) => {
                signal.check()?;
                Ok(ToolResult::error(why))
            }
        }
    }
}

/// Whether the embeddings slot lets the style model hear references.
fn embeddings_on() -> bool {
    crate::slots::kept().now(crate::slots::Job::Embeddings) != crate::slots::Choice::Off
}

/// What a reference the style model couldn't hear says.
fn unstyled(why: &str) -> String {
    format!("the style model couldn't hear it ({why}), so a judge run won't guard its style; asked for again, it's heard then")
}

enum Measured {
    /// Kept, the tracks passed over (with why), what Kumi fetched for it (a model, the first time), and why the style
    /// model couldn't hear a track, when it couldn't.
    Kept(Box<KeptReference>, Vec<String>, Vec<String>, Option<String>),
    Ask {
        question: String,
        options: Vec<Choice>,
    },
}

impl ReferenceTool {
    async fn measure(&self, what: &str, count: usize, signal: Signal) -> Result<Measured, String> {
        // A few spare tracks: an upload that doesn't match is passed over, not counted as one less example.
        let (name, kind, mut tracks, mbid) = match self.sources.resolve(what, count + 4, signal.clone()).await? {
            Resolved::Ask { question, options } => return Ok(Measured::Ask { question, options }),
            Resolved::Tracks { name, kind, tracks, mbid } => (name, kind, tracks, mbid),
        };
        // A playlist stands for its videos.
        if kind == Kind::Playlist {
            let url = tracks.first().and_then(|track| track.url.clone()).unwrap_or_default();
            tracks = self.fetcher.playlist(&url, count + 4, signal.clone()).await?;
        }
        // One file the producer gave is taken as it is, however short.
        let alone = tracks.len() == 1 && matches!(kind, Kind::Files | Kind::Video);
        let mut profiles = vec![];
        let mut used = vec![];
        let mut missed = vec![];
        let mut unstyled = None;
        let fetched = std::cell::RefCell::new(vec![]);
        let say = |said: &str| fetched.borrow_mut().push(said.to_string());
        for wanted in &tracks {
            if profiles.len() >= count {
                break;
            }
            signal.check().map_err(|error| error.to_string())?;
            match self.one(wanted, alone, &say, signal.clone()).await {
                Ok((profile, source, style)) => {
                    unstyled = unstyled.or(style);
                    profiles.push(profile);
                    used.push(KeptTrack { artist: wanted.artist.clone(), title: wanted.title.clone(), source, mbid: wanted.mbid.clone() });
                }
                Err(why) => missed.push(format!("{}: {why}", label(wanted))),
            }
        }
        let wanted_least = if matches!(kind, Kind::Files | Kind::Video | Kind::Playlist) { 1 } else { 3.min(tracks.len()).min(count) };
        if profiles.len() < wanted_least.max(1) {
            return Err(format!(
                "Kumi could measure only {} of {name}'s tracks{}",
                profiles.len(),
                if missed.is_empty() {
                    String::new()
                } else {
                    format!(": {}", missed.iter().take(3).cloned().collect::<Vec<_>>().join("; "))
                }
            ));
        }
        let profile = Profile::combine(&name, &profiles).ok_or("Kumi measured nothing.")?;
        let kept = KeptReference {
            version: 1,
            key: ReferenceStore::key(what),
            name,
            kind: format!("{kind:?}").to_lowercase(),
            tracks: used,
            profile,
            at: now_ms(),
            mbid,
            stamp: if kind == Kind::Files { stamp(what) } else { None },
        };
        self.store.save(&kept).await.map_err(|error| format!("Kumi measured it but couldn't keep it: {error}"))?;
        Ok(Measured::Kept(Box::new(kept), missed, fetched.take(), unstyled))
    }

    /// One track's profile and where its audio came from: fetched (or found), checked to be a finished track, measured.
    /// Audio Kumi fetched goes once it's measured. Beside them, why the style model couldn't hear it, when it couldn't.
    async fn one(
        &self,
        wanted: &Wanted,
        alone: bool,
        say: embed::Say<'_>,
        signal: Signal,
    ) -> Result<(Profile, String, Option<String>), String> {
        let audio = self.fetcher.audio(wanted, signal.clone()).await?;
        let heard = measure_file(
            &audio.file.to_string_lossy(),
            MeasureOptions { start: None, seconds: Some(MEASURED), signal: Some(signal.clone()) },
        )
        .await
        .map_err(|error| head(&error.to_string(), 200));
        // How it sounds to the style model, while the audio is still here (when the embeddings slot isn't off).
        let vibe = match &heard {
            Ok(heard) if embeddings_on() => {
                let slot = crate::slots::kept().model_file(crate::slots::Job::Embeddings);
                let seconds = heard.measures.seconds.min(MEASURED);
                Some(embed::vibe(&audio.file, 0., seconds, heard.measures.integrated, slot, say, &signal).await)
            }
            _ => None,
        };
        if audio.fetched {
            let _ = tokio::fs::remove_file(&audio.file).await;
        }
        let heard = heard?;
        let measures = &heard.measures;
        match measures.integrated.filter(|loudness| loudness.is_finite()) {
            None => return Err("silent, nothing to measure".into()),
            Some(loudness) if loudness < QUIETEST => return Err(format!("too quiet for a finished track ({loudness:.0} LUFS)")),
            Some(_) => {}
        }
        if !alone && measures.seconds < SHORTEST {
            return Err(format!("{:.0} s long, a sample or a sketch rather than a track", measures.seconds));
        }
        let mut profile = Profile::of(&label(wanted), &heard);
        let style = match vibe {
            Some(Ok(vibe)) => {
                profile.vibe = Some(vibe);
                None
            }
            Some(Err(why)) => Some(why),
            None => None,
        };
        Ok((profile, audio.source, style))
    }
}

fn label(wanted: &Wanted) -> String {
    if wanted.artist.is_empty() {
        wanted.title.clone()
    } else {
        format!("{} – {}", wanted.artist, wanted.title)
    }
}

/// A kept reference as the model reads it: its tracks and each measure's typical value and range, and how judge finds
/// it again (by what was asked for).
fn reply(kept: &KeptReference, cached: bool, what: &str) -> Value {
    let p = &kept.profile;
    let spread = |spread: &Spread, unit: &str| format!("{} {unit} ({} to {})", spread.mid, spread.low, spread.high);
    let mut measures = serde_json::Map::new();
    if let Some(value) = &p.integrated {
        measures.insert("loudness".into(), json!(spread(value, "LUFS")));
    }
    if let Some(value) = &p.plr {
        measures.insert("dynamics (peak to loudness)".into(), json!(spread(value, "dB")));
    }
    if let Some(value) = &p.crest {
        measures.insert("punch (crest)".into(), json!(spread(value, "dB")));
    }
    if let Some(value) = &p.low_width {
        measures.insert("stereo below 120 Hz".into(), json!(spread(value, "dB")));
    }
    measures.insert("brightness (tilt)".into(), json!(spread(&p.tilt, "dB/oct")));
    if let Some(value) = &p.range {
        measures.insert("contrast between sections (loudness range)".into(), json!(spread(value, "LU")));
    }
    let balance: serde_json::Map<String, Value> =
        p.regions.iter().enumerate().map(|(region, value)| (REGIONS[region].0.to_string(), json!(spread(value, "dB")))).collect();
    measures.insert("balance (each region against the whole)".into(), Value::Object(balance));
    json!({
        "name": kept.name,
        "kind": kept.kind,
        "tracks": kept.tracks.iter().map(|track| if track.artist.is_empty() { track.title.clone() } else { format!("{} – {}", track.artist, track.title) }).collect::<Vec<_>>(),
        "measures": measures,
        "kept": if cached { "measured before, read back" } else { "measured now, kept for next time" },
        "note": format!("To work toward it: judge with goal.reference \"{what}\". Its ranges are the style's: inside them is in the style.")
    })
}
