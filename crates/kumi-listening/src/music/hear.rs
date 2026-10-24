//! Hearing one song: lightly (the style model from one window halfway through, the profile from a minute starting at
//! 30%) or in full (the style model from windows at a quarter, half and three quarters, the profile as the reference
//! tool measures a track: up to 360 s). The file is decoded once, and only the stretch heard (a compressed file is
//! converted from where it's heard on).

use super::store::{song_name, Song, Stage, TOO_QUIET, TOO_SHORT, WONT_DECODE};
use crate::{
    checklist::Profile,
    embed::{self, Say},
    measure::{measure_file, MeasureOptions},
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_media::audio::decode::{open_audio_to, prepare_audio_range};
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

/// Where a song is heard, in seconds of it: the profile's stretch, the style model's windows, and the stretch decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub profile: (f64, f64),
    pub windows: Vec<f64>,
    pub decoded: (f64, f64),
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
    let from = windows.iter().fold(profile.0, |from, at| from.min(*at));
    let until = windows.iter().fold(profile.0 + profile.1, |until, at| until.max(at + WINDOW)).min(seconds);
    Plan { profile, windows, decoded: (from, until - from) }
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
    /// Hearing can't go on, for no reason of this song's (no room to convert it, or stopped): the scan stops.
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

/// How long a song is: its tags' length, else its file's (read as far as a full listen reaches).
async fn length(song: &Song, signal: &Signal) -> Result<f64, Heard> {
    if let Some(seconds) = song.tags.seconds.filter(|seconds| *seconds > 0.) {
        return Ok(seconds);
    }
    let mut source = open_audio_to(&song.path, Some(signal.clone()), Some(FULL_SECONDS + 1.)).await.map_err(|error| {
        if error.is_no_room() {
            Heard::Stop(error.to_string())
        } else {
            Heard::Skipped(WONT_DECODE.into())
        }
    })?;
    let seconds = source.frames as f64 / source.sample_rate;
    let _ = source.close().await;
    Ok(seconds)
}

/// Hear a song at a stage (light or full).
pub async fn hear(song: &Song, stage: Stage, ears: &Ears<'_>) -> Heard {
    let seconds = match length(song, ears.signal).await {
        Ok(seconds) => seconds,
        Err(heard) => return heard,
    };
    if seconds < SHORTEST {
        return Heard::Skipped(TOO_SHORT.into());
    }
    let plan = plan(seconds, stage);
    let prepared = match prepare_audio_range(&song.path, Some(ears.signal.clone()), plan.decoded.0, plan.decoded.1).await {
        Ok(prepared) => prepared,
        Err(_) if ears.signal.is_cancelled() => return Heard::Stop("stopped".into()),
        Err(error) if error.is_no_room() => return Heard::Stop(error.to_string()),
        Err(_) => return Heard::Skipped(WONT_DECODE.into()),
    };
    // Seconds of the song, as seconds of what was decoded.
    let offset = prepared.start;
    let file = prepared.path.to_string_lossy().into_owned();
    let options = MeasureOptions { start: Some(plan.profile.0 - offset), seconds: Some(plan.profile.1), signal: Some(ears.signal.clone()) };
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
        Some(style) => {
            let starts: Vec<f64> = plan.windows.iter().map(|at| at - offset).collect();
            match style.embed(&prepared.path, &starts, loudness, ears.say, ears.signal).await {
                Ok(vector) => (Some(vector), None),
                Err(_) if ears.signal.is_cancelled() => return Heard::Stop("stopped".into()),
                Err(why) => (None, Some(why)),
            }
        }
        None => (None, None),
    };
    drop(prepared);
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
    let seconds = length(song, ears.signal).await.map_err(|_| WONT_DECODE.to_string())?;
    let windows = plan(seconds, stage).windows;
    let from = windows.iter().fold(f64::INFINITY, |from, at| from.min(*at));
    let until = windows.iter().fold(0f64, |until, at| until.max(at + WINDOW)).min(seconds);
    let prepared =
        prepare_audio_range(&song.path, Some(ears.signal.clone()), from, until - from).await.map_err(|error| error.to_string())?;
    let starts: Vec<f64> = windows.iter().map(|at| at - prepared.start).collect();
    // Heard at the loudness its profile measured, as it was the first time.
    let loudness = song
        .profile
        .as_deref()
        .and_then(|profile| serde_json::from_str::<Profile>(profile).ok())
        .and_then(|profile| profile.integrated.map(|spread| spread.mid));
    style.embed(&prepared.path, &starts, loudness, ears.say, ears.signal).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_is_heard_where_the_stage_says() {
        let light = plan(300., Stage::Light);
        assert_eq!(light, Plan { profile: (90., 60.), windows: vec![145.], decoded: (90., 65.) });
        let full = plan(300., Stage::Full);
        assert_eq!(full, Plan { profile: (0., 300.), windows: vec![70., 145., 220.], decoded: (0., 300.) });
        // A long song: its profile stops at 360 s, its last window is past it.
        let long = plan(600., Stage::Full);
        assert_eq!((long.profile, long.windows.clone(), long.decoded), ((0., 360.), vec![145., 295., 445.], (0., 455.)));
        // A short one: windows stay inside it.
        let short = plan(40., Stage::Full);
        assert_eq!((short.windows.clone(), short.decoded), (vec![5., 15., 25.], (0., 40.)));
        assert_eq!(plan(40., Stage::Light).profile, (12., 28.));
    }
}
