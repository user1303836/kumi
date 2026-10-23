//! Playing a file out loud (`audition`): REAPER's own preview (`PlayPreviewEx`), to the master's hardware output, where
//! the producer listens. Nothing in the project changes. A new one stops the one playing; it stops by itself at its end,
//! when the producer presses play, on `audition_stop` and when the bridge unloads.

use std::ffi::c_void;
use std::sync::Arc;

use serde_json::{json, Value};

use super::Fault;
use crate::reaper::{Project, Reaper, Source};

unsafe extern "C" {
    fn kumi_preview_new(src: *mut c_void, out_chan: i32, curpos: f64, volume: f64) -> *mut c_void;
    fn kumi_preview_position(p: *mut c_void) -> f64;
    fn kumi_preview_set_volume(p: *mut c_void, volume: f64);
    fn kumi_preview_delete(p: *mut c_void);
}

#[cfg(test)]
unsafe extern "C" {
    pub fn kumi_preview_set_position(p: *mut c_void, at: f64);
    pub fn kumi_preview_volume(p: *mut c_void) -> f64;
    pub fn kumi_preview_out_chan(p: *mut c_void) -> i32;
}

/// The longest a preview plays (Kumi plays at most a minute of a render out loud).
pub const MAX_SECONDS: f64 = 600.0;
/// How long after Kumi's own render an audition may open REAPER's audio device itself (the render closed it).
pub const AFTER_RENDER: std::time::Duration = std::time::Duration::from_secs(5);

/// REAPER's `preview_register_t`, made by the shim (its lock is the OS's). Freed when the last holder lets go, which is
/// only ever after REAPER stopped playing it: the socket threads may still silence it a moment after, harmlessly.
pub struct Slot {
    preview: *mut c_void,
}

// SAFETY: the preview's fields Kumi touches from other threads (its volume) are read and written under its own lock,
// which REAPER's audio thread takes too; the pointer itself never changes.
unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

impl Slot {
    fn new(source: Source, out_chan: i32, from: f64, volume: f64) -> Option<Slot> {
        let preview = unsafe { kumi_preview_new(source.as_ptr(), out_chan, from, volume) };
        (!preview.is_null()).then_some(Slot { preview })
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.preview
    }

    /// Where it plays, in seconds of its file.
    pub fn position(&self) -> f64 {
        unsafe { kumi_preview_position(self.preview) }
    }

    /// Silent at once, from any thread.
    pub fn silence(&self) {
        unsafe { kumi_preview_set_volume(self.preview, 0.0) }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        unsafe { kumi_preview_delete(self.preview) }
    }
}

/// A preview playing, as the main thread holds it.
pub struct Preview {
    pub id: String,
    pub slot: Arc<Slot>,
    source: Source,
    /// Where it stops, in seconds of its file.
    until: f64,
    proj: Project,
}

/// Why REAPER can't play a file out loud now; None when it can. REAPER's audio device is opened only when Kumi's own
/// render just closed it (`after_render`): otherwise a closed device is the producer's (often freed for another app),
/// and opening a missing one can bring up REAPER's own error.
pub fn refusal(reaper: &Reaper, proj: Project, after_render: bool) -> Option<String> {
    if !reaper.audio_running() && after_render {
        reaper.audio_init();
    }
    if !reaper.audio_running() {
        return Some("REAPER's audio isn't running, so Kumi can't play its render out loud: start REAPER's audio to hear it".into());
    }
    if !reaper.audio_heard() {
        return Some("REAPER's audio device has no output the producer hears (REAPER is on its dummy device), so Kumi can't play its render out loud".into());
    }
    let state = reaper.play_state(proj);
    if state.playing || state.recording {
        return Some("REAPER is playing, so Kumi didn't play its render over it".into());
    }
    None
}

fn bad(message: impl Into<String>) -> Fault {
    ("bad-params".into(), message.into())
}

fn failed(message: impl Into<String>) -> Fault {
    ("failed".into(), message.into())
}

impl Preview {
    /// Starts playing `file` (an audio file REAPER reads) from `from` for up to `seconds`, at `gain_db`.
    pub fn start(reaper: &Reaper, proj: Project, params: &Value, after_render: bool) -> Result<(Preview, Value), Fault> {
        let id = params.get("previewId").and_then(Value::as_str).unwrap_or_default();
        if !super::render::id_ok(id, 40) {
            return Err(bad("previewId: the host's preview id"));
        }
        let file =
            params.get("file").and_then(Value::as_str).filter(|f| !f.is_empty()).ok_or_else(|| bad("file: the audio file to play"))?;
        let number = |name: &str, default: f64| -> Result<f64, Fault> {
            match params.get(name) {
                None | Some(Value::Null) => Ok(default),
                Some(value) => value.as_f64().filter(|n| n.is_finite()).ok_or_else(|| bad(format!("{name} must be a number"))),
            }
        };
        let from = number("fromSeconds", 0.0)?;
        let seconds = number("seconds", MAX_SECONDS)?;
        let gain = number("gainDb", 0.0)?;
        if from < 0.0 || seconds <= 0.0 || !(-60.0..=12.0).contains(&gain) {
            return Err(bad("fromSeconds 0 or more, seconds more than 0, gainDb from -60 to 12"));
        }
        if !std::path::Path::new(file).is_file() {
            return Err(failed(format!("{file} isn't there")));
        }
        if let Some(why) = refusal(reaper, proj, after_render) {
            return Err(failed(why));
        }
        let source = reaper.source_from_file(file).ok_or_else(|| failed("the file isn't audio REAPER reads"))?;
        let length = reaper.source_seconds(source).filter(|length| *length > 0.0);
        let Some(length) = length.filter(|_| reaper.source_channels(source) > 0) else {
            reaper.destroy_source(source);
            return Err(failed("the file isn't audio REAPER reads"));
        };
        if from >= length {
            reaper.destroy_source(source);
            return Err(bad(format!("the file is {length:.2} s long: fromSeconds is past its end")));
        }
        let until = (from + seconds.min(MAX_SECONDS)).min(length);
        let (first, mono) = reaper.master_output(proj);
        let out_chan = first | if mono { 1024 } else { 0 };
        let volume = 10f64.powf(gain / 20.0);
        let Some(slot) = Slot::new(source, out_chan, from, volume) else {
            reaper.destroy_source(source);
            return Err(failed("Kumi couldn't make a preview"));
        };
        let slot = Arc::new(slot);
        if !reaper.play_preview(slot.as_ptr()) {
            reaper.destroy_source(source);
            return Err(failed("REAPER wouldn't play the preview (its audio may not be running)"));
        }
        let answer = json!({"previewId": id, "playing": true, "seconds": crate::rows::seconds(until - from), "via": "preview"});
        Ok((Preview { id: id.into(), slot, source, until, proj }, answer))
    }

    /// Whether it should stop now: its end reached, the producer pressed play, or its project's tab was closed (its
    /// pointer isn't asked anything then).
    pub fn over(&self, reaper: &Reaper) -> bool {
        if !reaper.project_valid(self.proj) {
            return true;
        }
        let state = reaper.play_state(self.proj);
        state.playing || state.recording || self.slot.position() >= self.until - 1e-3
    }

    /// Stops it: REAPER lets go of it first, then its source is freed (the slot goes with its last holder).
    pub fn stop(self, reaper: &Reaper) {
        self.slot.silence();
        reaper.stop_preview(self.slot.as_ptr());
        reaper.destroy_source(self.source);
    }
}
