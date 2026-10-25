//! Any of REAPER's API functions by name, with JSON arguments (the generic call), and a Lua chunk run as a ReaScript.
//!
//! The call is generated from the vendored header (build.rs): each parameter is read by what its C type and name say it
//! is. Numbers and flags are numbers and booleans; text is a string; a handle is a ref (a track, item, take or envelope;
//! a project is the open one); a buffer REAPER writes into, or a value it writes back (`…Out`), comes back in `out`
//! under its name. A function that takes or returns anything else (a callback, a window, a bitmap, a source) can't be
//! called this way: run-lua can.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};

use serde_json::{Map, Value};

use super::{Project, Reaper};
use crate::api::{self, Api, MediaItem, MediaItem_Take, MediaTrack, ReaProject, TrackEnvelope, FUNCTIONS};

include!(concat!(env!("OUT_DIR"), "/reaper_generic.rs"));

/// How a call finds the objects its refs name, and names the objects REAPER gives back.
pub trait Handles {
    /// The pointer of the object `reference` names, of type `base` ("MediaTrack" and the like).
    fn find(&self, base: &str, reference: &str) -> Result<*mut c_void, String>;
    /// The ref of an object REAPER gave back, or what can be said of it.
    fn name(&self, base: &str, pointer: *mut c_void) -> Value;
}

/// A sized buffer's size when the caller gives none (`<name>_sz` gives another, up to `MOST_BUFFER`).
const BUFFER: usize = 64 * 1024;
/// The most a buffer holds, and what one REAPER isn't told the size of (its `…NeedBig` and `…Need64` texts) always holds:
/// REAPER writes the whole text whatever the buffer is (lab 7.82: more than 96 KiB of a track's 200 KB P_EXT), and the
/// memory is only taken as far as it writes.
const MOST_BUFFER: usize = 64 * 1024 * 1024;
/// The most of a buffer's contents an answer carries, as text (past it, `<name>_cut`): the host's frame holds 256 KiB
/// (lab: a take's 144 KB of MIDI, as hex, was refused whole).
const MOST_OUT: usize = 96 * 1024;

enum Slot {
    Int(Box<c_int>),
    UInt(Box<c_uint>),
    Int64(Box<i64>),
    Double(Box<f64>),
    Float(Box<f32>),
    Bool(Box<bool>),
    Text(Box<*const c_char>),
    Buf(Box<[u8]>),
    /// A buffer and the size REAPER is told and writes back (its name).
    Sized(Box<[u8]>, Box<c_int>, String),
}

/// One call's arguments, and what it keeps alive until REAPER has answered: text, buffers and the values it writes back.
pub struct Call<'h> {
    args: Value,
    handles: &'h dyn Handles,
    dry: bool,
    texts: Vec<CString>,
    // (name, slot, reported in `out`)
    slots: Vec<(String, Slot, bool)>,
}

impl<'h> Call<'h> {
    pub fn new(args: &Value, handles: &'h dyn Handles, dry: bool) -> Call<'h> {
        Call { args: args.clone(), handles, dry, texts: Vec::new(), slots: Vec::new() }
    }

    /// The arguments are only checked: REAPER isn't called.
    pub fn dry(&self) -> bool {
        self.dry
    }

    fn arg(&self, k: Option<usize>, name: &str) -> Option<&Value> {
        let k = k?;
        let value = match &self.args {
            Value::Array(list) => list.get(k),
            Value::Object(map) => map.get(name),
            _ => None,
        };
        value.filter(|v| !v.is_null())
    }

    fn missing(name: &str, what: &str) -> String {
        format!("args: {name} must be {what}")
    }

    pub fn int(&mut self, k: Option<usize>, name: &str, optional: bool) -> Result<i64, String> {
        match self.arg(k, name) {
            Some(Value::Bool(b)) => Ok(i64::from(*b)),
            Some(v) => v
                .as_i64()
                .or_else(|| v.as_u64().map(|n| n as i64))
                .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.is_finite()).map(|f| f as i64))
                .ok_or_else(|| Self::missing(name, "a whole number")),
            None if optional || k.is_none() => Ok(0),
            None => Err(Self::missing(name, "a whole number")),
        }
    }

    pub fn double(&mut self, k: Option<usize>, name: &str, optional: bool) -> Result<f64, String> {
        match self.arg(k, name) {
            Some(v) => v.as_f64().ok_or_else(|| Self::missing(name, "a number")),
            None if optional || k.is_none() => Ok(0.0),
            None => Err(Self::missing(name, "a number")),
        }
    }

    pub fn boolean(&mut self, k: Option<usize>, name: &str, optional: bool) -> Result<bool, String> {
        match self.arg(k, name) {
            Some(Value::Bool(b)) => Ok(*b),
            Some(v) if v.as_i64().is_some() => Ok(v.as_i64() != Some(0)),
            Some(_) => Err(Self::missing(name, "true or false")),
            None if optional || k.is_none() => Ok(false),
            None => Err(Self::missing(name, "true or false")),
        }
    }

    pub fn text(&mut self, k: Option<usize>, name: &str, optional: bool) -> Result<*const c_char, String> {
        match self.arg(k, name) {
            Some(Value::String(s)) => {
                let text = CString::new(s.replace('\0', "")).map_err(|_| Self::missing(name, "text"))?;
                self.texts.push(text);
                Ok(self.texts.last().expect("just kept").as_ptr())
            }
            Some(_) => Err(Self::missing(name, "text")),
            None if optional => Ok(std::ptr::null()),
            // (REAPER reads an empty string where none is given: a text it needs is never null)
            None if k.is_none() => Ok(c"".as_ptr()),
            None => Err(Self::missing(name, "text")),
        }
    }

    /// A buffer REAPER writes into (and reads first, for an input): its pointer and size. When REAPER is told the size
    /// (`sized`, the size passed after it), `<name>_sz` may make it smaller or bigger; when it isn't (`…NeedBig`,
    /// `…Need64`), REAPER writes the whole text whatever the caller says, so the buffer is the biggest there is.
    pub fn buf(&mut self, k: Option<usize>, name: &str, sized: bool) -> Result<(*mut c_char, c_int), String> {
        let asked = match &self.args {
            Value::Object(map) => map.get(&format!("{name}_sz")).and_then(Value::as_u64).map(|n| n as usize),
            _ => None,
        };
        let size = if sized { asked.unwrap_or(BUFFER) } else { MOST_BUFFER }.clamp(2, MOST_BUFFER);
        let mut buf = vec![0u8; size].into_boxed_slice();
        match self.arg(k, name) {
            Some(Value::String(s)) => {
                let bytes = s.as_bytes();
                if bytes.len() >= size {
                    return Err(format!("args: {name} is longer than its buffer ({size} bytes): pass {name}_sz"));
                }
                buf[..bytes.len()].copy_from_slice(bytes);
            }
            Some(_) => return Err(Self::missing(name, "text")),
            None => {}
        }
        let ptr = buf.as_mut_ptr().cast::<c_char>();
        self.slots.push((name.to_string(), Slot::Buf(buf), true));
        Ok((ptr, size as c_int))
    }

    /// A buffer with the size REAPER reads and writes back (`int* <name>_sz`): made as big as that size says (the caller's,
    /// a whole number of bytes, or 64 KiB) and REAPER told the size it has, so it never writes past it; what REAPER wrote
    /// comes back (as text, or as hex when it's bytes), with the size it wrote back.
    pub fn buf_in_out(
        &mut self,
        k: Option<usize>,
        name: &str,
        k_size: Option<usize>,
        size_name: &str,
    ) -> Result<(*mut c_char, *mut c_int), String> {
        let size = match self.arg(k_size, size_name) {
            None => BUFFER,
            Some(v) => {
                let n = v
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0)
                    .ok_or_else(|| Self::missing(size_name, "a whole number of bytes"))?;
                if n > MOST_BUFFER as f64 {
                    return Err(format!("args: {size_name} can be at most {MOST_BUFFER} bytes"));
                }
                (n as usize).max(2)
            }
        };
        let mut buf = vec![0u8; size].into_boxed_slice();
        match self.arg(k, name) {
            Some(Value::String(s)) => {
                let bytes = s.as_bytes();
                if bytes.len() >= size {
                    return Err(format!("args: {name} is longer than its buffer ({size} bytes): pass a larger {size_name}"));
                }
                buf[..bytes.len()].copy_from_slice(bytes);
            }
            Some(_) => return Err(Self::missing(name, "text")),
            None => {}
        }
        let mut told = Box::new(size as c_int);
        let (ptr, told_ptr) = (buf.as_mut_ptr().cast::<c_char>(), &mut *told as *mut c_int);
        self.slots.push((name.to_string(), Slot::Sized(buf, told, size_name.to_string()), true));
        Ok((ptr, told_ptr))
    }

    fn slot<T>(&mut self, name: &str, slot: Slot, report: bool, ptr: *mut T) -> *mut T {
        self.slots.push((name.to_string(), slot, report));
        ptr
    }

    pub fn out_int(&mut self, k: Option<usize>, name: &str) -> Result<*mut c_int, String> {
        let mut b = Box::new(self.int(k, name, true)? as c_int);
        let p = &mut *b as *mut c_int;
        Ok(self.slot(name, Slot::Int(b), true, p))
    }

    pub fn out_uint(&mut self, k: Option<usize>, name: &str) -> Result<*mut c_uint, String> {
        let mut b = Box::new(self.int(k, name, true)? as c_uint);
        let p = &mut *b as *mut c_uint;
        Ok(self.slot(name, Slot::UInt(b), true, p))
    }

    pub fn out_int64(&mut self, k: Option<usize>, name: &str) -> Result<*mut i64, String> {
        let mut b = Box::new(self.int(k, name, true)?);
        let p = &mut *b as *mut i64;
        Ok(self.slot(name, Slot::Int64(b), true, p))
    }

    pub fn out_double(&mut self, k: Option<usize>, name: &str) -> Result<*mut f64, String> {
        let mut b = Box::new(self.double(k, name, true)?);
        let p = &mut *b as *mut f64;
        Ok(self.slot(name, Slot::Double(b), true, p))
    }

    /// A value a GetSet… function writes back when it gets and reads when it sets (GetSet_LoopTimeRange's start and end):
    /// the caller's, which a call that sets must give (REAPER would set it to 0 otherwise).
    pub fn out_double_set(&mut self, k: Option<usize>, name: &str, k_set: Option<usize>, set: &str) -> Result<*mut f64, String> {
        if self.boolean(k_set, set, true)? && self.arg(k, name).is_none() {
            return Err(format!("args: {name} is what the call sets when {set} is true: give it"));
        }
        self.out_double(k, name)
    }

    pub fn out_bool(&mut self, k: Option<usize>, name: &str) -> Result<*mut bool, String> {
        let mut b = Box::new(self.boolean(k, name, true)?);
        let p = &mut *b as *mut bool;
        Ok(self.slot(name, Slot::Bool(b), true, p))
    }

    /// A value given by pointer (`const int*` and the like): null when the caller gives none.
    fn in_ptr<T>(&mut self, given: bool, name: &str, slot: Slot, ptr: *mut T) -> *const T {
        if !given {
            return std::ptr::null();
        }
        self.slot(name, slot, false, ptr) as *const T
    }

    pub fn in_ptr_int(&mut self, k: Option<usize>, name: &str) -> Result<*const c_int, String> {
        let given = self.arg(k, name).is_some();
        let mut b = Box::new(self.int(k, name, true)? as c_int);
        let p = &mut *b as *mut c_int;
        Ok(self.in_ptr(given, name, Slot::Int(b), p))
    }

    pub fn in_ptr_uint(&mut self, k: Option<usize>, name: &str) -> Result<*const c_uint, String> {
        let given = self.arg(k, name).is_some();
        let mut b = Box::new(self.int(k, name, true)? as c_uint);
        let p = &mut *b as *mut c_uint;
        Ok(self.in_ptr(given, name, Slot::UInt(b), p))
    }

    pub fn in_ptr_int64(&mut self, k: Option<usize>, name: &str) -> Result<*const i64, String> {
        let given = self.arg(k, name).is_some();
        let mut b = Box::new(self.int(k, name, true)?);
        let p = &mut *b as *mut i64;
        Ok(self.in_ptr(given, name, Slot::Int64(b), p))
    }

    pub fn in_ptr_double(&mut self, k: Option<usize>, name: &str) -> Result<*const f64, String> {
        let given = self.arg(k, name).is_some();
        let mut b = Box::new(self.double(k, name, true)?);
        let p = &mut *b as *mut f64;
        Ok(self.in_ptr(given, name, Slot::Double(b), p))
    }

    pub fn in_ptr_float(&mut self, k: Option<usize>, name: &str) -> Result<*const f32, String> {
        let given = self.arg(k, name).is_some();
        let mut b = Box::new(self.double(k, name, true)? as f32);
        let p = &mut *b as *mut f32;
        Ok(self.in_ptr(given, name, Slot::Float(b), p))
    }

    pub fn in_ptr_bool(&mut self, k: Option<usize>, name: &str) -> Result<*const bool, String> {
        let given = self.arg(k, name).is_some();
        let mut b = Box::new(self.boolean(k, name, true)?);
        let p = &mut *b as *mut bool;
        Ok(self.in_ptr(given, name, Slot::Bool(b), p))
    }

    // (`isize` and `usize` pointers: none in the header takes one as an input)
    pub fn in_ptr_isize(&mut self, k: Option<usize>, name: &str) -> Result<*const isize, String> {
        let _ = (k, name);
        Err(format!("{name} can't be given this way"))
    }

    pub fn in_ptr_usize(&mut self, k: Option<usize>, name: &str) -> Result<*const usize, String> {
        let _ = (k, name);
        Err(format!("{name} can't be given this way"))
    }

    /// Text REAPER points at (`const char** …Out`).
    pub fn out_text(&mut self, name: &str) -> *mut *const c_char {
        let mut b: Box<*const c_char> = Box::new(std::ptr::null());
        let p = &mut *b as *mut *const c_char;
        self.slot(name, Slot::Text(b), true, p)
    }

    pub fn handle(&mut self, k: Option<usize>, name: &str, base: &str, optional: bool) -> Result<*mut c_void, String> {
        match self.arg(k, name) {
            // (a project is the one open in REAPER: null, 0 or "current" name it)
            Some(Value::String(s)) if base == "ReaProject" && s == "current" => Ok(std::ptr::null_mut()),
            Some(v) if base == "ReaProject" && v.as_i64() == Some(0) => Ok(std::ptr::null_mut()),
            Some(Value::String(s)) => self.handles.find(base, s),
            Some(_) => Err(format!("args: {name} must be a ref")),
            None if optional || base == "ReaProject" => Ok(std::ptr::null_mut()),
            None => Err(format!("args: {name} must be a ref")),
        }
    }

    fn ret_text(&mut self, p: *const c_char) -> Value {
        if p.is_null() {
            Value::Null
        } else {
            Value::String(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        }
    }

    fn ret_handle(&mut self, base: &str, p: *mut c_void) -> Value {
        if p.is_null() {
            Value::Null
        } else {
            self.handles.name(base, p)
        }
    }

    /// What REAPER wrote back, by name: a buffer as text up to its end (at most `MOST_OUT`, and `<name>_cut` when it filled
    /// its buffer or passed that), a sized one as the bytes REAPER said it wrote (hex when they aren't text).
    pub fn outs(&self) -> Map<String, Value> {
        let mut out = Map::new();
        for (name, slot, report) in &self.slots {
            if !report {
                continue;
            }
            let value = match slot {
                Slot::Int(b) => Value::from(**b),
                Slot::UInt(b) => Value::from(**b),
                Slot::Int64(b) => Value::from(**b),
                Slot::Double(b) => Value::from(**b),
                Slot::Float(b) => Value::from(f64::from(**b)),
                Slot::Bool(b) => Value::from(**b),
                Slot::Text(b) => {
                    if b.is_null() {
                        Value::Null
                    } else {
                        Value::String(unsafe { CStr::from_ptr(**b) }.to_string_lossy().into_owned())
                    }
                }
                Slot::Buf(b) => {
                    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
                    let len = end.min(MOST_OUT);
                    if end + 1 >= b.len() || len < end {
                        out.insert(format!("{name}_cut"), Value::Bool(true));
                    }
                    Value::String(String::from_utf8_lossy(&b[..len]).into_owned())
                }
                Slot::Sized(b, told, size_name) => {
                    let wrote = (**told).max(0) as usize;
                    out.insert(size_name.clone(), Value::from(**told));
                    let exact = &b[..wrote.min(b.len())];
                    // (text, maybe with its NUL counted; anything else is bytes, every one of them, two hex digits each)
                    let mut text = exact;
                    while let [rest @ .., 0] = text {
                        text = rest;
                    }
                    let (value, shown) = match std::str::from_utf8(text).ok().filter(|text| !text.contains('\0')) {
                        Some(text) => {
                            let mut end = text.len().min(MOST_OUT);
                            while !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            (Value::String(text[..end].to_owned()), end == text.len())
                        }
                        None => {
                            out.insert(format!("{name}_hex"), Value::Bool(true));
                            let bytes = &exact[..exact.len().min(MOST_OUT / 2)];
                            (Value::String(bytes.iter().map(|b| format!("{b:02x}")).collect()), bytes.len() == exact.len())
                        }
                    };
                    if wrote > b.len() || !shown {
                        out.insert(format!("{name}_cut"), Value::Bool(true));
                    }
                    value
                }
            };
            out.insert(name.clone(), value);
        }
        out
    }
}

/// A function of the table by its name: its index, or what the caller may have meant.
pub fn function(name: &str) -> Result<usize, String> {
    if let Some(index) = FUNCTIONS.iter().position(|f| f.name.to_bytes() == name.as_bytes()) {
        return Ok(index);
    }
    // (the names that share the longest start with it, then those that hold it)
    let lower = name.to_ascii_lowercase();
    let shared = |n: &str| n.to_ascii_lowercase().bytes().zip(lower.bytes()).take_while(|(a, b)| a == b).count();
    let mut near: Vec<(usize, &str)> = FUNCTIONS
        .iter()
        .filter_map(|f| f.name.to_str().ok())
        .map(|n| (shared(n).max(if n.to_ascii_lowercase().contains(&lower) { lower.len() } else { 0 }), n))
        .filter(|(score, _)| *score >= 4.min(lower.len()).max(1))
        .collect();
    near.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().abs_diff(name.len()).cmp(&b.1.len().abs_diff(name.len()))));
    let near: Vec<&str> = near.into_iter().take(5).map(|(_, n)| n).collect();
    if near.is_empty() {
        Err(format!("REAPER has no API function {name:?}"))
    } else {
        Err(format!("REAPER has no API function {name:?}: perhaps {}", near.join(", ")))
    }
}

pub fn name_of(index: usize) -> &'static str {
    FUNCTIONS[index].name.to_str().unwrap_or("?")
}

/// Why a function can't be called by name, or None.
pub fn uncallable(index: usize) -> Option<&'static str> {
    UNCALLABLE[index]
}

/// Functions Kumi never calls by name, and why: they open windows or dialogs, quit, change REAPER's own settings, run
/// programs or register code. An action is run with the `action` method, which keeps dialogs and quitting out.
const DENIED: [(&str, &str); 40] = [
    ("Main_OnCommand", "runs an action: run it with the action method, which keeps dialogs and quitting out"),
    ("KBD_OnMainActionEx", "runs an action: run it with the action method, which keeps dialogs and quitting out"),
    ("PromptForAction", "opens the action list"),
    ("ShowActionList", "opens the action list"),
    ("Dialog", "opens a dialog"),
    ("Browse", "opens a file dialog"),
    ("GetUserInputs", "opens a dialog"),
    ("GetUserFileName", "opens a file dialog"),
    ("ShowMessageBox", "opens a dialog"),
    ("ShowPopupMenu", "opens a menu"),
    ("Menu", "changes REAPER's menus"),
    ("Quit", "quits"),
    ("Prefs", "changes REAPER's preferences"),
    ("Main_openProject", "opens a project, which may ask about unsaved changes"),
    ("Main_SaveProject", "saves through a dialog when the project has no file yet"),
    ("ExecProcess", "runs a program"),
    ("RecursiveCreateDirectory", "makes folders on the disk"),
    ("plugin_register", "registers code in REAPER"),
    ("AddRemoveReaScript", "registers scripts in REAPER"),
    ("Audio_Init", "restarts the audio device"),
    ("Audio_Quit", "stops the audio device"),
    ("OpenColorThemeFile", "changes REAPER's theme"),
    ("SetThemeColor", "changes REAPER's theme"),
    ("OpenMediaExplorer", "opens the media explorer"),
    ("Help_Set", "changes REAPER's help line"),
    ("DockWindow", "moves REAPER's windows"),
    ("realloc_cmd", "manages REAPER's own buffers"),
    // (Kumi's change runs in an undo block of its own: one left open, or an end past it, swallows the producer's later
    // edits; REAPER's undo and redo move its history under Kumi's HISTORY)
    ("Undo_BeginBlock", "opens an undo block of its own inside Kumi's (one left open swallows every later edit)"),
    ("Undo_EndBlock", "ends an undo block, which would end Kumi's own around the call"),
    ("Undo_DoUndo", "moves REAPER's history under Kumi's HISTORY: REAPER's own undo is the producer's"),
    ("Undo_DoRedo", "moves REAPER's history under Kumi's HISTORY: REAPER's own redo is the producer's"),
    ("Undo_OnStateChange", "makes an undo point of its own inside Kumi's undo block"),
    ("PreventUIRefresh", "holds back REAPER's screen, which a call can't let go of again"),
    ("set_config_var", "changes REAPER's preferences"),
    ("MIDIEditor_LastFocused_OnCommand", "runs a MIDI editor action (some open dialogs)"),
    ("MIDIEditor_OnCommand", "runs a MIDI editor action (some open dialogs)"),
    // (an OSC message to REAPER itself can run any action: "/action/40860")
    ("OscLocalMessageToHost", "runs what an OSC message names, any action included"),
    ("CSurf_OnOscControlMessage", "runs what an OSC message names, any action included"),
    ("CrossfadeEditor_OnCommand", "runs a crossfade editor action"),
    ("RenderFileSection", "renders a file onto the disk"),
];

/// Why Kumi never makes this call, with these arguments, by name; None when it may.
pub fn denied(index: usize, args: &Value) -> Option<String> {
    let name = name_of(index);
    if name == "MB" {
        return Some(format!("{name} opens a dialog"));
    }
    if let Some((_, why)) = DENIED.iter().find(|(part, _)| name.contains(part)) {
        return Some(format!("{name} {why}"));
    }
    // RENDER_STATS and RENDER_STATS_SUMMARY run the action their value names before answering ("41824" renders the
    // project over the producer's files): read with an empty value only.
    let given = |param: &str| given(index, args, param);
    if name.starts_with("GetSetProjectInfo_String")
        && given("desc").and_then(Value::as_str).is_some_and(|key| key.trim().to_ascii_uppercase().starts_with("RENDER_STATS"))
        && given("valuestrNeedBig").and_then(Value::as_str).is_some_and(|value| !value.trim().is_empty())
    {
        return Some(format!("{name} runs the action its value names for RENDER_STATS: read it with an empty value"));
    }
    None
}

/// The argument a call gives for parameter `param` of function `index`, by place or by name.
fn given<'a>(index: usize, args: &'a Value, param: &str) -> Option<&'a Value> {
    let at = INPUTS[index].iter().position(|p| *p == param)?;
    match args {
        Value::Array(list) => list.get(at),
        Value::Object(map) => map.get(param),
        _ => None,
    }
}

/// Whether a call of this function with these arguments only reads: a getter, or a GetSet… function asked to get.
pub fn reads_only(index: usize, args: &Value) -> bool {
    let name = name_of(index);
    let given = |param: &str| given(index, args, param);
    if name.starts_with("GetSet") {
        // (a getter and a setter in one: it sets when its flag says so, or, for the repeat, when the value isn't -1)
        if name.starts_with("GetSetRepeat") {
            return given("val").or_else(|| given("repeat")).and_then(Value::as_i64).is_some_and(|v| v < 0);
        }
        for flag in ["setNewValue", "setnewvalue", "set", "is_set", "isSet", "bSet", "setNewValueOptional"] {
            if INPUTS[index].contains(&flag) {
                return given(flag).is_none_or(|v| v == &Value::Bool(false) || v.as_i64() == Some(0));
            }
        }
        return false;
    }
    // A getter that makes what it gets: an envelope asked to be made (`create`), a container macro mapped or taken off by
    // reading its key. Those are changes (api-call): a read has no undo point and no HISTORY row.
    if given("create").is_some_and(|v| v == &Value::Bool(true) || v.as_i64().is_some_and(|n| n != 0)) {
        return false;
    }
    if name.ends_with("_GetNamedConfigParm")
        && given("parmname")
            .and_then(Value::as_str)
            .is_some_and(|key| key.contains("container_map.add") || key.contains("container_map.delete"))
    {
        return false;
    }
    const READS: [&str; 21] = [
        "Get",
        "Count",
        "Enum",
        "Is",
        "Has",
        "Validate",
        "TimeMap",
        "format_timestr",
        "parse_timestr",
        "Master_Get",
        "CSurf_TrackToID",
        "CSurf_TrackFromID",
        "TrackFX_Get",
        "TakeFX_Get",
        "TrackFX_Format",
        "TakeFX_Format",
        "MIDI_Get",
        "MIDI_Count",
        "MIDI_Enum",
        "Envelope_Evaluate",
        "Envelope_Format",
    ];
    READS.iter().any(|prefix| name.starts_with(prefix))
        || matches!(name, "file_exists" | "time_precise" | "Undo_CanUndo2" | "Undo_CanRedo2")
}

impl Reaper<'_> {
    /// Calls function `index` with `args` (or only checks the arguments, `dry`): what it answered, and what it wrote back.
    pub fn call(&self, index: usize, args: &Value, handles: &dyn Handles, dry: bool) -> Result<(Value, Map<String, Value>), String> {
        if let Some(why) = UNCALLABLE[index] {
            return Err(format!("{} can't be called by name: {why} (run-lua can call it)", name_of(index)));
        }
        if !dry && !reads_only(index, args) {
            super::wrote();
        }
        let mut call = Call::new(args, handles, dry);
        let result = dispatch(self.api, index, &mut call)?;
        Ok((result, call.outs()))
    }

    /// Runs a ReaScript (Lua) once, now, on the main thread: the text is written to a new file of its own (an unguessable
    /// name in the bridge's owner-only folder, made new, never through a file or link already there), registered as an
    /// action for as long as it runs (in memory only: REAPER's key map file isn't written), run, and taken away.
    pub fn run_script(&self, proj: Project, text: &str) -> Result<(), String> {
        let (Some(register), Some(run)) = (self.api.AddRemoveReaScript(), self.api.Main_OnCommandEx()) else {
            return Err("this REAPER can't run scripts".into());
        };
        let path = script_file(text)?;
        let file = CString::new(path.to_string_lossy().into_owned()).map_err(|_| "a temporary path with a NUL in it".to_string())?;
        let id = unsafe { register(true, 0, file.as_ptr(), false) };
        let result = if id <= 0 {
            Err("REAPER wouldn't take the chunk as a script".to_string())
        } else {
            unsafe { run(id, 0, proj.0) };
            unsafe { register(false, 0, file.as_ptr(), false) };
            Ok(())
        };
        let _ = std::fs::remove_file(&path);
        result
    }

    /// What a script left for the extension (`SetExtState(section, key, …, false)`), taken away as it is read.
    pub fn take_ext_state(&self, section: &str, key: &str) -> Option<String> {
        let (section, key) = (CString::new(section).ok()?, CString::new(key).ok()?);
        let text = self.api.GetExtState().map(|f| unsafe { api::string(f(section.as_ptr(), key.as_ptr())) })?;
        if let Some(delete) = self.api.DeleteExtState() {
            unsafe { delete(section.as_ptr(), key.as_ptr(), false) };
        }
        (!text.is_empty()).then_some(text)
    }
}

/// Where run-lua's scripts are written: the bridge's own folder in Kumi's (owner-only), set when the bridge starts serving.
static SCRIPTS: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

pub fn set_scripts_dir(dir: Option<std::path::PathBuf>) {
    *SCRIPTS.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// A new file holding `text`, owner-only, at a name no one can guess (another user can't make it first, or make Kumi write
/// through a link at it): in the bridge's folder, or the user's temporary folder before one is set.
fn script_file(text: &str) -> Result<std::path::PathBuf, String> {
    use std::hash::{BuildHasher, Hasher};
    use std::io::Write;
    let dir = SCRIPTS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(std::env::temp_dir);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for _ in 0..8 {
        // (a randomly keyed hash of the process, the run and the time: 64 bits no one outside the process can guess)
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        hasher.write_u64(RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        let path = dir.join(format!("kumi-run-{:016x}.lua", hasher.finish()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(text.as_bytes()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(format!("couldn't write the chunk to {}: {e}", path.display()));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("couldn't write the chunk to {}: {e}", path.display())),
        }
    }
    Err(format!("couldn't make a new file for the chunk in {}", dir.display()))
}

/// An object the bridge holds, as the pointer a generic call passes.
pub enum Handle {
    Track(super::Track),
    Item(super::Item),
    Take(super::Take),
    Envelope(super::Envelope),
}

pub fn pointer(handle: Handle) -> *mut c_void {
    match handle {
        Handle::Track(t) => t.0.cast(),
        Handle::Item(i) => i.0.cast(),
        Handle::Take(t) => t.0.cast(),
        Handle::Envelope(e) => e.0.cast(),
    }
}

impl Reaper<'_> {
    /// The ref of an object a call gave back (checked first: a pointer REAPER doesn't know is no object).
    pub fn ref_of(&self, proj: Project, base: &str, p: *mut c_void) -> Value {
        use crate::refs::{self, EnvelopeId};
        let unknown = || Value::String(format!("a {base} Kumi can't name"));
        match base {
            "MediaTrack" => {
                let track = super::Track(p.cast());
                if !self.track_valid(proj, track) && self.master_track(proj) != Some(track) {
                    return unknown();
                }
                Value::String(refs::track_ref(&crate::rows::track_id(self, track)))
            }
            "MediaItem" => {
                let item = super::Item(p.cast());
                if !self.item_valid(proj, item) {
                    return unknown();
                }
                Value::String(refs::item_ref(&self.item_guid(item)))
            }
            "MediaItem_Take" => {
                let take = super::Take(p.cast());
                if !self.take_valid(proj, take) {
                    return unknown();
                }
                Value::String(refs::take_ref(&self.take_guid(take)))
            }
            "TrackEnvelope" => {
                let envelope = super::Envelope(p.cast());
                if !self.envelope_valid(proj, envelope) {
                    return unknown();
                }
                match (self.envelope_owner(envelope), self.envelope_guid(envelope)) {
                    (Some((track, _, _)), Some(guid)) => {
                        Value::String(refs::envelope_ref(&crate::rows::track_id(self, track), &EnvelopeId::Guid(guid)))
                    }
                    _ => serde_json::json!({"envelope": self.envelope_name(envelope)}),
                }
            }
            _ => Value::String(if p == proj.0.cast() { "current".into() } else { "another project".into() }),
        }
    }
}

/// A track, item, take or envelope REAPER gave back, as the pointer it is (for `Handles::name`).
pub fn pointer_kind(base: &str) -> &'static str {
    match base {
        "MediaTrack" => "MediaTrack*",
        "MediaItem" => "MediaItem*",
        "MediaItem_Take" => "MediaItem_Take*",
        "TrackEnvelope" => "TrackEnvelope*",
        _ => "ReaProject*",
    }
}
