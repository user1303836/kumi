//! The REAPER calls for FX in any chain (a track's main or input chain, a take's, a container's) and for envelopes,
//! their points and automation items (M4). Each call is REAPER's own, one for one; a function this REAPER lacks answers
//! false or None, and the change that needed it is refused.

use std::ffi::{c_char, c_int};

use super::ctext;
use super::{Envelope, Fx, FxOwner, Reaper, Take, Track};
use crate::api;

/// The most an FX's state or an envelope's chunk may be when it is read.
const MAX_STATE: usize = 64 << 20;

/// An envelope point as REAPER gives it: its time (seconds; a take envelope's from the item's start), its value in the
/// envelope's own scale, its shape (0 linear, 1 square, 2 slow start/end, 3 fast start, 4 fast end, 5 bezier), the bezier's
/// tension and whether it is selected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvPoint {
    pub time: f64,
    pub value: f64,
    pub shape: i32,
    pub tension: f64,
    pub selected: bool,
}

/// What `TrackFX_GetEQParam` says of a ReaEQ parameter: its band's type (REAPER's numbering: 0 high pass, 1 low shelf,
/// 2 band, 3 notch, 4 high shelf, 5 low pass, 6 band pass, 7 parallel band pass; -1 for the plug-in's own parameters),
/// which band of that type, and what it is (0 frequency, 1 gain, 2 bandwidth).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqParam {
    pub band_type: i32,
    pub band: i32,
    pub kind: i32,
}

/// Reads a string through a getter that fills a buffer of a size, growing the buffer while what it wrote fills it.
fn grown(mut fill: impl FnMut(*mut c_char, c_int) -> bool) -> Option<String> {
    let mut size = 1usize << 16;
    loop {
        let mut buf = vec![0u8; size];
        if !fill(buf.as_mut_ptr().cast(), size as c_int) {
            return None;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(size);
        if len + 1 < size {
            buf.truncate(len);
            return Some(String::from_utf8_lossy(&buf).into_owned());
        }
        if size >= MAX_STATE {
            return None;
        }
        size *= 4;
    }
}

/// FX in any chain. `Fx::index` is REAPER's address: the place, 0x1000000 for a track's input chain, 0x2000000 for a
/// container's child.
impl Reaper<'_> {
    /// Adds the plug-in or FX chain file `name` to a chain at `place` (REAPER's `-1000 - place`; for a track's input chain
    /// the place without its flag), or at the end; what REAPER answers (the place in the chain it went to, or -1).
    pub fn fx_add(&self, owner: FxOwner, name: &str, input: bool, place: Option<i32>) -> i32 {
        let name = ctext(name);
        let instantiate = place.map_or(-1, |p| -1000 - p);
        match owner {
            FxOwner::Track(t) => self.api.TrackFX_AddByName().map_or(-1, |f| unsafe { f(t.0, name.as_ptr(), input, instantiate) }),
            FxOwner::Take(t) => self.api.TakeFX_AddByName().map_or(-1, |f| unsafe { f(t.0, name.as_ptr(), instantiate) }),
        }
    }

    pub fn fx_delete(&self, fx: Fx) -> bool {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_Delete().is_some_and(|f| unsafe { f(t.0, fx.index) }),
            FxOwner::Take(t) => self.api.TakeFX_Delete().is_some_and(|f| unsafe { f(t.0, fx.index) }),
        }
    }

    /// Copies (or moves) an FX to `to`'s chain at the address `index` (REAPER's: the place it ends up at).
    pub fn fx_copy(&self, from: Fx, to: FxOwner, index: i32, is_move: bool) -> bool {
        unsafe {
            match (from.owner, to) {
                (FxOwner::Track(s), FxOwner::Track(d)) => self.api.TrackFX_CopyToTrack().map(|f| f(s.0, from.index, d.0, index, is_move)),
                (FxOwner::Track(s), FxOwner::Take(d)) => self.api.TrackFX_CopyToTake().map(|f| f(s.0, from.index, d.0, index, is_move)),
                (FxOwner::Take(s), FxOwner::Track(d)) => self.api.TakeFX_CopyToTrack().map(|f| f(s.0, from.index, d.0, index, is_move)),
                (FxOwner::Take(s), FxOwner::Take(d)) => self.api.TakeFX_CopyToTake().map(|f| f(s.0, from.index, d.0, index, is_move)),
            }
        }
        .is_some()
    }

    pub fn fx_set_enabled(&self, fx: Fx, enabled: bool) -> bool {
        unsafe {
            match fx.owner {
                FxOwner::Track(t) => self.api.TrackFX_SetEnabled().map(|f| f(t.0, fx.index, enabled)),
                FxOwner::Take(t) => self.api.TakeFX_SetEnabled().map(|f| f(t.0, fx.index, enabled)),
            }
        }
        .is_some()
    }

    pub fn fx_set_offline(&self, fx: Fx, offline: bool) -> bool {
        unsafe {
            match fx.owner {
                FxOwner::Track(t) => self.api.TrackFX_SetOffline().map(|f| f(t.0, fx.index, offline)),
                FxOwner::Take(t) => self.api.TakeFX_SetOffline().map(|f| f(t.0, fx.index, offline)),
            }
        }
        .is_some()
    }

    /// A parameter on its 0 to 1 scale.
    pub fn fx_set_param(&self, fx: Fx, param: i32, normalized: f64) -> bool {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_SetParamNormalized().is_some_and(|f| unsafe { f(t.0, fx.index, param, normalized) }),
            FxOwner::Take(t) => self.api.TakeFX_SetParamNormalized().is_some_and(|f| unsafe { f(t.0, fx.index, param, normalized) }),
        }
    }

    /// What the plug-in would print for a normalized value, without setting it.
    pub fn fx_format(&self, fx: Fx, param: i32, normalized: f64) -> Option<String> {
        let mut ok = false;
        let text = match fx.owner {
            FxOwner::Track(t) => {
                let f = self.api.TrackFX_FormatParamValueNormalized()?;
                api::read_buffer(|buf, size| ok = unsafe { f(t.0, fx.index, param, normalized, buf, size) })
            }
            FxOwner::Take(t) => {
                let f = self.api.TakeFX_FormatParamValueNormalized()?;
                api::read_buffer(|buf, size| ok = unsafe { f(t.0, fx.index, param, normalized, buf, size) })
            }
        };
        ok.then_some(text)
    }

    /// A parameter in the plug-in's own units (a JSFX slider's value), with its range: `(value, min, max)`.
    pub fn fx_param_raw(&self, fx: Fx, param: i32) -> Option<(f64, f64, f64)> {
        let (mut min, mut max) = (0.0, 0.0);
        let value = match fx.owner {
            FxOwner::Track(t) => unsafe { self.api.TrackFX_GetParam()?(t.0, fx.index, param, &mut min, &mut max) },
            FxOwner::Take(t) => unsafe { self.api.TakeFX_GetParam()?(t.0, fx.index, param, &mut min, &mut max) },
        };
        Some((value, min, max))
    }

    pub fn fx_set_param_raw(&self, fx: Fx, param: i32, value: f64) -> bool {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_SetParam().is_some_and(|f| unsafe { f(t.0, fx.index, param, value) }),
            FxOwner::Take(t) => self.api.TakeFX_SetParam().is_some_and(|f| unsafe { f(t.0, fx.index, param, value) }),
        }
    }

    /// A named configuration value written. What REAPER answers is not always whether it took (ReaEQ's `BANDTYPEn` answers
    /// false and takes): the change reads it back.
    pub fn fx_set_config(&self, fx: Fx, key: &str, value: &str) -> bool {
        let (key, value) = (ctext(key), ctext(value));
        match fx.owner {
            FxOwner::Track(t) => {
                self.api.TrackFX_SetNamedConfigParm().is_some_and(|f| unsafe { f(t.0, fx.index, key.as_ptr(), value.as_ptr()) })
            }
            FxOwner::Take(t) => {
                self.api.TakeFX_SetNamedConfigParm().is_some_and(|f| unsafe { f(t.0, fx.index, key.as_ptr(), value.as_ptr()) })
            }
        }
    }

    /// A named configuration value that can be big (a plug-in's state, `vst_chunk`): None when the FX has none.
    pub fn fx_config_big(&self, fx: Fx, key: &str) -> Option<String> {
        let key = ctext(key);
        match fx.owner {
            FxOwner::Track(t) => {
                let f = self.api.TrackFX_GetNamedConfigParm()?;
                grown(|buf, size| unsafe { f(t.0, fx.index, key.as_ptr(), buf, size) })
            }
            FxOwner::Take(t) => {
                let f = self.api.TakeFX_GetNamedConfigParm()?;
                grown(|buf, size| unsafe { f(t.0, fx.index, key.as_ptr(), buf, size) })
            }
        }
    }

    /// The parameter a stable ident names (`:wet`, `:delta`, `:bypass`, `0:_Threshold`).
    pub fn fx_ident(&self, fx: Fx, ident: &str) -> Option<i32> {
        let ident = ctext(ident);
        let index = match fx.owner {
            FxOwner::Track(t) => unsafe { self.api.TrackFX_GetParamFromIdent()?(t.0, fx.index, ident.as_ptr()) },
            FxOwner::Take(t) => unsafe { self.api.TakeFX_GetParamFromIdent()?(t.0, fx.index, ident.as_ptr()) },
        };
        (index >= 0).then_some(index)
    }

    /// The preset loaded (-1: none) and how many the plug-in has.
    pub fn fx_preset_index(&self, fx: Fx) -> Option<(i32, i32)> {
        let mut count = 0;
        let index = match fx.owner {
            FxOwner::Track(t) => unsafe { self.api.TrackFX_GetPresetIndex()?(t.0, fx.index, &mut count) },
            FxOwner::Take(t) => unsafe { self.api.TakeFX_GetPresetIndex()?(t.0, fx.index, &mut count) },
        };
        Some((index, count))
    }

    pub fn fx_set_preset(&self, fx: Fx, name: &str) -> bool {
        let name = ctext(name);
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_SetPreset().is_some_and(|f| unsafe { f(t.0, fx.index, name.as_ptr()) }),
            FxOwner::Take(t) => self.api.TakeFX_SetPreset().is_some_and(|f| unsafe { f(t.0, fx.index, name.as_ptr()) }),
        }
    }

    /// A preset by its place; -2 is the plug-in's factory state, -1 its default user preset.
    pub fn fx_set_preset_index(&self, fx: Fx, index: i32) -> bool {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_SetPresetByIndex().is_some_and(|f| unsafe { f(t.0, fx.index, index) }),
            FxOwner::Take(t) => self.api.TakeFX_SetPresetByIndex().is_some_and(|f| unsafe { f(t.0, fx.index, index) }),
        }
    }

    /// The next (1) or previous (-1) preset.
    pub fn fx_step_preset(&self, fx: Fx, step: i32) -> bool {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_NavigatePresets().is_some_and(|f| unsafe { f(t.0, fx.index, step) }),
            FxOwner::Take(t) => self.api.TakeFX_NavigatePresets().is_some_and(|f| unsafe { f(t.0, fx.index, step) }),
        }
    }

    /// What a ReaEQ parameter is (a track's ReaEQ only: REAPER has no such call for a take's).
    pub fn fx_eq_param(&self, fx: Fx, param: i32) -> Option<EqParam> {
        let FxOwner::Track(t) = fx.owner else { return None };
        let f = self.api.TrackFX_GetEQParam()?;
        let (mut band_type, mut band, mut kind, mut normalized) = (0, 0, 0, 0.0);
        let ok = unsafe { f(t.0, fx.index, param, &mut band_type, &mut band, &mut kind, &mut normalized) };
        ok.then_some(EqParam { band_type, band, kind })
    }

    /// An FX parameter's envelope; made when `create` (REAPER makes one with a point at the parameter's value).
    pub fn fx_envelope(&self, fx: Fx, param: i32, create: bool) -> Option<Envelope> {
        let envelope = match fx.owner {
            FxOwner::Track(t) => unsafe { self.api.GetFXEnvelope()?(t.0, fx.index, param, create) },
            FxOwner::Take(t) => unsafe { self.api.TakeFX_GetEnvelope()?(t.0, fx.index, param, create) },
        };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    // (the item chunk, the item selection and the active take: m4-r-items' wrappers in `reaper/items.rs`)
}

/// Envelopes, their points and automation items. `item` is an automation item's index, or -1 for the envelope itself.
impl Reaper<'_> {
    pub fn env_point_count(&self, envelope: Envelope, item: i32) -> i32 {
        self.api.CountEnvelopePointsEx().map_or(0, |f| unsafe { f(envelope.0, item) })
    }

    pub fn env_point(&self, envelope: Envelope, item: i32, index: i32) -> Option<EnvPoint> {
        let f = self.api.GetEnvelopePointEx()?;
        let (mut time, mut value, mut shape, mut tension, mut selected) = (0.0, 0.0, 0, 0.0, false);
        let ok = unsafe { f(envelope.0, item, index, &mut time, &mut value, &mut shape, &mut tension, &mut selected) };
        ok.then_some(EnvPoint { time, value, shape, tension, selected })
    }

    /// Adds a point; with `no_sort` the envelope is sorted later, once, by the caller.
    pub fn env_insert_point(&self, envelope: Envelope, item: i32, point: EnvPoint, no_sort: bool) -> bool {
        let mut no_sort = no_sort;
        self.api.InsertEnvelopePointEx().is_some_and(|f| unsafe {
            f(envelope.0, item, point.time, point.value, point.shape, point.tension, point.selected, &mut no_sort)
        })
    }

    pub fn env_set_point(&self, envelope: Envelope, item: i32, index: i32, point: EnvPoint, no_sort: bool) -> bool {
        let EnvPoint { mut time, mut value, mut shape, mut tension, mut selected } = point;
        let mut no_sort = no_sort;
        self.api.SetEnvelopePointEx().is_some_and(|f| unsafe {
            f(envelope.0, item, index, &mut time, &mut value, &mut shape, &mut tension, &mut selected, &mut no_sort)
        })
    }

    pub fn env_delete_point(&self, envelope: Envelope, item: i32, index: i32) -> bool {
        self.api.DeleteEnvelopePointEx().is_some_and(|f| unsafe { f(envelope.0, item, index) })
    }

    /// Deletes the points from `start` to just before `end`.
    pub fn env_delete_range(&self, envelope: Envelope, item: i32, start: f64, end: f64) -> bool {
        self.api.DeleteEnvelopePointRangeEx().is_some_and(|f| unsafe { f(envelope.0, item, start, end) })
    }

    /// Sorts the points by time. REAPER counts this as an envelope edit, so a block around it makes an undo point that
    /// holds the envelope (what `GetFXEnvelope(…, create)` and a chunk write alone don't).
    pub fn env_sort(&self, envelope: Envelope, item: i32) -> bool {
        self.api.Envelope_SortPointsEx().is_some_and(|f| unsafe { f(envelope.0, item) })
    }

    /// 0 for a plain envelope, 1 for one in fader scaling (volume).
    pub fn env_scaling(&self, envelope: Envelope) -> i32 {
        self.api.GetEnvelopeScalingMode().map_or(0, |f| unsafe { f(envelope.0) })
    }

    pub fn scale_to_envelope(&self, mode: i32, value: f64) -> f64 {
        self.api.ScaleToEnvelopeMode().map_or(value, |f| unsafe { f(mode, value) })
    }

    pub fn scale_from_envelope(&self, mode: i32, value: f64) -> f64 {
        self.api.ScaleFromEnvelopeMode().map_or(value, |f| unsafe { f(mode, value) })
    }

    /// A value as the envelope shows it ("-6.00dB", "50%L", "+1.0000 semitones").
    pub fn env_format(&self, envelope: Envelope, value: f64) -> String {
        match self.api.Envelope_FormatValue() {
            Some(f) => api::read_buffer(|buf, size| unsafe { f(envelope.0, value, buf, size) }),
            None => String::new(),
        }
    }

    pub fn env_chunk(&self, envelope: Envelope) -> Option<String> {
        let f = self.api.GetEnvelopeStateChunk()?;
        grown(|buf, size| unsafe { f(envelope.0, buf, size, false) })
    }

    pub fn set_env_chunk(&self, envelope: Envelope, chunk: &str) -> bool {
        super::wrote();
        let chunk = ctext(chunk);
        self.api.SetEnvelopeStateChunk().is_some_and(|f| unsafe { f(envelope.0, chunk.as_ptr(), false) })
    }

    /// An envelope's yes/no attribute written: `ACTIVE`, `VISIBLE` or `ARM`.
    pub fn set_env_flag(&self, envelope: Envelope, key: &str, on: bool) -> bool {
        super::wrote();
        let key = ctext(key);
        let Some(f) = self.api.GetSetEnvelopeInfo_String() else { return false };
        let mut buf = if on { *b"1\0" } else { *b"0\0" };
        unsafe { f(envelope.0, key.as_ptr(), buf.as_mut_ptr().cast(), true) }
    }

    pub fn track_envelope_by_name(&self, track: Track, name: &str) -> Option<Envelope> {
        let name = ctext(name);
        let envelope = unsafe { self.api.GetTrackEnvelopeByName()?(track.0, name.as_ptr()) };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    pub fn take_envelope_by_name(&self, take: Take, name: &str) -> Option<Envelope> {
        let name = ctext(name);
        let envelope = unsafe { self.api.GetTakeEnvelopeByName()?(take.0, name.as_ptr()) };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    /// The take an envelope is on, if it is a take's.
    pub fn envelope_take(&self, envelope: Envelope) -> Option<Take> {
        let f = self.api.GetEnvelopeInfo_Value()?;
        let pointer = unsafe { f(envelope.0, c"P_TAKE".as_ptr()) } as usize;
        (pointer != 0).then_some(Take(pointer as *mut api::MediaItem_Take))
    }

    pub fn ai_count(&self, envelope: Envelope) -> i32 {
        self.api.CountAutomationItems().map_or(0, |f| unsafe { f(envelope.0) })
    }

    /// An automation item's number attribute: `D_POOL_ID`, `D_POSITION`, `D_LENGTH`, `D_STARTOFFS`, `D_PLAYRATE`,
    /// `D_BASELINE`, `D_AMPLITUDE`, `D_LOOPSRC`, `D_MUTE`, `D_UISEL`.
    pub fn ai_value(&self, envelope: Envelope, item: i32, key: &str) -> f64 {
        let key = ctext(key);
        self.api.GetSetAutomationItemInfo().map_or(0.0, |f| unsafe { f(envelope.0, item, key.as_ptr(), 0.0, false) })
    }

    pub fn set_ai_value(&self, envelope: Envelope, item: i32, key: &str, value: f64) -> bool {
        super::wrote();
        let key = ctext(key);
        match self.api.GetSetAutomationItemInfo() {
            Some(f) => {
                unsafe { f(envelope.0, item, key.as_ptr(), value, true) };
                true
            }
            None => false,
        }
    }

    /// An automation item's text attribute: `P_POOL_NAME`.
    pub fn ai_text(&self, envelope: Envelope, item: i32, key: &str) -> Option<String> {
        let key = ctext(key);
        let f = self.api.GetSetAutomationItemInfo_String()?;
        super::objects::big_string(|buf| unsafe { f(envelope.0, item, key.as_ptr(), buf, false) })
    }

    /// A new automation item: of a new pool (`pool` -1: it takes the envelope's points in its range) or another instance of
    /// a pool. Its index, or -1.
    pub fn ai_insert(&self, envelope: Envelope, pool: i32, position: f64, length: f64) -> i32 {
        self.api.InsertAutomationItem().map_or(-1, |f| unsafe { f(envelope.0, pool, position, length) })
    }

    /// The global automation override: -1 none, 0 trim/read, 1 read, 2 touch, 3 write, 4 latch, 5 latch preview, 6 bypass.
    pub fn automation_override(&self) -> Option<i32> {
        self.api.GetGlobalAutomationOverride().map(|f| unsafe { f() })
    }

    pub fn set_automation_override(&self, mode: i32) -> bool {
        super::wrote();
        match self.api.SetGlobalAutomationOverride() {
            Some(f) => {
                unsafe { f(mode) };
                true
            }
            None => false,
        }
    }
}
