//! The REAPER calls the bridge makes, as safe functions over the API table. Main thread only (Api isn't Send).
//! A function this REAPER lacks reads as empty or None rather than failing.

use std::borrow::Cow;
use std::cell::Cell;
use std::ffi::{c_char, c_int, CString};
use std::ptr;
use std::thread::LocalKey;

use serde_json::{json, Value};

use crate::api::{self, Api, MediaItem, MediaItem_Take, MediaTrack, ReaProject, TrackEnvelope};

pub mod generic;
mod items;
// (r-tracks) M4's track, routing, project and action calls.
mod io;
mod objects;
mod write;
pub use items::*;
pub use objects::*;
// m4-r-fxenv: FX in any chain, envelopes, points and automation items.
mod fxenv;
pub use fxenv::{EnvPoint, EqParam};
// M5 listening (m5-reaper): render settings, the audio device, sources, previews, accessors.
mod audio;
pub use audio::*;

/// File: Quit REAPER.
pub const ACTION_QUIT: i32 = 40004;

/// A text as REAPER's C takes it, NUL-terminated: on the stack when short (nearly every key is a literal, "D_VOL" or
/// "P_NAME"), on the heap past 63 bytes (a path, a chunk); an inner NUL is refused. A track's row asks for some two
/// hundred keys, a snapshot of 200 tracks for forty thousand: none of them is an allocation.
pub(crate) struct CText {
    short: [u8; CTEXT_SHORT],
    /// The bytes of `short` in use, the NUL included.
    len: usize,
    long: Option<CString>,
}

const CTEXT_SHORT: usize = 64;

/// Closing an undo block is cleanup: old saved labels must not keep it open or disagree with its metadata.
pub(crate) fn undo_label(label: &str) -> Cow<'_, str> {
    if label.chars().any(char::is_control) {
        Cow::Owned(label.chars().map(|c| if c.is_control() { ' ' } else { c }).collect())
    } else {
        Cow::Borrowed(label)
    }
}

pub(crate) fn ctext(text: &str) -> Option<CText> {
    if text.as_bytes().contains(&0) {
        return None;
    }
    if text.len() < CTEXT_SHORT {
        let mut short = [0u8; CTEXT_SHORT];
        short[..text.len()].copy_from_slice(text.as_bytes());
        Some(CText { short, len: text.len() + 1, long: None })
    } else {
        Some(CText { short: [0; CTEXT_SHORT], len: 0, long: Some(CString::new(text).ok()?) })
    }
}

impl CText {
    pub fn as_ptr(&self) -> *const c_char {
        match &self.long {
            Some(long) => long.as_ptr(),
            None => self.short.as_ptr().cast(),
        }
    }

    /// The text's bytes with the NUL, for a getter-setter that writes into the caller's buffer.
    pub fn into_bytes_with_nul(self) -> Vec<u8> {
        match self.long {
            Some(long) => long.into_bytes_with_nul(),
            None => self.short[..self.len].to_vec(),
        }
    }
}

/// Reads a text a getter writes into a buffer it's told the size of, the buffer grown ×4 while the text fills it, up to
/// `most` (None past it, or when the getter fails). REAPER makes the whole text on every call (a track's state with its
/// plug-ins' states, an item's with a MIDI take's every event), so a reader starts from twice what its last text needed
/// (`last`; `least` at the least): the same object read again (a change's fence, what it applied, its undo) or one like
/// it is one call, where a buffer grown from `least` each time took up to seven. (A buffer bigger than its text costs what's
/// written of it: zeroed pages come from the system as they're first touched.)
fn grown_text(
    last: &'static LocalKey<Cell<usize>>,
    least: usize,
    most: usize,
    mut fill: impl FnMut(*mut c_char, c_int) -> bool,
) -> Option<String> {
    let mut size = last.get().saturating_mul(2).clamp(least, most);
    loop {
        let mut buf = vec![0u8; size];
        if !fill(buf.as_mut_ptr().cast(), size as c_int) {
            return None;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(size);
        if len + 1 < size {
            last.set(len);
            buf.truncate(len);
            return Some(String::from_utf8_lossy(&buf).into_owned());
        }
        if size >= most {
            return None;
        }
        size = size.saturating_mul(4).min(most);
    }
}

thread_local! {
    /// How many writes this extension has made to REAPER (every setter, action and save through these wrappers, inside
    /// an undo block or not): what a digest's kept pages are good for. A bare write outside a block moves neither
    /// REAPER's state change count nor its undo history (7.82), so the count alone can't say the project is as it was.
    static WRITES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One more write of this extension's.
pub(crate) fn wrote() {
    WRITES.with(|writes| writes.set(writes.get() + 1));
}

/// How many writes this extension has made to REAPER so far.
pub fn writes() -> u64 {
    WRITES.with(|writes| writes.get())
}

pub struct Reaper<'a> {
    api: &'a Api,
}

/// A project REAPER handed out; only this module (and its children) makes one, so the safe calls get REAPER's own pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Project(*mut ReaProject);

/// A track REAPER handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Track(*mut MediaTrack);

/// A media item REAPER handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Item(*mut MediaItem);

/// A take REAPER handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Take(*mut MediaItem_Take);

/// An envelope REAPER handed out (track, FX parameter or take).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Envelope(*mut TrackEnvelope);

/// What owns an FX chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FxOwner {
    Track(Track),
    Take(Take),
}

/// One FX in a chain, addressed the way REAPER's FX calls do: the index, with 0x1000000 for the input chain and
/// 0x2000000 for a container's child (the address `container_item.N` gives).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fx {
    pub owner: FxOwner,
    pub index: i32,
}

impl Project {
    /// The first project the fake REAPER hands out, for tests that need a handle.
    #[cfg(test)]
    pub fn first() -> Project {
        Reaper::new(&fake::api()).current_project().expect("a fake project").ptr
    }

    /// Tells projects apart (a tab switch) without exposing the pointer.
    pub fn addr(self) -> usize {
        self.0 as usize
    }
}

/// The project in the active tab.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectInfo {
    pub ptr: Project,
    /// The .rpp file; empty while unsaved.
    pub path: String,
    pub name: String,
}

/// Why REAPER can't take work: `status`'s `busy`, and the reason given with a busy error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Loading,
    Rendering,
    /// The main thread stopped ticking: a modal dialog, or a long load or render.
    Modal,
    Starting,
    /// The main thread has been running one request for longer than a stall: a plug-in loading, or a dialog that request
    /// opened (a licence prompt), which nothing else can get past.
    Working,
    /// M5 listening (m5-reaper): a render Kumi asked for is queued or running (it holds the main thread inside REAPER's).
    KumiRender,
}

impl Busy {
    pub fn code(self) -> &'static str {
        match self {
            Busy::Loading => "loading",
            Busy::Rendering => "rendering",
            Busy::Modal => "modal",
            Busy::Starting => "starting",
            Busy::Working => "working",
            Busy::KumiRender => "rendering",
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            Busy::Loading => "REAPER is loading or saving a project",
            Busy::Rendering => "REAPER is rendering",
            Busy::Modal => "REAPER isn't answering: a dialog may be open",
            Busy::Starting => "REAPER is starting",
            Busy::Working => "REAPER is still running a long request (a plug-in loading, or a dialog that request opened)",
            Busy::KumiRender => "REAPER is rendering for Kumi",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayState {
    pub playing: bool,
    pub paused: bool,
    pub recording: bool,
}

impl PlayState {
    pub fn from_bits(bits: i32) -> PlayState {
        PlayState { playing: bits & 1 != 0, paused: bits & 2 != 0, recording: bits & 4 != 0 }
    }

    pub fn to_json(self) -> Value {
        json!({"playing": self.playing, "paused": self.paused, "recording": self.recording})
    }
}

impl<'a> Reaper<'a> {
    pub fn new(api: &'a Api) -> Self {
        Reaper { api }
    }

    pub fn api(&self) -> &'a Api {
        self.api
    }

    pub fn app_version(&self) -> String {
        self.api.GetAppVersion().map(|f| unsafe { api::string(f()) }).unwrap_or_default()
    }

    pub fn resource_path(&self) -> String {
        self.api.GetResourcePath().map(|f| unsafe { api::string(f()) }).unwrap_or_default()
    }

    pub fn current_project(&self) -> Option<ProjectInfo> {
        self.project_at(-1)
    }

    /// Every open project tab, in tab order.
    pub fn projects(&self) -> Vec<ProjectInfo> {
        (0..).map_while(|i| self.project_at(i)).collect()
    }

    fn project_at(&self, index: i32) -> Option<ProjectInfo> {
        let enum_projects = self.api.EnumProjects()?;
        let mut proj = ptr::null_mut();
        let path = api::read_buffer(|buf, size| proj = unsafe { enum_projects(index, buf, size) });
        if proj.is_null() {
            return None;
        }
        let name = match self.api.GetProjectName() {
            Some(f) => api::read_buffer(|buf, size| unsafe { f(proj, buf, size) }),
            None => String::new(),
        };
        Some(ProjectInfo { ptr: Project(proj), path, name })
    }

    /// The folder REAPER keeps a project's recorded media in (its own, or the default path when it has none).
    pub fn project_media_folder(&self, proj: Project) -> Option<String> {
        let f = self.api.GetProjectPathEx()?;
        Some(api::read_buffer(|buf, size| unsafe { f(proj.0, buf, size) })).filter(|path| !path.is_empty())
    }

    pub fn is_dirty(&self, proj: Project) -> bool {
        self.api.IsProjectDirty().is_some_and(|f| unsafe { f(proj.0) } != 0)
    }

    /// Saves a project that has a file (one without would open Save As).
    pub fn save(&self, proj: Project) -> bool {
        wrote();
        match self.api.Main_SaveProject() {
            Some(f) => {
                unsafe { f(proj.0, false) };
                true
            }
            None => false,
        }
    }

    /// The project REAPER is loading or saving now, if it is.
    pub fn project_in_load_save(&self) -> Option<Project> {
        let project = unsafe { self.api.GetCurrentProjectInLoadSave()?() };
        (!project.is_null()).then_some(Project(project))
    }

    /// Whether a project is loading or saving, or one is rendering: no edits then. REAPER reports loading and saving
    /// as one state, so both read as `loading`.
    pub fn busy(&self) -> Option<Busy> {
        if self.api.GetCurrentProjectInLoadSave().is_some_and(|f| !unsafe { f() }.is_null()) {
            return Some(Busy::Loading);
        }
        if self.api.EnumProjects().is_some_and(|f| !unsafe { f(0x4000_0000, ptr::null_mut(), 0) }.is_null()) {
            return Some(Busy::Rendering);
        }
        None
    }

    /// Whether the main window takes input; false while a modal dialog has it disabled. None when REAPER or this
    /// platform can't say.
    pub fn main_window_enabled(&self) -> Option<bool> {
        let hwnd = unsafe { self.api.GetMainHwnd()?() };
        if hwnd.is_null() {
            return None;
        }
        crate::window::is_enabled(hwnd)
    }

    pub fn play_state(&self, proj: Project) -> PlayState {
        PlayState::from_bits(self.api.GetPlayStateEx().map(|f| unsafe { f(proj.0) }).unwrap_or(0))
    }

    pub fn state_change_count(&self, proj: Project) -> Option<i32> {
        self.api.GetProjectStateChangeCount().map(|f| unsafe { f(proj.0) })
    }

    /// The project's tempo now, in beats per minute.
    pub fn tempo(&self) -> Option<f64> {
        self.api.Master_GetTempo().map(|f| unsafe { f() })
    }

    pub fn track_count(&self, proj: Project) -> i32 {
        self.api.CountTracks().map(|f| unsafe { f(proj.0) }).unwrap_or(0)
    }

    /// The selected tracks, the master first when it is selected.
    pub fn selected_tracks(&self, proj: Project) -> Vec<Track> {
        let (Some(count), Some(get)) = (self.api.CountSelectedTracks2(), self.api.GetSelectedTrack2()) else { return vec![] };
        let n = unsafe { count(proj.0, true) };
        (0..n).map(|i| unsafe { get(proj.0, i, true) }).filter(|t| !t.is_null()).map(Track).collect()
    }

    /// `{index, guid, name}`; index is 1-based as REAPER shows it, 0 for the master.
    pub fn track_json(&self, Track(track): Track) -> Value {
        let number = match self.api.GetMediaTrackInfo_Value() {
            Some(f) => unsafe { f(track, c"IP_TRACKNUMBER".as_ptr()) },
            None => 0.0,
        };
        let name = match self.api.GetTrackName() {
            Some(f) => api::read_buffer(|buf, size| {
                unsafe { f(track, buf, size) };
            }),
            None => String::new(),
        };
        let guid = self.track_guid(Track(track));
        let index = if number < 0.0 { 0 } else { number as i64 };
        json!({"index": index, "guid": guid, "name": name})
    }

    /// A main-section action's command id: a number, or a named command such as `_SWS_ABOUT`.
    pub fn command_id(&self, command: &str) -> Option<i32> {
        if let Ok(id) = command.parse::<i32>() {
            return (id > 0).then_some(id);
        }
        let lookup = self.api.NamedCommandLookup()?;
        let name = CString::new(command).ok()?;
        let id = unsafe { lookup(name.as_ptr()) };
        (id > 0).then_some(id)
    }

    /// Registers a script file as a main-section action (`commit`: written to REAPER's key map, so it stays across restarts):
    /// its command id.
    pub fn add_script(&self, path: &std::path::Path, commit: bool) -> Option<i32> {
        wrote();
        let register = self.api.AddRemoveReaScript()?;
        let file = CString::new(path.to_string_lossy().into_owned()).ok()?;
        let id = unsafe { register(true, 0, file.as_ptr(), commit) };
        (id > 0).then_some(id)
    }

    /// Takes a script's action away (from REAPER's key map too, with `commit`).
    pub fn remove_script(&self, path: &std::path::Path, commit: bool) -> bool {
        wrote();
        let (Some(register), Ok(file)) = (self.api.AddRemoveReaScript(), CString::new(path.to_string_lossy().into_owned())) else {
            return false;
        };
        unsafe { register(false, 0, file.as_ptr(), commit) };
        true
    }

    /// An action's named command ("RS…" for a script), without the underscore its lookup takes.
    pub fn named_command(&self, command: i32) -> Option<String> {
        let f = self.api.ReverseNamedCommandLookup()?;
        let named = unsafe { api::string(f(command)) };
        (!named.is_empty()).then_some(named)
    }

    /// The action's name as the action list shows it; None when there is no such action.
    pub fn action_name(&self, command: i32) -> Option<String> {
        let f = self.api.kbd_getTextFromCmd()?;
        let name = unsafe { api::string(f(command, ptr::null_mut())) };
        (!name.is_empty()).then_some(name)
    }

    pub fn run_action(&self, command: i32) -> bool {
        wrote();
        match self.api.Main_OnCommandEx() {
            Some(f) => {
                unsafe { f(command, 0, ptr::null_mut()) };
                true
            }
            None => false,
        }
    }

    pub fn prevent_ui_refresh(&self, count: i32) {
        if let Some(f) = self.api.PreventUIRefresh() {
            unsafe { f(count) }
        }
    }

    pub fn console(&self, text: &str) {
        if let (Some(f), Ok(text)) = (self.api.ShowConsoleMsg(), CString::new(text)) {
            unsafe { f(text.as_ptr()) }
        }
    }
}

#[cfg(test)]
pub mod fake;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_go_through_the_table() {
        fake::reset();
        fake::with(|s| {
            s.project = Some(("/lab/Projects/a.rpp".into(), "a.rpp".into()));
            s.play_bits = 5;
        });
        fake::select_tracks(&[1]);
        let api = fake::api();
        let reaper = Reaper::new(&api);
        assert_eq!(reaper.app_version(), "7.82/macOS-arm64");
        assert_eq!(reaper.resource_path(), "/lab/REAPER");
        let project = reaper.current_project().unwrap();
        assert_eq!((project.path.as_str(), project.name.as_str()), ("/lab/Projects/a.rpp", "a.rpp"));
        assert_eq!(reaper.play_state(project.ptr), PlayState { playing: true, paused: false, recording: true });
        assert_eq!(reaper.track_count(project.ptr), 2);
        let selected = reaper.selected_tracks(project.ptr);
        assert_eq!(selected.len(), 1);
        assert_eq!(reaper.track_json(selected[0]), json!({"index": 2, "guid": "{BBBB}", "name": "Bass"}));
        assert_eq!(reaper.busy(), None);
        fake::with(|s| s.loading = true);
        assert_eq!(reaper.busy(), Some(Busy::Loading));
    }

    #[test]
    fn missing_functions_read_as_empty() {
        let api = Api::load(|_| ptr::null_mut());
        let reaper = Reaper::new(&api);
        assert_eq!(reaper.app_version(), "");
        assert_eq!(reaper.current_project(), None);
        assert_eq!(reaper.busy(), None);
        assert!(!reaper.run_action(ACTION_QUIT));
    }

    // Second sweep RP1: a chunk reader starts from twice what its last text needed, so the same object read again is one
    // call to REAPER (which makes the whole text each call), where a buffer grown from 64 KiB each time took several.
    #[test]
    fn a_chunk_read_again_is_one_call_and_a_bigger_one_grows_from_the_last() {
        thread_local! {
            static LAST: Cell<usize> = const { Cell::new(0) };
        }
        let calls = Cell::new(0);
        // (REAPER's getters write what fits and a NUL)
        let read = |len: usize, least: usize, most: usize| {
            let text = "x".repeat(len);
            let before = calls.get();
            let got = grown_text(&LAST, least, most, |buf, size| {
                calls.set(calls.get() + 1);
                let n = len.min(size as usize - 1);
                unsafe {
                    ptr::copy_nonoverlapping(text.as_ptr(), buf.cast::<u8>(), n);
                    *buf.add(n) = 0;
                }
                true
            });
            assert!(got.is_none() || got.as_deref() == Some(text.as_str()));
            (got.is_some(), calls.get() - before)
        };
        let chunk = |len| read(len, 1 << 16, 256 << 20);
        // A 1.1 MB track chunk: 64 KiB, 256 KiB and 1 MiB too small, 4 MiB holds it. Read again: once.
        assert_eq!(chunk(1_100_000), (true, 4));
        assert_eq!(chunk(1_100_000), (true, 1));
        assert_eq!(chunk(2_000_000), (true, 1), "one a little bigger fits twice the last");
        assert_eq!(chunk(10), (true, 1));
        assert_eq!(chunk(1_100_000), (true, 4), "after a small one, from 64 KiB again");
        // Past the most it reads, none (and what the last one needed stays).
        assert_eq!(read(5_000, 16, 1024), (false, 1), "twice the last is past the most: only the most is tried");
        assert_eq!(LAST.get(), 1_100_000);
    }

    #[test]
    fn a_c_text_is_on_the_stack_when_short_on_the_heap_when_long_and_refuses_an_inner_nul() {
        let read = |text: &CText| unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }.to_str().unwrap().to_owned();
        let short = ctext("D_VOL").unwrap();
        assert!(short.long.is_none());
        assert_eq!(read(&short), "D_VOL");
        assert_eq!(short.into_bytes_with_nul(), b"D_VOL\0");
        let edge = ctext(&"k".repeat(63)).unwrap();
        assert!(edge.long.is_none(), "63 bytes and the NUL fit");
        assert_eq!(read(&edge).len(), 63);
        let long = ctext(&"k".repeat(64)).unwrap();
        assert!(long.long.is_some());
        assert_eq!(read(&long).len(), 64);
        assert_eq!(long.into_bytes_with_nul().len(), 65);
        assert!(ctext("a\0b").is_none());
        assert!(ctext(&format!("{}\0{}", "x".repeat(40), "y".repeat(40))).is_none());
        assert_eq!(read(&ctext("").unwrap()), "");
    }
}
