//! The reference tool: a reference in any form in, its measured profile out (or the one question that settles what
//! it is), kept so it's measured once.

use super::{
    fetch::Fetcher,
    sources::{Kind, Resolved, Sources, Wanted},
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
const DESCRIPTION: &str = "Turn a reference into measured targets, once: an audio file or a folder of them, a YouTube video or playlist, a Spotify link (track, album, playlist or artist), or words: an artist (\"bladee\"), an album (\"Kid A\") or a genre or style (\"dub techno\"). Kumi finds example tracks (for words, the artist's most listened recordings, an album's tracks or a genre's main artists, through MusicBrainz and ListenBrainz; the audio from YouTube Music, matched by length), measures each, and keeps a profile with each measure's typical value and the range the tracks keep to: loudness, dynamics, punch, tonal balance by region, stereo below 120 Hz and brightness. It's kept, so asking again costs nothing. When the words could mean more than one thing it answers with a question: ask the producer that one question, then call again with their answer. Work toward the profile with judge (goal.reference: its name). For one element of a reference (its bass, its drums), put the track in the Set, separate its stems (live_command separate_stems) and give a stem's clipRef. Fetching and measuring takes a minute or two.";
/// How long a track is heard for its profile: long enough for its loud and quiet parts.
const MEASURED: f64 = 360.;

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
                "what": {"type": "string", "minLength": 1, "maxLength": 1024, "description": "The reference: a file or folder path, a clip in the Set (its clipRef), a YouTube or Spotify link, or an artist, album or genre in plain words"},
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
                return Ok(ToolResult::text(stringify(&reply(&kept, true))));
            }
        }
        match self.measure(&what, count, signal.clone()).await {
            Ok(Measured::Kept(kept, missed)) => {
                let mut said = reply(&kept, false);
                if !missed.is_empty() {
                    said["passedOver"] = json!(missed);
                }
                Ok(ToolResult::text(stringify(&said)))
            }
            Ok(Measured::Ask { question, options }) => Ok(ToolResult::text(stringify(&json!({
                "question": question,
                "options": options,
                "note": "Ask the producer this one question (with these options when there are some), then call reference again with what they say."
            })))),
            Err(why) => {
                signal.check()?;
                Ok(ToolResult::error(why))
            }
        }
    }
}

enum Measured {
    /// Kept, and the tracks passed over (with why).
    Kept(KeptReference, Vec<String>),
    Ask {
        question: String,
        options: Vec<String>,
    },
}

impl ReferenceTool {
    async fn measure(&self, what: &str, count: usize, signal: Signal) -> Result<Measured, String> {
        // A few spare tracks: an upload that doesn't match is passed over, not counted as one less example.
        let (name, kind, mut tracks) = match self.sources.resolve(what, count + 4, signal.clone()).await? {
            Resolved::Ask { question, options } => return Ok(Measured::Ask { question, options }),
            Resolved::Tracks { name, kind, tracks } => (name, kind, tracks),
        };
        // A playlist link stands for its videos.
        if kind == Kind::Video && crate::video::youtube_id(what).is_none() {
            tracks = self.fetcher.playlist(what, count, signal.clone()).await?;
        }
        let mut profiles = vec![];
        let mut used = vec![];
        let mut missed = vec![];
        for wanted in &tracks {
            if profiles.len() >= count {
                break;
            }
            signal.check().map_err(|error| error.to_string())?;
            match self.one(wanted, signal.clone()).await {
                Ok(profile) => {
                    profiles.push(profile);
                    used.push(KeptTrack {
                        artist: wanted.artist.clone(),
                        title: wanted.title.clone(),
                        source: wanted
                            .file
                            .as_ref()
                            .map(|file| file.display().to_string())
                            .or_else(|| wanted.url.clone())
                            .unwrap_or_else(|| "YouTube Music".into()),
                    });
                }
                Err(why) => missed.push(format!("{}: {why}", label(wanted))),
            }
        }
        let wanted_least = if kind == Kind::Files || kind == Kind::Video { 1 } else { 3.min(tracks.len()).min(count) };
        if profiles.len() < wanted_least {
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
        };
        self.store.save(&kept).await.map_err(|error| format!("Kumi measured it but couldn't keep it: {error}"))?;
        Ok(Measured::Kept(kept, missed))
    }

    /// One track's profile: its audio fetched (or found) and measured.
    async fn one(&self, wanted: &Wanted, signal: Signal) -> Result<Profile, String> {
        let file = self.fetcher.audio(wanted, signal.clone()).await?;
        let heard = measure_file(&file.to_string_lossy(), MeasureOptions { start: None, seconds: Some(MEASURED), signal: Some(signal) })
            .await
            .map_err(|error| head(&error.to_string(), 200))?;
        Ok(Profile::of(&label(wanted), &heard))
    }
}

fn label(wanted: &Wanted) -> String {
    if wanted.artist.is_empty() {
        wanted.title.clone()
    } else {
        format!("{} – {}", wanted.artist, wanted.title)
    }
}

/// A kept reference as the model reads it: its tracks and each measure's typical value and range.
fn reply(kept: &KeptReference, cached: bool) -> Value {
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
    let balance: serde_json::Map<String, Value> =
        p.regions.iter().enumerate().map(|(region, value)| (REGIONS[region].0.to_string(), json!(spread(value, "dB")))).collect();
    measures.insert("balance (each region against the whole)".into(), Value::Object(balance));
    json!({
        "name": kept.name,
        "kind": kept.kind,
        "tracks": kept.tracks.iter().map(|track| if track.artist.is_empty() { track.title.clone() } else { format!("{} – {}", track.artist, track.title) }).collect::<Vec<_>>(),
        "measures": measures,
        "kept": if cached { "measured before, read back" } else { "measured now, kept for next time" },
        "note": format!("To work toward it: judge with goal.reference \"{}\". Its ranges are the style's: inside them is in the style.", kept.name)
    })
}
