//! Hearing one song: lightly (the style model from one window halfway through, the profile from a minute starting at
//! 30%) or in full (the style model from windows at a quarter, half and three quarters, the profile as the reference
//! tool measures a track: up to 360 s). Only the stretches heard are decoded (a compressed file is converted from
//! where each is heard on): a two-hour mix needs no more of its audio than a song does.

use super::store::{song_name, Song, Stage, TOO_QUIET, TOO_SHORT, WONT_DECODE};
use crate::{
    checklist::Profile,
    embed::{self, Say},
    measure::{measure_file, MeasureOptions},
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_media::audio::decode::{open_audio_to, prepare_audio_range, AudioError, PreparedAudio};
use std::path::{Path, PathBuf};

/// A song shorter than this isn't a finished track (the reference tool's limit)...
pub const SHORTEST: f64 = 30.;
/// ... nor one quieter than this (LUFS).
pub const QUIETEST: f64 = -45.;
/// The light stage's minute, and where in the song it starts.
const LIGHT_SECONDS: f64 = 60.;
const LIGHT_AT: f64 = 0.3;
/// A full listen's profile, as the reference tool measures a track.
const FULL_SECONDS: f64 = crate::references::tool::MEASURED;
/// The style model's window.
const WINDOW: f64 = embed::CLAP_SAMPLES as f64 / embed::CLAP_RATE;
/// Stretches heard this close together are decoded as one: less than another conversion costs.
const GAP: f64 = 30.;

/// Where a song is heard, in seconds of it: the profile's stretch, the style model's windows, and the stretches decoded
/// (from, how long) to hear them.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub profile: (f64, f64),
    pub windows: Vec<f64>,
    pub decoded: Vec<(f64, f64)>,
}

/// The stretches that cover `spans` (from, until), as few as decode them without decoding much else, in order.
fn stretches(mut spans: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = vec![];
    for (from, until) in spans {
        match merged.last_mut() {
            Some(last) if from <= last.1 + GAP => last.1 = last.1.max(until),
            _ => merged.push((from, until)),
        }
    }
    merged.into_iter().map(|(from, until)| (from, until - from)).collect()
}

/// Where a song `seconds` long is heard at a stage.
pub fn plan(seconds: f64, stage: Stage) -> Plan {
    let window_at = |share: f64| (share * seconds - WINDOW / 2.).clamp(0., (seconds - WINDOW).max(0.));
    let (profile, windows) = match stage {
        Stage::Full => ((0., seconds.min(FULL_SECONDS)), vec![window_at(0.25), window_at(0.5), window_at(0.75)]),
        _ => {
            let start = LIGHT_AT * seconds;
            ((start, LIGHT_SECONDS.min(seconds - start)), vec![window_at(0.5)])
        }
    };
    let mut spans = vec![(profile.0, profile.0 + profile.1)];
    spans.extend(windows.iter().map(|at| (*at, (at + WINDOW).min(seconds))));
    Plan { profile, windows, decoded: stretches(spans) }
}

/// What of a song to play to stand for it: the window the style model hears first (halfway through), from where and
/// how long, in seconds.
pub fn heard_first(seconds: f64) -> (f64, f64) {
    (plan(seconds, Stage::Light).windows[0], WINDOW.min(seconds))
}

/// What hearing a song gave.
#[derive(Debug, Clone)]
pub enum Heard {
    /// Heard: its profile (without its style vector, kept apart), its style vector and which model made it (when the
    /// style model heard it, else why not), and how long it is.
    Done { profile: Box<Profile>, vector: Option<(Vec<f32>, String)>, unstyled: Option<String>, seconds: f64 },
    /// Why it isn't heard, kept with the song.
    Skipped(String),
    /// It can't be heard now, for a reason that may pass: its file isn't there (gone since the walk, which the next
    /// walk sees, or on a drive that isn't there now: `away`, its music folder gone too). Nothing is kept of it; a
    /// later run hears it.
    Later { why: String, away: bool },
    /// Hearing can't go on, for no reason of this song's (no room to convert it, no converter, or stopped): the scan
    /// stops.
    Stop(String),
}

/// The style model the scan hears with: CLAP as the embeddings slot runs it, or a test's own.
#[async_trait(?Send)]
pub trait Style {
    /// Which model it is (`embed::style_id`): its vectors go to its own file.
    fn model(&self) -> &str;
    /// What windows of a file, starting at `starts` (seconds), sound like to it, and which model heard them.
    async fn embed(
        &self,
        file: &Path,
        starts: &[f64],
        loudness: Option<f64>,
        say: Say<'_>,
        signal: &Signal,
    ) -> Result<(Vec<f32>, String), String>;
    /// What words mean to it, on the same map (unit length): only a model with a text side that matches it.
    async fn words(&self, _text: &str, _say: Say<'_>, _signal: &Signal) -> Result<Vec<f32>, String> {
        Err("this style model has no words".into())
    }
}

/// Why words can't find songs when the embeddings slot runs another style model than Kumi's own.
pub const SWAPPED: &str =
    "the embeddings slot runs another style model than Kumi's own, and Kumi's words model only matches its own (/slots embeddings default swaps it back)";

/// CLAP, as the embeddings slot runs it: the slot's model file, else Kumi's own.
pub struct Clap {
    slot: Option<PathBuf>,
    model: String,
}

impl Clap {
    /// The style model the slot runs now: None when the slot is off.
    pub async fn from_slot(signal: &Signal) -> Option<Clap> {
        let kept = crate::slots::kept();
        if kept.now(crate::slots::Job::Embeddings) == crate::slots::Choice::Off {
            return None;
        }
        let slot = kept.model_file(crate::slots::Job::Embeddings);
        let model = embed::style_id(slot.as_deref(), signal).await;
        Some(Clap { slot, model })
    }
}

#[async_trait(?Send)]
impl Style for Clap {
    fn model(&self) -> &str {
        &self.model
    }
    async fn embed(
        &self,
        file: &Path,
        starts: &[f64],
        loudness: Option<f64>,
        say: Say<'_>,
        signal: &Signal,
    ) -> Result<(Vec<f32>, String), String> {
        embed::vibe_at(file, starts, loudness, self.slot.clone(), say, signal).await
    }
    /// The text side of Kumi's own model: a swapped slot's model has none that matches it.
    async fn words(&self, text: &str, say: Say<'_>, signal: &Signal) -> Result<Vec<f32>, String> {
        if self.slot.is_some() {
            return Err(SWAPPED.into());
        }
        crate::words::words(text, say, signal).await
    }
}

/// How the scan hears: the style model (none when the embeddings slot is off), how a fetch tells what it's doing, and
/// the scan's stop.
pub struct Ears<'a> {
    pub style: Option<&'a dyn Style>,
    pub say: Say<'a>,
    pub signal: &'a Signal,
}

/// A song's name as its profile carries it.
pub fn label(song: &Song) -> String {
    song_name(song.tags.artist.as_deref(), song.tags.title.as_deref(), &song.path)
}

/// A song whose file isn't there now: left for later.
fn absent(song: &Song) -> Option<Heard> {
    if Path::new(&song.path).is_file() {
        return None;
    }
    let away = !Path::new(&song.folder).is_dir();
    let why = if away { format!("{} isn't there now", song.folder) } else { "its file isn't there now".into() };
    Some(Heard::Later { why, away })
}

/// Formats Kumi reads in place; every other needs a converter (ffmpeg, or a Mac's afconvert).
fn read_in_place(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".wav", ".wave", ".aif", ".aiff", ".aifc"].iter().any(|extension| lower.ends_with(extension))
}

/// A file that couldn't be decoded: one that won't (a converter tried it), or, where there's no converter at all (no
/// ffmpeg, and none could be fetched), every compressed song until there is: the scan stops and says so, keeping
/// nothing against the song.
async fn undecodable(song: &Song, error: &AudioError, signal: &Signal) -> Heard {
    if signal.is_cancelled() {
        return Heard::Stop("stopped".into());
    }
    if error.is_no_room() {
        return Heard::Stop(error.to_string());
    }
    if cfg!(target_os = "macos") || read_in_place(&song.path) {
        return Heard::Skipped(WONT_DECODE.into());
    }
    let options = kumi_media::programs::FfmpegOptions { installed_only: true, signal: Some(signal.clone()), ..Default::default() };
    match kumi_media::programs::find_ffmpeg(options).await {
        Ok(Some(_)) => Heard::Skipped(WONT_DECODE.into()),
        _ => Heard::Stop(format!(
            "Kumi needs ffmpeg to hear your {} songs, and found none here nor could fetch one: it tries again later",
            Path::new(&song.path).extension().unwrap_or_default().to_string_lossy().to_uppercase()
        )),
    }
}

/// How long a song is: its tags' length, else its file's (read as far as a full listen reaches).
async fn length(song: &Song, signal: &Signal) -> Result<f64, Heard> {
    if let Some(seconds) = song.tags.seconds.filter(|seconds| *seconds > 0.) {
        return Ok(seconds);
    }
    let mut source = match open_audio_to(&song.path, Some(signal.clone()), Some(FULL_SECONDS + 1.)).await {
        Ok(source) => source,
        Err(error) => return Err(undecodable(song, &error, signal).await),
    };
    let seconds = source.frames as f64 / source.sample_rate;
    let _ = source.close().await;
    Ok(seconds)
}

/// Each of a plan's stretches decoded, in order. A copy of the whole file (a Mac's afconvert can't convert a stretch) is
/// linked for the stretches after the first, each under a path of its own as a copy of its own was: a two-hour mix is
/// converted once, not once a stretch.
async fn decode(song: &Song, stretches: &[(f64, f64)], signal: &Signal) -> Result<Vec<PreparedAudio>, Heard> {
    let mut decoded: Vec<PreparedAudio> = Vec::with_capacity(stretches.len());
    for (from, seconds) in stretches {
        let linked = match decoded.first() {
            Some(first) => first.linked().await,
            None => None,
        };
        if let Some(linked) = linked {
            decoded.push(linked);
            continue;
        }
        match prepare_audio_range(&song.path, Some(signal.clone()), *from, *seconds).await {
            Ok(prepared) => decoded.push(prepared),
            Err(error) => return Err(undecodable(song, &error, signal).await),
        }
    }
    Ok(decoded)
}

/// The decoded stretch that holds `at` (seconds of the song), and where `at` lies in it.
fn holding<'a>(decoded: &'a [PreparedAudio], stretches: &[(f64, f64)], at: f64) -> Option<(&'a PreparedAudio, f64)> {
    let found = stretches.iter().position(|(from, seconds)| at >= *from - 1e-6 && at < from + seconds + 1e-6)?;
    let prepared = &decoded[found];
    Some((prepared, at - prepared.start))
}

/// What the style model hears in a song's windows: all at once when they're in one file (a file read in place, or one
/// decoded stretch), else each from its own stretch, the windows' vectors averaged as one call averages them.
async fn styled(
    style: &dyn Style,
    decoded: &[PreparedAudio],
    stretches: &[(f64, f64)],
    windows: &[f64],
    loudness: Option<f64>,
    ears: &Ears<'_>,
) -> Result<(Vec<f32>, String), String> {
    let at: Vec<(&Path, f64)> = windows
        .iter()
        .map(|window| holding(decoded, stretches, *window).map(|(prepared, start)| (prepared.path.as_path(), start)))
        .collect::<Option<_>>()
        .ok_or("a window lies outside what was decoded")?;
    if at.iter().all(|(file, _)| *file == at[0].0) {
        let starts: Vec<f64> = at.iter().map(|(_, start)| *start).collect();
        return style.embed(at[0].0, &starts, loudness, ears.say, ears.signal).await;
    }
    let (mut vectors, mut model) = (vec![], String::new());
    for (file, start) in at {
        let (vector, by) = style.embed(file, &[start], loudness, ears.say, ears.signal).await?;
        vectors.push(vector);
        model = by;
    }
    embed::averaged(&vectors).map(|vector| (vector, model)).ok_or_else(|| "Nothing was heard to embed.".into())
}

/// Hear a song at a stage (light or full).
pub async fn hear(song: &Song, stage: Stage, ears: &Ears<'_>) -> Heard {
    if let Some(later) = absent(song) {
        return later;
    }
    let seconds = match length(song, ears.signal).await {
        Ok(seconds) => seconds,
        Err(heard) => return heard,
    };
    if seconds < SHORTEST {
        return Heard::Skipped(TOO_SHORT.into());
    }
    let plan = plan(seconds, stage);
    let decoded = match decode(song, &plan.decoded, ears.signal).await {
        Ok(decoded) => decoded,
        Err(heard) => return heard,
    };
    let Some((prepared, start)) = holding(&decoded, &plan.decoded, plan.profile.0) else {
        return Heard::Skipped(WONT_DECODE.into());
    };
    let file = prepared.path.to_string_lossy().into_owned();
    let options = MeasureOptions { start: Some(start), seconds: Some(plan.profile.1), signal: Some(ears.signal.clone()) };
    let heard = match measure_file(&file, options).await {
        Ok(heard) => heard,
        Err(_) if ears.signal.is_cancelled() => return Heard::Stop("stopped".into()),
        Err(_) => return Heard::Skipped(WONT_DECODE.into()),
    };
    let loudness = heard.measures.integrated.filter(|loudness| loudness.is_finite());
    if loudness.is_none_or(|loudness| loudness < QUIETEST) {
        return Heard::Skipped(TOO_QUIET.into());
    }
    let (vector, unstyled) = match ears.style {
        Some(style) => match styled(style, &decoded, &plan.decoded, &plan.windows, loudness, ears).await {
            Ok(vector) => (Some(vector), None),
            Err(_) if ears.signal.is_cancelled() => return Heard::Stop("stopped".into()),
            Err(why) => (None, Some(why)),
        },
        None => (None, None),
    };
    drop(decoded);
    let name = label(song);
    let profile = match tokio::task::spawn_blocking(move || Profile::of(&name, &heard)).await {
        Ok(profile) => profile,
        Err(error) => return Heard::Stop(error.to_string()),
    };
    Heard::Done { profile: Box::new(Profile { vibe: None, vibe_model: None, effects: None, ..profile }), vector, unstyled, seconds }
}

/// A song heard before (by another style model) heard by this one: its windows only, at the stage it was heard at.
pub async fn restyle(song: &Song, stage: Stage, ears: &Ears<'_>) -> Result<(Vec<f32>, String), String> {
    let style = ears.style.ok_or("the style model is off")?;
    let why = |heard: Heard| match heard {
        Heard::Skipped(why) | Heard::Stop(why) | Heard::Later { why, .. } => why,
        Heard::Done { .. } => WONT_DECODE.to_string(),
    };
    if let Some(later) = absent(song) {
        return Err(why(later));
    }
    let seconds = length(song, ears.signal).await.map_err(why)?;
    let windows = plan(seconds, stage).windows;
    let spans = stretches(windows.iter().map(|at| (*at, (at + WINDOW).min(seconds))).collect());
    let decoded = decode(song, &spans, ears.signal).await.map_err(why)?;
    // Heard at the loudness its profile measured, as it was the first time.
    let loudness = song
        .profile
        .as_deref()
        .and_then(|profile| serde_json::from_str::<Profile>(profile).ok())
        .and_then(|profile| profile.integrated.map(|spread| spread.mid));
    styled(style, &decoded, &spans, &windows, loudness, ears).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_is_heard_where_the_stage_says() {
        let light = plan(300., Stage::Light);
        assert_eq!(light, Plan { profile: (90., 60.), windows: vec![145.], decoded: vec![(90., 65.)] });
        let full = plan(300., Stage::Full);
        assert_eq!(full, Plan { profile: (0., 300.), windows: vec![70., 145., 220.], decoded: vec![(0., 300.)] });
        // A long song: its profile stops at 360 s, and its last window, past it, is decoded on its own.
        let long = plan(600., Stage::Full);
        assert_eq!((long.profile, long.windows.clone(), long.decoded), ((0., 360.), vec![145., 295., 445.], vec![(0., 360.), (445., 10.)]));
        // A short one: windows stay inside it.
        let short = plan(40., Stage::Full);
        assert_eq!((short.windows.clone(), short.decoded), (vec![5., 15., 25.], vec![(0., 40.)]));
        assert_eq!(plan(40., Stage::Light).profile, (12., 28.));
    }

    /// On a Mac without ffmpeg, afconvert copies a song whole whatever stretch is asked for: a song heard in two
    /// stretches is converted once, the second reading the same copy under a path of its own.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "current_thread")]
    async fn a_copy_of_the_whole_song_is_converted_once_for_all_its_stretches() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (programs, converted, copy) = (dir.path().join("programs"), dir.path().join("converted"), dir.path().join("copy.wav"));
        std::fs::create_dir(&programs).unwrap();
        std::fs::write(&copy, b"RIFF the whole mix").unwrap();
        // No ffmpeg (nor ffprobe), and an afconvert that says what it converted and writes the copy.
        let afconvert = format!("#!/bin/sh\necho \"$5\" >> '{}'\n/bin/cp '{}' \"$6\"\n", converted.display(), copy.display());
        for (name, script) in [("ffmpeg", "#!/bin/sh\nexit 1\n".to_string()), ("afconvert", afconvert)] {
            std::fs::write(programs.join(name), script).unwrap();
            std::fs::set_permissions(programs.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("PATH", &programs);
        let path = dir.path().join("Mix.mp3");
        std::fs::write(&path, b"an mp3").unwrap();
        let song = Song {
            id: 1,
            path: path.to_string_lossy().into_owned(),
            folder: dir.path().to_string_lossy().into_owned(),
            size: 6,
            mtime: 1,
            tags: crate::music::tags::Tags::default(),
            duplicate_of: None,
            gone: false,
            stage: 0,
            skipped: None,
            profile: None,
            heard_at: None,
            used_at: None,
            cluster: None,
            cloud: false,
        };
        let stretches = [(0., 360.), (1_795., 10.)];
        let decoded = decode(&song, &stretches, &Signal::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&converted).unwrap().lines().collect::<Vec<_>>(), [song.path.as_str()], "converted once");
        assert_ne!(decoded[0].path, decoded[1].path, "a path of its own, as a copy of its own had");
        assert_eq!(std::fs::read(&decoded[1].path).unwrap(), b"RIFF the whole mix");
        let (prepared, at) = holding(&decoded, &stretches, 1_800.).unwrap();
        assert_eq!((&prepared.path, at), (&decoded[1].path, 1_800.), "read where it lies in the whole copy");
    }

    #[test]
    fn a_two_hour_mix_decodes_no_more_than_a_song_does() {
        let full = plan(7_200., Stage::Full);
        assert_eq!(full.decoded, [(0., 360.), (1_795., 10.), (3_595., 10.), (5_395., 10.)]);
        let light = plan(7_200., Stage::Light);
        assert_eq!(light.decoded, [(2_160., 60.), (3_595., 10.)]);
        for stage in [Stage::Light, Stage::Full] {
            let decoded: f64 = plan(7_200., stage).decoded.iter().map(|(_, seconds)| seconds).sum();
            assert!(decoded <= FULL_SECONDS + 3. * WINDOW, "{stage:?}: {decoded} s");
        }
    }
}
