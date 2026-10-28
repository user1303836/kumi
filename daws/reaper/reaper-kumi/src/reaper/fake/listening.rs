//! The fake REAPER's listening side (M5, m5-reaper): the project's render settings and a render (action 42230) that
//! writes the WAV files REAPER would, as measured in REAPER 7.82; the audio device, media sources and previews; audio
//! accessors; the hardware hook; parameter steps; chunks that keep the GUIDs written into them; tracks' `P_EXT`.
//!
//! `fake::lookup` asks this module first. Where another part of the fake (another stream's, or the M3 fake's own) gives a
//! function too, the one here answers only for what is listening's and passes every other call on to that part (`next`),
//! so the fakes compose in one lookup whatever order the others are asked in.

use super::*;
use std::cell::Cell;

/// What the fake renders, as each render saw the project when it began.
#[derive(Clone, Debug)]
pub struct RenderSeen {
    pub settings: i32,
    /// The selected tracks' GUIDs, "master" first when it was selected.
    pub selected: Vec<String>,
    pub master_fx: bool,
    pub speed: i32,
    pub start: f64,
    pub end: f64,
    pub rate: f64,
    pub targets: Vec<String>,
    pub format: String,
    pub normalize: f64,
    /// The muted tracks' GUIDs.
    pub muted: Vec<String>,
}

/// A source made from a file (`PCM_Source_CreateFromFile`).
#[derive(Clone, Debug)]
pub struct FakeSource {
    pub path: String,
    pub seconds: f64,
    pub channels: i32,
    pub rate: i32,
    pub destroyed: bool,
}

/// The file a take plays (keyed by the take's GUID), and the section of it when the take's source is a section.
#[derive(Clone, Debug)]
pub struct FakeMedia {
    pub path: String,
    pub seconds: f64,
    pub channels: i32,
    pub rate: i32,
    pub section: Option<(f64, f64, bool)>,
}

pub struct FakeListening {
    pub numbers: HashMap<String, f64>,
    pub texts: HashMap<String, String>,
    pub refuse_text_once: Option<(String, bool)>,
    /// `projrenderlimit` (the render speed), where REAPER's projectconfig_var_addr points.
    pub speed: Box<i32>,
    pub audio: bool,
    /// `Audio_Init` opens the device (false: there is none to open), and how often it was asked.
    pub device_opens: bool,
    pub inits: u32,
    /// The device REAPER says it runs on (`MODE`), and its output channels.
    pub device: String,
    pub outputs: i32,
    pub device_rate: u32,
    pub metronome: bool,
    pub hooks: Vec<usize>,
    pub hook_refuses: bool,
    pub sources: Vec<FakeSource>,
    pub take_files: HashMap<String, FakeMedia>,
    pub previews: Vec<usize>,
    pub preview_refuses: bool,
    pub accessors: Vec<(String, bool)>,
    pub accessor_changed: bool,
    pub renders: Vec<RenderSeen>,
    /// The next render writes nothing (a render that failed in REAPER).
    pub fail_render: bool,
    /// REAPER lists a file more than the settings ask (a preference Kumi didn't foresee).
    pub odd_targets: bool,
    /// The next render stops half way and keeps what it wrote (the producer cancelled it in REAPER's window).
    pub short_render: bool,
    /// This REAPER has no `projrenderlimit` to read.
    pub no_speed: bool,
    /// Called mid-render with no borrow held: REAPER's timer firing inside its render loop.
    pub during_render: Option<fn()>,
    /// The render window has a Cancel to press, and how often it was pressed.
    pub cancel_button: bool,
    pub cancel_presses: u32,
    cancelled_render: bool,
    /// Setting a render key marks the project changed (as some REAPER might).
    pub dirty_on_set: bool,
    /// Chunks carry their items' and takes' GUIDs, and setting one gives the track the GUIDs written in it.
    pub chunk_guids: bool,
    /// `PCM_Source_CreateFromFile` makes listening's own sources (the file read as a WAV), as listening's tests want;
    /// otherwise the call goes on to the rest of the fake.
    pub sources_from_files: bool,
    pub track_ext: HashMap<(String, String), String>,
}

impl Default for FakeListening {
    fn default() -> Self {
        let mut numbers = HashMap::new();
        // A new project's render settings, as REAPER 7.82 has them.
        for (key, value) in [
            ("RENDER_SETTINGS", 0.0),
            ("RENDER_BOUNDSFLAG", 1.0),
            ("RENDER_CHANNELS", 2.0),
            ("RENDER_SRATE", 0.0),
            ("RENDER_STARTPOS", 0.0),
            ("RENDER_ENDPOS", 0.0),
            ("RENDER_TAILFLAG", 0.0),
            ("RENDER_TAILMS", 1000.0),
            ("RENDER_ADDTOPROJ", 0.0),
            ("RENDER_DITHER", 0.0),
            ("RENDER_NORMALIZE", 0.0),
            ("RENDER_NORMALIZE_TARGET", 0.0),
            ("RENDER_BRICKWALL", 0.0),
            ("RENDER_FADEIN", 0.0),
            ("RENDER_FADEOUT", 0.0),
            ("RENDER_FADEINSHAPE", 1.0),
            ("RENDER_FADEOUTSHAPE", 1.0),
            ("RENDER_FADELPF", 0.0),
            ("RENDER_PADSTART", 0.0),
            ("RENDER_PADEND", 0.0),
            ("RENDER_TRIMSTART", 1e-6),
            ("RENDER_TRIMEND", 1e-6),
            ("RENDER_DELAY", 0.0),
            ("PROJECT_SRATE", 44100.0),
            ("PROJECT_SRATE_USE", 0.0),
        ] {
            numbers.insert(key.to_string(), value);
        }
        let mut texts = HashMap::new();
        for (key, value) in [
            ("RENDER_FILE", ""),
            ("RENDER_PATTERN", ""),
            ("RENDER_FORMAT", "ZXZhdxgAAQ=="),
            ("RENDER_FORMAT2", ""),
            ("RENDER_EXTRAFILEDIR", ""),
        ] {
            texts.insert(key.to_string(), value.to_string());
        }
        FakeListening {
            numbers,
            texts,
            refuse_text_once: None,
            speed: Box::new(0),
            audio: true,
            device_opens: false,
            inits: 0,
            device: "CoreAudio Built-in Output".into(),
            outputs: 2,
            device_rate: 44100,
            metronome: false,
            hooks: vec![],
            hook_refuses: false,
            sources: vec![],
            take_files: HashMap::new(),
            previews: vec![],
            preview_refuses: false,
            accessors: vec![],
            accessor_changed: false,
            renders: vec![],
            fail_render: false,
            odd_targets: false,
            short_render: false,
            no_speed: false,
            during_render: None,
            cancel_button: false,
            cancel_presses: 0,
            cancelled_render: false,
            dirty_on_set: false,
            chunk_guids: false,
            sources_from_files: false,
            track_ext: HashMap::new(),
        }
    }
}

impl Clone for FakeListening {
    fn clone(&self) -> Self {
        FakeListening {
            numbers: self.numbers.clone(),
            texts: self.texts.clone(),
            refuse_text_once: self.refuse_text_once.clone(),
            speed: Box::new(*self.speed),
            audio: self.audio,
            device_opens: self.device_opens,
            inits: self.inits,
            device: self.device.clone(),
            outputs: self.outputs,
            device_rate: self.device_rate,
            metronome: self.metronome,
            hooks: self.hooks.clone(),
            hook_refuses: self.hook_refuses,
            sources: self.sources.clone(),
            take_files: self.take_files.clone(),
            previews: self.previews.clone(),
            preview_refuses: self.preview_refuses,
            accessors: self.accessors.clone(),
            accessor_changed: self.accessor_changed,
            renders: self.renders.clone(),
            fail_render: self.fail_render,
            odd_targets: self.odd_targets,
            short_render: self.short_render,
            no_speed: self.no_speed,
            during_render: self.during_render,
            cancel_button: self.cancel_button,
            cancel_presses: self.cancel_presses,
            cancelled_render: self.cancelled_render,
            dirty_on_set: self.dirty_on_set,
            chunk_guids: self.chunk_guids,
            sources_from_files: self.sources_from_files,
            track_ext: self.track_ext.clone(),
        }
    }
}

// Handle kinds of listening's own, apart from the M3 fake's and the other streams' kinds.
const FILE_SOURCE: u64 = 0xA0;
const SECTION_PARENT: u64 = 0xA1;
const ACCESSOR: u64 = 0xA2;
const SPEED_OFFSET: c_int = 0x10;

// ----- the rest of the fake -----

thread_local! {
    /// Set while the rest of the fake is asked for a function this module gives too, so that the ask passes it by.
    static PASSING: Cell<bool> = const { Cell::new(false) };
}

/// The function the rest of the fake gives for `name` (another stream's, or the M3 fake's own), as `fake::lookup` finds
/// it past this module; null when there is none.
fn next(name: &CStr) -> *mut c_void {
    struct Passed;
    impl Drop for Passed {
        fn drop(&mut self) {
            PASSING.with(|p| p.set(false));
        }
    }
    PASSING.with(|p| p.set(true));
    let _passed = Passed;
    super::lookup(name)
}

/// `next` as a function of type `$ty`; None when the rest of the fake has none.
macro_rules! next_fn {
    ($name:literal as $ty:ty) => {{
        let f = next($name);
        // SAFETY: the fake's lookup gives each name the function with REAPER's signature for it.
        (!f.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, $ty>(f) })
    }};
}

// ----- render settings -----

unsafe extern "C" fn get_set_project_info(proj: *mut ReaProject, desc: *const c_char, value: f64, is_set: bool) -> f64 {
    let key = name_of(desc);
    // The render settings and DIRTY are listening's; any other key is the rest of the fake's (kept here when it has none).
    if key != "DIRTY" && !key.starts_with("RENDER_") {
        if let Some(f) = next_fn!(c"GetSetProjectInfo" as unsafe extern "C" fn(*mut ReaProject, *const c_char, f64, bool) -> f64) {
            return unsafe { f(proj, desc, value, is_set) };
        }
    }
    counted(|s| {
        if key == "DIRTY" {
            if is_set {
                s.dirty = value != 0.0;
            }
            return if s.dirty { 1.0 } else { 0.0 };
        }
        if is_set {
            if s.listening.dirty_on_set && key.starts_with("RENDER_") {
                s.dirty = true;
            }
            s.listening.numbers.insert(key.clone(), value);
            return value;
        }
        s.listening.numbers.get(&key).copied().unwrap_or(0.0)
    })
}

/// The files REAPER would write with the settings as they are, as 7.82 lists them: the master first (`$tracknumber`
/// "00"), then each selected track (its number, two digits at least); source 2 alone lists none. (#401) Over every region
/// (bounds 3), a file a region (`$region` its name), or with the region render matrix (source 8) a file for each track a region
/// renders (`$track` its name, "Master" for the master), through the master (8|128) one for each region whose row has a track;
/// `$project` is "untitled" in the fake.
fn targets(s: &State) -> Option<Vec<String>> {
    let settings = s.listening.numbers.get("RENDER_SETTINGS").copied().unwrap_or(0.0) as i32;
    let bounds = s.listening.numbers.get("RENDER_BOUNDSFLAG").copied().unwrap_or(0.0) as i32;
    let folder = s.listening.texts.get("RENDER_FILE").cloned().unwrap_or_default();
    let pattern = s.listening.texts.get("RENDER_PATTERN").cloned().unwrap_or_default().replace("$project", "untitled");
    if bounds == 3 {
        let mut regions: Vec<&super::FakeMarker> = s.markers.iter().filter(|m| m.is_region).collect();
        regions.sort_by(|a, b| a.start.total_cmp(&b.start));
        let track_name = |id: &str| match id {
            "master" => "Master".to_string(),
            guid => s.tracks.iter().find(|t| t.guid == guid).map(|t| t.name.clone()).unwrap_or_default(),
        };
        let mut list = Vec::new();
        for region in regions {
            let named = pattern.replace("$region", &region.name);
            let row = s.render_matrix.get(&region.number).cloned().unwrap_or_default();
            if settings & (8 | 128) == 8 | 128 {
                if !row.is_empty() {
                    list.push(format!("{folder}/{}.wav", named.replace("$track", "Master")));
                }
            } else if settings & 8 != 0 {
                for id in row {
                    list.push(format!("{folder}/{}.wav", named.replace("$track", &track_name(&id))));
                }
            } else {
                list.push(format!("{folder}/{}.wav", named.replace("$track", "Master")));
            }
        }
        return Some(list);
    }
    let file = |number: &str| format!("{folder}/{}.wav", pattern.replace("$tracknumber", number));
    let selected: Vec<String> =
        s.tracks.iter().enumerate().filter(|(_, t)| t.selected).map(|(n, _)| file(&format!("{:02}", n + 1))).collect();
    let mut list = match settings & 3 {
        0 => vec![file("00")],
        1 => std::iter::once(file("00")).chain(selected).collect(),
        3 => selected,
        _ => return None,
    };
    if s.listening.odd_targets {
        list.push(file("odd"));
    }
    Some(list)
}

unsafe extern "C" fn get_set_project_info_string(proj: *mut ReaProject, desc: *const c_char, buf: *mut c_char, is_set: bool) -> bool {
    let key = name_of(desc);
    if !key.starts_with("RENDER_") {
        if let Some(f) =
            next_fn!(c"GetSetProjectInfo_String" as unsafe extern "C" fn(*mut ReaProject, *const c_char, *mut c_char, bool) -> bool)
        {
            return unsafe { f(proj, desc, buf, is_set) };
        }
    }
    if is_set {
        let value = name_of(buf);
        return counted(|s| {
            if s.listening.refuse_text_once.as_ref().is_some_and(|(refused, _)| refused == &key) {
                return s.listening.refuse_text_once.take().unwrap().1;
            }
            if s.listening.dirty_on_set && key.starts_with("RENDER_") {
                s.dirty = true;
            }
            s.listening.texts.insert(key, value);
            true
        });
    }
    counted(|s| {
        let value = match key.as_str() {
            "RENDER_TARGETS" => match targets(s) {
                Some(list) => list.join(";"),
                None => return false,
            },
            other => match s.listening.texts.get(other) {
                Some(value) => value.clone(),
                None => return false,
            },
        };
        write(buf, 1 << 20, &value);
        true
    })
}

unsafe extern "C" fn projectconfig_var_getoffs(name: *const c_char, size: *mut c_int) -> c_int {
    let known = name_of(name) == "projrenderlimit" && !with(|s| s.listening.no_speed);
    unsafe { *size = if known { 4 } else { 0 } };
    if known {
        SPEED_OFFSET
    } else {
        0
    }
}

unsafe extern "C" fn projectconfig_var_addr(_proj: *mut ReaProject, index: c_int) -> *mut c_void {
    if index != SPEED_OFFSET {
        return ptr::null_mut();
    }
    with(|s| (&mut *s.listening.speed as *mut i32).cast())
}

/// A float WAV of `frames` frames of `value`, stereo, as REAPER writes a render (a `bext` chunk before the data).
fn float_wav(rate: u32, frames: u64, value: f32) -> Vec<u8> {
    let data = frames * 8;
    let mut bytes = Vec::with_capacity(data as usize + 80);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&((data + 20 + 24 + 8 + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEbext");
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    for v in [3u16, 2] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 8).to_le_bytes());
    for v in [8u16, 32] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data as u32).to_le_bytes());
    for _ in 0..frames * 2 {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// REAPER's render with the most recent settings (42230), as a test's `on_action`: it notes what it saw, stops playback,
/// writes half of each file, lets a test's tick run as REAPER's timer would inside its render loop, then finishes the files
/// (unless the render window's Cancel was pressed meanwhile, or the render is set to fail).
pub fn render_action(command: i32) {
    if command != crate::listening::render::RENDER_WITH_LAST {
        return;
    }
    let prepared = with(|s| {
        let targets = targets(s).unwrap_or_default();
        let number = |key: &str| s.listening.numbers.get(key).copied().unwrap_or(0.0);
        let rate = match number("RENDER_SRATE") {
            r if r > 0.0 => r,
            _ => number("PROJECT_SRATE"),
        };
        let mut selected: Vec<String> = if s.master.selected { vec!["master".into()] } else { vec![] };
        selected.extend(s.tracks.iter().filter(|t| t.selected).map(|t| t.guid.clone()));
        let seen = RenderSeen {
            settings: number("RENDER_SETTINGS") as i32,
            selected,
            master_fx: s.master.extra.get("I_FXEN").copied().unwrap_or(1.0) != 0.0,
            speed: *s.listening.speed,
            start: number("RENDER_STARTPOS"),
            end: number("RENDER_ENDPOS"),
            rate,
            targets: targets.clone(),
            format: s.listening.texts.get("RENDER_FORMAT").cloned().unwrap_or_default(),
            normalize: number("RENDER_NORMALIZE"),
            muted: s.tracks.iter().filter(|t| t.mute).map(|t| t.guid.clone()).collect(),
        };
        s.listening.renders.push(seen.clone());
        s.listening.cancelled_render = false;
        // A render stops playback (measured).
        s.play_bits &= !1;
        let fail = std::mem::take(&mut s.listening.fail_render);
        (targets, seen, fail, s.listening.during_render)
    });
    let (targets, seen, fail, during) = prepared;
    if fail {
        return;
    }
    // (a render of regions is the regions' length: a second, in the fake)
    let frames = match ((seen.end - seen.start) * seen.rate).round().max(0.0) as u64 {
        0 => seen.rate as u64,
        frames => frames,
    };
    for target in &targets {
        let _ = std::fs::write(target, float_wav(seen.rate as u32, frames / 2, 0.25));
    }
    if let Some(during) = during {
        during();
    }
    if with(|s| std::mem::take(&mut s.listening.cancelled_render) || std::mem::take(&mut s.listening.short_render)) {
        return;
    }
    for target in &targets {
        let _ = std::fs::write(target, float_wav(seen.rate as u32, frames, 0.25));
    }
}

/// The render window's Cancel, pressed: true when it has one (the render then stops where it is).
pub fn press_render_cancel() -> bool {
    with(|s| {
        s.listening.cancel_presses += 1;
        if s.listening.cancel_button {
            s.listening.cancelled_render = true;
        }
        s.listening.cancel_button
    })
}

// ----- the audio device -----

unsafe extern "C" fn audio_is_running() -> c_int {
    with(|s| s.listening.audio as c_int)
}

unsafe extern "C" fn get_audio_device_info(attribute: *const c_char, buf: *mut c_char, size: c_int) -> bool {
    let attribute = name_of(attribute);
    with(|s| {
        if !s.listening.audio {
            return false;
        }
        let value = match attribute.as_str() {
            "MODE" => s.listening.device.clone(),
            "SRATE" => s.listening.device_rate.to_string(),
            "BSIZE" => "512".to_string(),
            _ => return false,
        };
        write(buf, size, &value);
        true
    })
}

unsafe extern "C" fn audio_reg_hardware_hook(add: bool, reg: *mut api::audio_hook_register_t) -> c_int {
    with(|s| {
        if add {
            if s.listening.hook_refuses {
                return 0;
            }
            s.listening.hooks.push(reg as usize);
        } else {
            s.listening.hooks.retain(|h| *h != reg as usize);
        }
        1
    })
}

unsafe extern "C" fn audio_init() {
    with(|s| {
        s.listening.inits += 1;
        if s.listening.device_opens {
            s.listening.audio = true;
        }
    })
}

unsafe extern "C" fn get_num_audio_outputs() -> c_int {
    // No device, no outputs; with one, as many as the rest of the fake lists (when it lists them).
    if !with(|s| s.listening.audio) {
        return 0;
    }
    match next_fn!(c"GetNumAudioOutputs" as unsafe extern "C" fn() -> c_int) {
        Some(f) => unsafe { f() },
        None => with(|s| s.listening.outputs),
    }
}

unsafe extern "C" fn master_get_play_rate(_proj: *mut ReaProject) -> f64 {
    1.0
}

unsafe extern "C" fn get_toggle_command_state(command: c_int) -> c_int {
    if command == 40364 {
        return with(|s| s.listening.metronome as c_int);
    }
    next_fn!(c"GetToggleCommandState" as unsafe extern "C" fn(c_int) -> c_int).map_or(-1, |f| unsafe { f(command) })
}

/// `audio_hook_register_t` as REAPER holds what it was given.
#[repr(C)]
struct HookLayout {
    on_audio_buffer: extern "C" fn(bool, c_int, f64, *mut HookLayout),
    userdata1: *mut c_void,
    userdata2: *mut c_void,
    input_nch: c_int,
    output_nch: c_int,
    get_buffer: Option<unsafe extern "C" fn(bool, c_int) -> *mut f64>,
}

thread_local! {
    static OUTPUTS: RefCell<Vec<Vec<f64>>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn get_output_buffer(is_output: bool, index: c_int) -> *mut f64 {
    if !is_output {
        return ptr::null_mut();
    }
    OUTPUTS.with(|o| o.borrow_mut().get_mut(index as usize).map_or(ptr::null_mut(), |b| b.as_mut_ptr()))
}

/// REAPER's audio thread processing a block: each registered hook gets `len` frames of `channels` outputs, channel n
/// holding `value(n, i)`; the play position moves on by the block while playing.
pub fn audio_block(len: usize, channels: usize, value: impl Fn(usize, usize) -> f64) {
    let (hooks, rate) = with(|s| (s.listening.hooks.clone(), s.listening.device_rate as f64));
    OUTPUTS.with(|o| *o.borrow_mut() = (0..channels).map(|n| (0..len).map(|i| value(n, i)).collect()).collect());
    for hook in hooks {
        let reg = hook as *mut HookLayout;
        unsafe {
            (*reg).output_nch = channels as c_int;
            (*reg).get_buffer = Some(get_output_buffer);
            ((*reg).on_audio_buffer)(false, len as c_int, rate, reg);
            ((*reg).on_audio_buffer)(true, len as c_int, rate, reg);
        }
    }
    with(|s| {
        if s.play_bits & 1 != 0 {
            s.play_position += len as f64 / rate;
        }
    });
}

// ----- media sources -----

fn source_kind(handle: u64) -> u64 {
    unpack(handle).0
}

/// The media behind a take's source handle.
fn take_media(s: &State, handle: u64) -> Option<FakeMedia> {
    let (kind, a, b, c) = unpack(handle);
    let take = s.tracks.get(a as usize)?.items.get(b as usize)?.takes.get(c as usize)?;
    let _ = kind;
    s.listening.take_files.get(&take.guid).cloned()
}

unsafe extern "C" fn pcm_source_create_from_file(path: *const c_char) -> *mut api::PCM_source {
    if !with(|s| s.listening.sources_from_files) {
        return next_fn!(c"PCM_Source_CreateFromFile" as unsafe extern "C" fn(*const c_char) -> *mut api::PCM_source)
            .map_or(ptr::null_mut(), |f| unsafe { f(path) });
    }
    let path = name_of(path);
    let Ok(info) = crate::listening::wav::read_info(std::path::Path::new(&path)) else { return ptr::null_mut() };
    with(|s| {
        s.listening.sources.push(FakeSource {
            path,
            seconds: info.seconds(),
            channels: info.channels as i32,
            rate: info.rate as i32,
            destroyed: false,
        });
        pack(FILE_SOURCE, (s.listening.sources.len() - 1) as u64, 0, 0) as *mut api::PCM_source
    })
}

unsafe extern "C" fn pcm_source_destroy(source: *mut api::PCM_source) {
    let (kind, index, _, _) = unpack(source as u64);
    if kind != FILE_SOURCE {
        if let Some(f) = next_fn!(c"PCM_Source_Destroy" as unsafe extern "C" fn(*mut api::PCM_source)) {
            unsafe { f(source) };
        }
        return;
    }
    with(|s| {
        if let Some(source) = s.listening.sources.get_mut(index as usize) {
            source.destroyed = true;
        }
    })
}

unsafe extern "C" fn get_media_source_type(source: *mut api::PCM_source, buf: *mut c_char, size: c_int) {
    let handle = source as u64;
    match source_kind(handle) {
        FILE_SOURCE | SECTION_PARENT => write(buf, size, "WAVE"),
        _ => {
            let section = with(|s| take_media(s, handle).is_some_and(|m| m.section.is_some()));
            if section {
                write(buf, size, "SECTION");
            } else if let Some(f) = next_fn!(c"GetMediaSourceType" as unsafe extern "C" fn(*mut api::PCM_source, *mut c_char, c_int)) {
                unsafe { f(source, buf, size) }
            }
        }
    }
}

unsafe extern "C" fn get_media_source_parent(source: *mut api::PCM_source) -> *mut api::PCM_source {
    let handle = source as u64;
    let mine = matches!(source_kind(handle), FILE_SOURCE | SECTION_PARENT)
        || with(|s| source_kind(handle) == SOURCE && take_media(s, handle).is_some());
    if mine {
        return match with(|s| take_media(s, handle).is_some_and(|m| m.section.is_some())) {
            true if source_kind(handle) == SOURCE => {
                let (_, a, b, c) = unpack(handle);
                pack(SECTION_PARENT, a, b, c) as *mut api::PCM_source
            }
            _ => ptr::null_mut(),
        };
    }
    next_fn!(c"GetMediaSourceParent" as unsafe extern "C" fn(*mut api::PCM_source) -> *mut api::PCM_source)
        .map_or(ptr::null_mut(), |f| unsafe { f(source) })
}

unsafe extern "C" fn pcm_source_get_section_info(
    source: *mut api::PCM_source,
    offset: *mut f64,
    length: *mut f64,
    reversed: *mut bool,
) -> bool {
    let handle = source as u64;
    let mine = matches!(source_kind(handle), FILE_SOURCE | SECTION_PARENT)
        || with(|s| source_kind(handle) == SOURCE && take_media(s, handle).is_some());
    if !mine {
        return next_fn!(c"PCM_Source_GetSectionInfo" as unsafe extern "C" fn(*mut api::PCM_source, *mut f64, *mut f64, *mut bool) -> bool)
            .is_some_and(|f| unsafe { f(source, offset, length, reversed) });
    }
    with(|s| match take_media(s, handle).and_then(|m| m.section).filter(|_| source_kind(handle) == SOURCE) {
        Some((o, l, r)) => {
            unsafe {
                *offset = o;
                *length = l;
                *reversed = r;
            }
            true
        }
        None => false,
    })
}

/// (file, seconds, channels, rate) of any source handle the fake hands out.
fn media_of(s: &State, handle: u64) -> Option<(String, f64, i32, i32)> {
    match source_kind(handle) {
        FILE_SOURCE => s.listening.sources.get(unpack(handle).1 as usize).map(|m| (m.path.clone(), m.seconds, m.channels, m.rate)),
        SECTION_PARENT => {
            let (_, a, b, c) = unpack(handle);
            take_media(s, pack(SOURCE, a, b, c)).map(|m| (m.path, m.seconds, m.channels, m.rate))
        }
        _ => take_media(s, handle).map(|m| {
            let seconds = m.section.map_or(m.seconds, |(_, length, _)| length);
            (m.path, seconds, m.channels, m.rate)
        }),
    }
}

unsafe extern "C" fn get_media_source_length(source: *mut api::PCM_source, in_beats: *mut bool) -> f64 {
    let (midi, media) = with(|s| {
        let midi = take_of(s, source as u64).is_some_and(|t| t.source.as_deref() == Some("MIDI")) && source_kind(source as u64) == SOURCE;
        (midi, media_of(s, source as u64))
    });
    if media.is_none() {
        if let Some(f) = next_fn!(c"GetMediaSourceLength" as unsafe extern "C" fn(*mut api::PCM_source, *mut bool) -> f64) {
            return unsafe { f(source, in_beats) };
        }
    }
    if !in_beats.is_null() {
        unsafe { *in_beats = midi };
    }
    media.map_or(0.0, |m| m.1)
}

unsafe extern "C" fn get_media_source_num_channels(source: *mut api::PCM_source) -> c_int {
    match with(|s| media_of(s, source as u64)) {
        Some(m) => m.2,
        None => next_fn!(c"GetMediaSourceNumChannels" as unsafe extern "C" fn(*mut api::PCM_source) -> c_int)
            .map_or(0, |f| unsafe { f(source) }),
    }
}

unsafe extern "C" fn get_media_source_sample_rate(source: *mut api::PCM_source) -> c_int {
    match with(|s| media_of(s, source as u64)) {
        Some(m) => m.3,
        None => {
            next_fn!(c"GetMediaSourceSampleRate" as unsafe extern "C" fn(*mut api::PCM_source) -> c_int).map_or(0, |f| unsafe { f(source) })
        }
    }
}

unsafe extern "C" fn get_media_source_file_name(source: *mut api::PCM_source, buf: *mut c_char, size: c_int) {
    match with(|s| media_of(s, source as u64)) {
        Some(m) => write(buf, size, &m.0),
        None => match next_fn!(c"GetMediaSourceFileName" as unsafe extern "C" fn(*mut api::PCM_source, *mut c_char, c_int)) {
            Some(f) => unsafe { f(source, buf, size) },
            None => write(buf, size, ""),
        },
    }
}

// ----- previews -----

unsafe extern "C" fn play_preview_ex(preview: *mut api::preview_register_t, _flags: c_int, _align: f64) -> c_int {
    with(|s| {
        if !s.listening.audio || s.listening.preview_refuses {
            return 0;
        }
        s.listening.previews.push(preview as usize);
        1
    })
}

unsafe extern "C" fn stop_preview(preview: *mut api::preview_register_t) -> c_int {
    with(|s| {
        let before = s.listening.previews.len();
        s.listening.previews.retain(|p| *p != preview as usize);
        (before != s.listening.previews.len()) as c_int
    })
}

// ----- audio accessors -----

unsafe extern "C" fn create_track_audio_accessor(track: *mut MediaTrack) -> *mut api::AudioAccessor {
    with(|s| {
        let Some(guid) = track_of(s, track as u64).map(|t| t.guid.clone()) else { return ptr::null_mut() };
        s.listening.accessors.push((guid, false));
        pack(ACCESSOR, (s.listening.accessors.len() - 1) as u64, 0, 0) as *mut api::AudioAccessor
    })
}

/// The track's audio before its FX: 0.25 wherever an audio item plays, silence elsewhere.
unsafe extern "C" fn get_audio_accessor_samples(
    accessor: *mut api::AudioAccessor,
    rate: c_int,
    channels: c_int,
    start: f64,
    frames: c_int,
    buf: *mut f64,
) -> c_int {
    let spans: Option<Vec<(f64, f64)>> = with(|s| {
        let (guid, destroyed) = s.listening.accessors.get(unpack(accessor as u64).1 as usize)?.clone();
        if destroyed {
            return None;
        }
        let track = s.tracks.iter().find(|t| t.guid == guid)?;
        Some(
            track
                .items
                .iter()
                .filter(|i| !i.mute && i.takes.get(i.active).is_some_and(|t| t.source.as_deref() != Some("MIDI")))
                .map(|i| (i.position, i.position + i.length))
                .collect(),
        )
    });
    let Some(spans) = spans else { return -1 };
    let mut heard = false;
    for frame in 0..frames.max(0) as usize {
        let at = start + frame as f64 / rate as f64;
        let value = if spans.iter().any(|(from, to)| at >= *from && at < *to) { 0.25 } else { 0.0 };
        heard |= value != 0.0;
        for channel in 0..channels.max(0) as usize {
            unsafe { *buf.add(frame * channels as usize + channel) = value };
        }
    }
    heard as c_int
}

unsafe extern "C" fn audio_accessor_state_changed(_accessor: *mut api::AudioAccessor) -> bool {
    with(|s| s.listening.accessor_changed)
}

unsafe extern "C" fn destroy_audio_accessor(accessor: *mut api::AudioAccessor) {
    with(|s| {
        if let Some(entry) = s.listening.accessors.get_mut(unpack(accessor as u64).1 as usize) {
            entry.1 = true;
        }
    })
}

// ----- parameters' steps -----

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn track_fx_get_parameter_step_sizes(
    track: *mut MediaTrack,
    fx: c_int,
    param: c_int,
    step: *mut f64,
    small: *mut f64,
    large: *mut f64,
    toggle: *mut bool,
) -> bool {
    let stepped = counted(|s| {
        let p = fx_of(s, track as u64, fx).and_then(|f| f.params.get(param as usize))?;
        let Display::Steps(values) = &p.display else { return None };
        (values.len() >= 2).then(|| ((p.max - p.min) / (values.len() - 1) as f64, values.len() == 2))
    });
    let Some((size, two)) = stepped else {
        type StepSizes = unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, *mut f64, *mut f64, *mut f64, *mut bool) -> bool;
        return next_fn!(c"TrackFX_GetParameterStepSizes" as StepSizes)
            .is_some_and(|f| unsafe { f(track, fx, param, step, small, large, toggle) });
    };
    unsafe {
        *step = size;
        *small = size;
        *large = size;
        *toggle = two;
    }
    true
}

// ----- an FX's own name -----

/// REAPER's `fx_name`: the plug-in's own name, whatever the FX was renamed (measured in 7.82); other keys go on.
unsafe extern "C" fn track_fx_get_named_config_parm(
    track: *mut MediaTrack,
    fx: c_int,
    key: *const c_char,
    buf: *mut c_char,
    size: c_int,
) -> bool {
    if name_of(key) == "fx_name" {
        return counted(|s| fx_of(s, track as u64, fx).map(|f| write(buf, size, &f.name)).is_some());
    }
    type Get = unsafe extern "C" fn(*mut MediaTrack, c_int, *const c_char, *mut c_char, c_int) -> bool;
    next_fn!(c"TrackFX_GetNamedConfigParm" as Get).is_some_and(|f| unsafe { f(track, fx, key, buf, size) })
}

unsafe extern "C" fn take_fx_get_named_config_parm(
    take: *mut MediaItem_Take,
    fx: c_int,
    key: *const c_char,
    buf: *mut c_char,
    size: c_int,
) -> bool {
    if name_of(key) == "fx_name" {
        return counted(|s| fx_of(s, take as u64, fx).map(|f| write(buf, size, &f.name)).is_some());
    }
    type Get = unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_char, *mut c_char, c_int) -> bool;
    next_fn!(c"TakeFX_GetNamedConfigParm" as Get).is_some_and(|f| unsafe { f(take, fx, key, buf, size) })
}

// ----- chunks that keep the GUIDs written in them, and tracks' P_EXT -----

fn guids_in(chunk: &str, key: &str) -> Vec<String> {
    chunk
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix(key))
        .map(|rest| rest.trim().to_string())
        .filter(|guid| guid.starts_with('{') && guid.ends_with('}'))
        .collect()
}

unsafe extern "C" fn get_track_state_chunk(track: *mut MediaTrack, buf: *mut c_char, size: c_int, isundo: bool) -> bool {
    if !with(|s| s.listening.chunk_guids) {
        return next_fn!(c"GetTrackStateChunk" as unsafe extern "C" fn(*mut MediaTrack, *mut c_char, c_int, bool) -> bool)
            .is_some_and(|f| unsafe { f(track, buf, size, isundo) });
    }
    counted(|s| {
        let Some(mut t) = track_of(s, track as u64).cloned() else { return false };
        // Neither its sends nor its selection are in a track's chunk (7.82 lab); equal states share a number, so the chunk
        // reads the same until the track changes.
        t.sends.clear();
        t.selected = false;
        let mut items = String::new();
        for item in &t.items {
            items += &format!("<ITEM\nPOSITION {}\nIGUID {}\n", item.position, item.guid);
            for take in &item.takes {
                items += &format!("NAME \"{}\"\nGUID {}\n", take.name, take.guid);
            }
            items += ">\n";
        }
        let key = super::chunk_key(&t);
        let n = match s.chunks.iter().position(|kept| *kept == key) {
            Some(n) => n,
            None => {
                s.chunks.push(key);
                s.chunks.len() - 1
            }
        };
        let text =
            format!("<TRACK {}\nNAME \"{}\"\nTRACKID {}\nFAKECHUNK {n}\n{}{}>\n", t.guid, t.name, t.guid, fx_chain_text(&t.fx), items);
        write(buf, size, &text);
        true
    })
}

unsafe extern "C" fn set_track_state_chunk(track: *mut MediaTrack, chunk: *const c_char, isundo: bool) -> bool {
    let set = next_fn!(c"SetTrackStateChunk" as unsafe extern "C" fn(*mut MediaTrack, *const c_char, bool) -> bool);
    if !set.is_some_and(|f| unsafe { f(track, chunk, isundo) }) {
        return false;
    }
    if !with(|s| s.listening.chunk_guids) {
        return true;
    }
    let text = name_of(chunk);
    let header = text.lines().next().and_then(|line| line.strip_prefix("<TRACK ")).map(|g| g.trim().to_string());
    let (fx, items, takes) = (guids_in(&text, "FXID "), guids_in(&text, "IGUID "), guids_in(&text, "GUID "));
    counted(|s| {
        let Some(t) = track_mut(s, track as u64) else { return false };
        if let Some(guid) = header {
            t.guid = guid;
        }
        for (f, guid) in t.fx.iter_mut().zip(fx) {
            f.guid = guid;
        }
        for (item, guid) in t.items.iter_mut().zip(items) {
            item.guid = guid;
        }
        for (take, guid) in t.items.iter_mut().flat_map(|i| i.takes.iter_mut()).zip(takes) {
            take.guid = guid;
        }
        true
    })
}

unsafe extern "C" fn get_set_media_track_info_string(track: *mut MediaTrack, parm: *const c_char, buf: *mut c_char, set: bool) -> bool {
    let key = name_of(parm);
    if !key.starts_with("P_EXT:") {
        return next_fn!(c"GetSetMediaTrackInfo_String" as unsafe extern "C" fn(*mut MediaTrack, *const c_char, *mut c_char, bool) -> bool)
            .is_some_and(|f| unsafe { f(track, parm, buf, set) });
    }
    let Some(guid) = with(|s| track_of(s, track as u64).map(|t| t.guid.clone())) else { return false };
    if set {
        let value = name_of(buf);
        with(|s| {
            if value.is_empty() {
                s.listening.track_ext.remove(&(guid, key));
            } else {
                s.listening.track_ext.insert((guid, key), value);
            }
            s.undo_dirty = true;
        });
        return true;
    }
    with(|s| match s.listening.track_ext.get(&(guid, key)) {
        Some(value) => {
            // (REAPER isn't told the buffer's size: like it, the fake writes the whole value, so a buffer handed to it
            // must hold that)
            write(buf, c_int::MAX, value);
            true
        }
        None => false,
    })
}

/// Listening's functions, looked up before the rest of the fake's (see the module's words); none while one of them asks
/// the rest of the fake for its own (`next`).
pub(super) fn lookup(name: &CStr) -> Option<*mut c_void> {
    if PASSING.with(Cell::get) {
        return None;
    }
    macro_rules! table {
        ($($name:literal => $f:expr),* $(,)?) => {
            match name.to_bytes() {
                $($name => Some($f as *mut c_void),)*
                _ => None,
            }
        };
    }
    table! {
        b"GetSetProjectInfo" => get_set_project_info as unsafe extern "C" fn(*mut ReaProject, *const c_char, f64, bool) -> f64,
        b"GetSetProjectInfo_String" => get_set_project_info_string as unsafe extern "C" fn(*mut ReaProject, *const c_char, *mut c_char, bool) -> bool,
        b"projectconfig_var_getoffs" => projectconfig_var_getoffs as unsafe extern "C" fn(*const c_char, *mut c_int) -> c_int,
        b"projectconfig_var_addr" => projectconfig_var_addr as unsafe extern "C" fn(*mut ReaProject, c_int) -> *mut c_void,
        b"Audio_IsRunning" => audio_is_running as unsafe extern "C" fn() -> c_int,
        b"GetAudioDeviceInfo" => get_audio_device_info as unsafe extern "C" fn(*const c_char, *mut c_char, c_int) -> bool,
        b"Audio_RegHardwareHook" => audio_reg_hardware_hook as unsafe extern "C" fn(bool, *mut api::audio_hook_register_t) -> c_int,
        b"Master_GetPlayRate" => master_get_play_rate as unsafe extern "C" fn(*mut ReaProject) -> f64,
        b"GetNumAudioOutputs" => get_num_audio_outputs as unsafe extern "C" fn() -> c_int,
        b"Audio_Init" => audio_init as unsafe extern "C" fn(),
        b"GetToggleCommandState" => get_toggle_command_state as unsafe extern "C" fn(c_int) -> c_int,
        b"PCM_Source_CreateFromFile" => pcm_source_create_from_file as unsafe extern "C" fn(*const c_char) -> *mut api::PCM_source,
        b"PCM_Source_Destroy" => pcm_source_destroy as unsafe extern "C" fn(*mut api::PCM_source),
        b"GetMediaSourceType" => get_media_source_type as unsafe extern "C" fn(*mut api::PCM_source, *mut c_char, c_int),
        b"GetMediaSourceParent" => get_media_source_parent as unsafe extern "C" fn(*mut api::PCM_source) -> *mut api::PCM_source,
        b"PCM_Source_GetSectionInfo" => pcm_source_get_section_info as unsafe extern "C" fn(*mut api::PCM_source, *mut f64, *mut f64, *mut bool) -> bool,
        b"GetMediaSourceLength" => get_media_source_length as unsafe extern "C" fn(*mut api::PCM_source, *mut bool) -> f64,
        b"GetMediaSourceNumChannels" => get_media_source_num_channels as unsafe extern "C" fn(*mut api::PCM_source) -> c_int,
        b"GetMediaSourceSampleRate" => get_media_source_sample_rate as unsafe extern "C" fn(*mut api::PCM_source) -> c_int,
        b"GetMediaSourceFileName" => get_media_source_file_name as unsafe extern "C" fn(*mut api::PCM_source, *mut c_char, c_int),
        b"PlayPreviewEx" => play_preview_ex as unsafe extern "C" fn(*mut api::preview_register_t, c_int, f64) -> c_int,
        b"StopPreview" => stop_preview as unsafe extern "C" fn(*mut api::preview_register_t) -> c_int,
        b"CreateTrackAudioAccessor" => create_track_audio_accessor as unsafe extern "C" fn(*mut MediaTrack) -> *mut api::AudioAccessor,
        b"GetAudioAccessorSamples" => get_audio_accessor_samples as unsafe extern "C" fn(*mut api::AudioAccessor, c_int, c_int, f64, c_int, *mut f64) -> c_int,
        b"AudioAccessorStateChanged" => audio_accessor_state_changed as unsafe extern "C" fn(*mut api::AudioAccessor) -> bool,
        b"DestroyAudioAccessor" => destroy_audio_accessor as unsafe extern "C" fn(*mut api::AudioAccessor),
        b"TrackFX_GetParameterStepSizes" => track_fx_get_parameter_step_sizes as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, *mut f64, *mut f64, *mut f64, *mut bool) -> bool,
        b"GetTrackStateChunk" => get_track_state_chunk as unsafe extern "C" fn(*mut MediaTrack, *mut c_char, c_int, bool) -> bool,
        b"SetTrackStateChunk" => set_track_state_chunk as unsafe extern "C" fn(*mut MediaTrack, *const c_char, bool) -> bool,
        b"GetSetMediaTrackInfo_String" => get_set_media_track_info_string as unsafe extern "C" fn(*mut MediaTrack, *const c_char, *mut c_char, bool) -> bool,
        b"TrackFX_GetNamedConfigParm" => track_fx_get_named_config_parm as unsafe extern "C" fn(*mut MediaTrack, c_int, *const c_char, *mut c_char, c_int) -> bool,
        b"TakeFX_GetNamedConfigParm" => take_fx_get_named_config_parm as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_char, *mut c_char, c_int) -> bool,
    }
}
