//! The REAPER calls listening makes (M5): the project's render settings, the audio device, media sources, previews played
//! out loud, audio accessors and the hardware hook. Main thread only, as the rest of `Reaper`; the hook's own callback
//! runs on the audio thread and touches nothing here (see `listening::capture`).

use std::ffi::{c_int, c_void, CString};

use super::{Fx, FxOwner, Project, Reaper, Take, Track};
use crate::api::{self, AudioAccessor, PCM_source};

fn cstring(text: &str) -> CString {
    CString::new(text.replace('\0', "")).unwrap_or_default()
}

/// Counts REAPER's dummy audio device as heard (a lab without a device measuring previews); never set for a producer.
const LAB_DUMMY_ENV: &str = "KUMI_REAPER_LAB_DUMMY";

/// Whether this REAPER is a lab's, told to count its dummy device as heard.
pub fn lab_dummy() -> bool {
    std::env::var(LAB_DUMMY_ENV).is_ok_and(|v| v == "1")
}

/// A media source REAPER made or handed out (a take's, or one made from a file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Source(*mut PCM_source);

/// An audio accessor: a track's or a take's audio read on the main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Accessor(*mut AudioAccessor);

impl Source {
    pub fn as_ptr(self) -> *mut c_void {
        self.0.cast()
    }
}

/// A plug-in parameter's steps (`TrackFX_GetParameterStepSizes`): the step, the small and large ones, and whether it is a
/// toggle, in the parameter's own units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Steps {
    pub step: f64,
    pub small: f64,
    pub large: f64,
    pub toggle: bool,
}

impl Reaper<'_> {
    // ----- project settings -----

    /// A project setting by name (`RENDER_SETTINGS`, `DIRTY`…); None when this REAPER lacks the call.
    pub fn project_number(&self, proj: Project, key: &str) -> Option<f64> {
        let key = cstring(key);
        self.api.GetSetProjectInfo().map(|f| unsafe { f(proj.0, key.as_ptr(), 0.0, false) })
    }

    pub fn set_project_number(&self, proj: Project, key: &str, value: f64) -> bool {
        let key = cstring(key);
        match self.api.GetSetProjectInfo() {
            Some(f) => {
                unsafe { f(proj.0, key.as_ptr(), value, true) };
                true
            }
            None => false,
        }
    }

    /// A render setting's text (`RENDER_FILE`, `RENDER_TARGETS`, a file per stem) whole, so a producer's settings are put
    /// back exactly; None when REAPER refuses the key.
    pub fn project_text_long(&self, proj: Project, key: &str) -> Option<String> {
        let f = self.api.GetSetProjectInfo_String()?;
        let key = cstring(key);
        super::objects::big_string(|buf| unsafe { f(proj.0, key.as_ptr(), buf, false) })
    }

    /// The address of a project's own config variable (`projrenderlimit`, the render speed) and its size in bytes.
    fn project_config(&self, proj: Project, name: &str) -> Option<(*mut c_void, i32)> {
        let (offset_of, address_of) = (self.api.projectconfig_var_getoffs()?, self.api.projectconfig_var_addr()?);
        let name = cstring(name);
        let mut size: c_int = 0;
        let offset = unsafe { offset_of(name.as_ptr(), &mut size) };
        if offset <= 0 && size <= 0 {
            return None;
        }
        let address = unsafe { address_of(proj.0, offset) };
        (!address.is_null()).then_some((address, size))
    }

    /// A project config variable that is an int (4 bytes); None when REAPER has none of that name.
    pub fn project_config_int(&self, proj: Project, name: &str) -> Option<i32> {
        let (address, size) = self.project_config(proj, name)?;
        (size == 4).then(|| unsafe { address.cast::<i32>().read_unaligned() })
    }

    pub fn set_project_config_int(&self, proj: Project, name: &str, value: i32) -> bool {
        match self.project_config(proj, name) {
            Some((address, 4)) => {
                unsafe { address.cast::<i32>().write_unaligned(value) };
                true
            }
            _ => false,
        }
    }

    // ----- the audio device -----

    /// Whether REAPER's audio is running (a device open). Thread-safe in REAPER, though only the main thread asks.
    pub fn audio_running(&self) -> bool {
        self.api.Audio_IsRunning().is_some_and(|f| unsafe { f() } != 0)
    }

    /// Opens REAPER's audio devices if they aren't open (what pressing play does): REAPER closes them during a render
    /// (for about 0.2 s after it, measured) and, with its preference, while stopped in the background.
    pub fn audio_init(&self) {
        if let Some(f) = self.api.Audio_Init() {
            unsafe { f() }
        }
    }

    /// Whether REAPER's audio reaches a device the producer hears: running, with outputs, and not REAPER's own dummy device
    /// (which runs the audio engine with nothing behind it, and gives the hardware hook no output channels: measured).
    /// `KUMI_REAPER_LAB_DUMMY=1` counts the dummy device as heard, for measuring previews in a lab with no device.
    pub fn audio_heard(&self) -> bool {
        let dummy = self.audio_device("MODE").is_some_and(|mode| mode.contains("Dummy"));
        self.audio_running() && self.audio_outputs() > 0 && (!dummy || lab_dummy())
    }

    /// What the device is (`MODE`, `IDENT_OUT`, `SRATE`, `BSIZE`); None when REAPER can't say.
    pub fn audio_device(&self, key: &str) -> Option<String> {
        let f = self.api.GetAudioDeviceInfo()?;
        let key = cstring(key);
        let mut ok = false;
        let text = api::read_buffer(|buf, size| ok = unsafe { f(key.as_ptr(), buf, size) });
        ok.then_some(text)
    }

    /// Registers or unregisters the hardware hook (`audio_hook_register_t`, which must stay where it is meanwhile).
    pub fn register_audio_hook(&self, add: bool, hook: *mut c_void) -> bool {
        self.api.Audio_RegHardwareHook().is_some_and(|f| unsafe { f(add, hook.cast()) } > 0)
    }

    /// The time of the next block the audio thread processes, in the project's seconds.
    pub fn next_block_position(&self, proj: Project) -> f64 {
        self.play_position(proj)
    }

    /// How fast the project plays (1 at normal speed): its position moves this many seconds per second.
    pub fn play_rate(&self, proj: Project) -> f64 {
        self.api.Master_GetPlayRate().map(|f| unsafe { f(proj.0) }).filter(|rate| rate.is_finite() && *rate > 0.0).unwrap_or(1.0)
    }

    /// Whether an action is on (a toggle: the metronome, repeat).
    pub fn action_on(&self, command: i32) -> bool {
        self.api.GetToggleCommandState().is_some_and(|f| unsafe { f(command) } == 1)
    }

    // ----- media sources -----

    /// A source made from a media file; None when REAPER can't read it.
    pub fn source_from_file(&self, path: &str) -> Option<Source> {
        let f = self.api.PCM_Source_CreateFromFile()?;
        let path = cstring(path);
        let source = unsafe { f(path.as_ptr()) };
        (!source.is_null()).then_some(Source(source))
    }

    /// Frees a source this bridge made (never a take's own).
    pub fn destroy_source(&self, source: Source) {
        if let Some(f) = self.api.PCM_Source_Destroy() {
            unsafe { f(source.0) }
        }
    }

    /// A take's source; None for a take without one (an empty take).
    pub fn take_source(&self, take: Take) -> Option<Source> {
        let source = unsafe { self.api.GetMediaItemTake_Source()?(take.0) };
        (!source.is_null()).then_some(Source(source))
    }

    /// The source a section or reverse source wraps; None for a plain file's.
    pub fn source_parent(&self, source: Source) -> Option<Source> {
        let parent = unsafe { self.api.GetMediaSourceParent()?(source.0) };
        (!parent.is_null() && parent != source.0).then_some(Source(parent))
    }

    /// The source's length in seconds; None when it is counted in quarter notes (MIDI) or REAPER can't say.
    pub fn source_seconds(&self, source: Source) -> Option<f64> {
        let f = self.api.GetMediaSourceLength()?;
        let mut in_beats = false;
        let length = unsafe { f(source.0, &mut in_beats) };
        (!in_beats && length.is_finite()).then_some(length)
    }

    pub fn source_channels(&self, source: Source) -> i32 {
        self.api.GetMediaSourceNumChannels().map(|f| unsafe { f(source.0) }).unwrap_or(0)
    }

    pub fn source_rate(&self, source: Source) -> i32 {
        self.api.GetMediaSourceSampleRate().map(|f| unsafe { f(source.0) }).unwrap_or(0)
    }

    /// The file the source plays (a section's is its parent's file); empty for one without a file.
    pub fn source_file(&self, source: Source) -> String {
        match self.api.GetMediaSourceFileName() {
            Some(f) => api::read_buffer(|buf, size| unsafe { f(source.0, buf, size) }),
            None => String::new(),
        }
    }

    /// The source's type as REAPER names it ("WAVE", "MIDI", "SECTION"…).
    pub fn source_type(&self, source: Source) -> String {
        match self.api.GetMediaSourceType() {
            Some(f) => api::read_buffer(|buf, size| unsafe { f(source.0, buf, size) }),
            None => String::new(),
        }
    }

    /// A section source's part of its parent: (offset, length, reversed), in seconds.
    pub fn source_section(&self, source: Source) -> Option<(f64, f64, bool)> {
        let f = self.api.PCM_Source_GetSectionInfo()?;
        let (mut offset, mut length, mut reversed) = (0.0, 0.0, false);
        unsafe { f(source.0, &mut offset, &mut length, &mut reversed) }.then_some((offset, length, reversed))
    }

    // ----- previews played out loud -----

    /// Plays a `preview_register_t` out loud at once (its source buffered); false when REAPER refused.
    pub fn play_preview(&self, preview: *mut c_void) -> bool {
        self.api.PlayPreviewEx().is_some_and(|f| unsafe { f(preview.cast(), 1, -1.0) } != 0)
    }

    pub fn stop_preview(&self, preview: *mut c_void) -> bool {
        self.api.StopPreview().is_some_and(|f| unsafe { f(preview.cast()) } != 0)
    }

    // ----- audio accessors -----

    /// An accessor of the track's audio before its FX (its items, mixed); None when REAPER makes none.
    pub fn track_accessor(&self, track: Track) -> Option<Accessor> {
        let accessor = unsafe { self.api.CreateTrackAudioAccessor()?(track.0) };
        (!accessor.is_null()).then_some(Accessor(accessor))
    }

    /// Interleaved samples from `start` (seconds): 1 when there was audio, 0 when none, -1 on an error.
    pub fn accessor_samples(&self, accessor: Accessor, rate: i32, channels: i32, start: f64, buf: &mut [f64]) -> i32 {
        let frames = (buf.len() / channels.max(1) as usize) as c_int;
        match self.api.GetAudioAccessorSamples() {
            Some(f) => unsafe { f(accessor.0, rate, channels, start, frames, buf.as_mut_ptr()) },
            None => -1,
        }
    }

    /// Whether what the accessor reads changed since it was made (an item moved, a take changed).
    pub fn accessor_changed(&self, accessor: Accessor) -> bool {
        self.api.AudioAccessorStateChanged().is_some_and(|f| unsafe { f(accessor.0) })
    }

    pub fn destroy_accessor(&self, accessor: Accessor) {
        if let Some(f) = self.api.DestroyAudioAccessor() {
            unsafe { f(accessor.0) }
        }
    }

    // ----- plug-in parameters across their range -----

    /// What the plug-in would print for a normalized value, without setting it (a track's FX or a take's).
    pub fn format_param(&self, fx: Fx, param: i32, value: f64) -> Option<String> {
        let mut ok = false;
        let text = match fx.owner {
            FxOwner::Track(track) => {
                let f = self.api.TrackFX_FormatParamValueNormalized()?;
                api::read_buffer(|buf, size| ok = unsafe { f(track.0, fx.index, param, value, buf, size) })
            }
            FxOwner::Take(take) => {
                let f = self.api.TakeFX_FormatParamValueNormalized()?;
                api::read_buffer(|buf, size| ok = unsafe { f(take.0, fx.index, param, value, buf, size) })
            }
        };
        ok.then_some(text)
    }

    /// A parameter's steps; None for a continuous one.
    pub fn param_steps(&self, fx: Fx, param: i32) -> Option<Steps> {
        let (mut step, mut small, mut large, mut toggle) = (0.0, 0.0, 0.0, false);
        let stepped = match fx.owner {
            FxOwner::Track(track) => {
                let f = self.api.TrackFX_GetParameterStepSizes()?;
                unsafe { f(track.0, fx.index, param, &mut step, &mut small, &mut large, &mut toggle) }
            }
            FxOwner::Take(take) => {
                let f = self.api.TakeFX_GetParameterStepSizes()?;
                unsafe { f(take.0, fx.index, param, &mut step, &mut small, &mut large, &mut toggle) }
            }
        };
        stepped.then_some(Steps { step, small, large, toggle })
    }

    /// The master's first hardware output: its first channel, and whether it is mono (where the producer listens).
    pub fn master_output(&self, proj: Project) -> (i32, bool) {
        let Some(master) = self.master_track(proj) else { return (0, false) };
        if self.send_count(master, 1) <= 0 {
            return (0, false);
        }
        let dest = self.send_value(master, 1, 0, "I_DSTCHAN") as i32;
        (dest & 1023, dest & 1024 != 0)
    }
}
