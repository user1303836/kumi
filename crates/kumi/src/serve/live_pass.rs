//! What the runtime adds to every pass Live's own listening tools play here (judge, tune): the plugin host's limits as
//! the runtime can say them before Live is asked, the call's own folder for what the pass captures, and a reference
//! made readable with what this computer has installed.
//! - A pass plays at most a minute from Live starting to stopping: a bar before the stretch (less from the
//!   Arrangement's start), the stretch, and half a bar after it. Live's integration holds that on every pass, the one
//!   played again when Live reached the stretch late included; the runtime says it first, at the tempo Live last said.
//! - What a pass captures goes into the call's scratch under the runtime's state (`<state>/scratch/listen/<id>/`),
//!   removed with the call: never Kumi's shared folder.
//! - A reference is decoded as the runtime decodes everything: WAV and AIFF read as they are, other audio only through
//!   the ffmpeg setup allowed. Nothing is downloaded to read it.

use super::artifacts::StagedArtifact;
use super::ctx::{Ctx, OpError, Scratch};
use super::listen::{stage_source, Source};
use kumi_agent::core::contracts::Integration;
use kumi_common::abort::Signal;
use kumi_media::audio::decode::{prepare_audio_to_with_options, DecodeOptions};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The longest a pass plays here, seconds (the plugin host's listening limit).
pub(super) const MOST_SECONDS: f64 = 60.;

/// How long Live plays a pass over `beats` from `from_beat`, seconds, at `tempo` and `beats_per_bar`: a bar before
/// the stretch (less when it starts sooner), the stretch, and half a bar after it.
pub(super) fn played_seconds(from_beat: f64, beats: f64, tempo: f64, beats_per_bar: f64) -> f64 {
    let lead = from_beat.min(beats_per_bar);
    (lead + beats + beats_per_bar / 2.) * 60. / tempo
}

/// Why a pass over `beats` from `from_beat` won't play here, at the tempo and meter Live last said: over a minute with
/// its lead-in and what follows. None when it fits, or Live hasn't said its tempo (its integration says it then).
pub(super) fn window_refusal(ctx: &Ctx, from_beat: f64, beats: f64) -> Option<OpError> {
    let transport = ctx.link().connection().and_then(|connection| connection.transport())?;
    let tempo = transport.tempo.filter(|tempo| *tempo > 0.)?;
    let meter = transport.beats_per_bar.filter(|beats| *beats > 0.).unwrap_or(4.);
    let seconds = played_seconds(from_beat, beats, tempo, meter);
    (seconds > MOST_SECONDS).then(|| OpError::Refused {
        code: "window_too_long",
        reason: format!(
            "{beats} beats at {tempo} BPM plays {seconds:.1} seconds with the bar before it and half a bar after: Kumi plays at most \
             60 seconds at a time here, so hear a shorter stretch (from_beat and beats)."
        ),
        owner: None,
    })
}

/// The call's own folder for what Live's listening captures, given to the integration while this lives: the integration
/// captures nowhere else, and forgets it, then the folder goes with all that's in it, when this drops.
pub(super) struct Capturing {
    integration: Rc<dyn Integration>,
    scratch: Scratch,
}

impl Capturing {
    pub(super) fn new(ctx: &Ctx) -> Result<Self, OpError> {
        let integration = ctx.connection()?.integration.clone();
        // Directly under the listening policy's root, as the integration checks.
        let scratch = ctx.scratch("listen")?;
        integration.listen_scratch(Some(scratch.path().to_path_buf()));
        Ok(Self { integration, scratch })
    }

    pub(super) fn folder(&self) -> &Path {
        self.scratch.path()
    }
}

impl Drop for Capturing {
    fn drop(&mut self) {
        self.integration.listen_scratch(None);
    }
}

/// A reference source staged for Live's listening to read: the staged copy when it's WAV or AIFF, else a WAV decoded
/// into `folder` through the ffmpeg setup allowed, refused in the runtime's words without one. The staged copy lives
/// as long as what's returned.
pub(super) async fn readable_reference(
    ctx: &Ctx,
    source: &Source<'_>,
    folder: &Path,
    signal: &Signal,
) -> Result<(PathBuf, StagedArtifact), OpError> {
    let staged = stage_source(ctx, source, signal).await?;
    let extension = staged.path().extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ["wav", "wave", "aif", "aiff", "aifc"].contains(&extension.as_str()) {
        return Ok((staged.path().to_path_buf(), staged));
    }
    let decode = DecodeOptions { installed_only: true, ffmpeg: ctx.media().tool("ffmpeg").map(Path::to_path_buf) };
    let prepared = prepare_audio_to_with_options(staged.path(), Some(signal.clone()), None, &decode)
        .await
        .map_err(|error| OpError::Refused { code: "unreadable_reference", reason: error.0, owner: None })?;
    // One reference a call, and the folder is the call's own.
    let wav = folder.join("reference.wav");
    let copied = tokio::fs::copy(&prepared.path, &wav).await;
    prepared.cleanup().await;
    copied.map_err(|error| OpError::Failed { reason: format!("Kumi couldn't keep the decoded reference: {error}") })?;
    Ok((wav, staged))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime's own check counts what Live's integration counts: a bar's lead-in and half a bar after.
    #[test]
    fn a_pass_is_counted_with_its_lead_in_and_what_follows() {
        // At 120 BPM in 4/4, a beat is half a second: 4 beats before, 2 after.
        assert_eq!(played_seconds(16., 114., 120., 4.), 60.);
        assert!(played_seconds(16., 116., 120., 4.) > MOST_SECONDS);
        // From the Arrangement's start there's no lead-in to play.
        assert_eq!(played_seconds(0., 118., 120., 4.), 60.);
        assert_eq!(played_seconds(2., 116., 120., 4.), 60.);
    }

    /// A compressed reference without the ffmpeg setup allowed is refused in the runtime's words, and nothing is
    /// decoded (or downloaded) another way; a WAV is read as it is.
    #[tokio::test(flavor = "current_thread")]
    async fn a_compressed_reference_needs_the_ffmpeg_setup_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("approved");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let ctx = Ctx::new(
            super::super::Args {
                state_dir: dir.path().join("state"),
                kumi_home: dir.path().join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: kumi_types::daw::DawId::Ableton,
                library_roots: vec![root.clone()],
                media_roots: Vec::new(),
                // No ffmpeg allowed at setup.
                tools: Default::default(),
            },
            kumi_types::system::Env::new(),
            Default::default(),
        )
        .unwrap();
        let mp3 = root.join("ref.mp3");
        std::fs::write(&mp3, b"ID3 not really audio").unwrap();
        let signal = Signal::new();
        let file = mp3.to_string_lossy().into_owned();
        let source = Source::File(&file);
        let folder = ctx.scratch("listen").unwrap();
        let Err(refused) = readable_reference(&ctx, &source, folder.path(), &signal).await else { panic!("decoded without ffmpeg") };
        assert_eq!(refused.code(), "unreadable_reference");
        assert!(refused.reason().contains("approve a local decoder at setup, or export as WAV/AIFF"), "{}", refused.reason());
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0, "nothing decoded");
        let wav = root.join("ref.wav");
        kumi_media::audio::decode::write_test_wav(&wav, &[0.; 4800], 48_000);
        let file = wav.to_string_lossy().into_owned();
        let (path, staged) = readable_reference(&ctx, &Source::File(&file), folder.path(), &signal).await.unwrap();
        assert_eq!(path, staged.path());
    }
}
