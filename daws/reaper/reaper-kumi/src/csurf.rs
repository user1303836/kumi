//! The Rust half of the hidden control surface (shim/csurf.cpp): REAPER's callbacks arrive here as `Call`s.

use std::ffi::{c_char, c_int, c_void};

use crate::api::{self, MediaTrack};

/// One control surface callback, as REAPER made it (on the main thread).
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Run,
    TrackListChange,
    Volume(*mut MediaTrack, f64),
    Pan(*mut MediaTrack, f64),
    Mute(*mut MediaTrack, bool),
    Selected(*mut MediaTrack, bool),
    Solo(*mut MediaTrack, bool),
    RecArm(*mut MediaTrack, bool),
    PlayState { play: bool, pause: bool, rec: bool },
    Repeat(bool),
    TrackTitle(*mut MediaTrack, String),
    AutoMode(i32),
    ResetCachedVolPan,
    TrackSelection(*mut MediaTrack),
    Extended { call: i32, parm1: *mut c_void, parm2: *mut c_void, parm3: *mut c_void },
}

/// Where calls go. Returns what `Extended` answers (0: not handled).
pub type Handler = fn(Call) -> i32;

#[repr(C)]
struct Callbacks {
    ctx: *mut c_void,
    run: extern "C" fn(*mut c_void),
    set_track_list_change: extern "C" fn(*mut c_void),
    set_surface_volume: extern "C" fn(*mut c_void, *mut MediaTrack, f64),
    set_surface_pan: extern "C" fn(*mut c_void, *mut MediaTrack, f64),
    set_surface_mute: extern "C" fn(*mut c_void, *mut MediaTrack, bool),
    set_surface_selected: extern "C" fn(*mut c_void, *mut MediaTrack, bool),
    set_surface_solo: extern "C" fn(*mut c_void, *mut MediaTrack, bool),
    set_surface_rec_arm: extern "C" fn(*mut c_void, *mut MediaTrack, bool),
    set_play_state: extern "C" fn(*mut c_void, bool, bool, bool),
    set_repeat_state: extern "C" fn(*mut c_void, bool),
    set_track_title: extern "C" fn(*mut c_void, *mut MediaTrack, *const c_char),
    set_auto_mode: extern "C" fn(*mut c_void, c_int),
    reset_cached_vol_pan_states: extern "C" fn(*mut c_void),
    on_track_selection: extern "C" fn(*mut c_void, *mut MediaTrack),
    extended: extern "C" fn(*mut c_void, c_int, *mut c_void, *mut c_void, *mut c_void) -> c_int,
}

extern "C" {
    fn kumi_csurf_new(callbacks: *const Callbacks) -> *mut api::IReaperControlSurface;
    fn kumi_csurf_delete(surface: *mut api::IReaperControlSurface);
    #[cfg(test)]
    fn kumi_csurf_test_drive(surface: *mut api::IReaperControlSurface, track: *mut MediaTrack);
}

/// A control surface instance; deleted on drop. Register it with `csurf_inst` and unregister before dropping.
pub struct Surface {
    ptr: *mut api::IReaperControlSurface,
}

impl Surface {
    pub fn new(handler: Handler) -> Option<Surface> {
        let callbacks = Callbacks {
            ctx: handler as *mut c_void,
            run,
            set_track_list_change,
            set_surface_volume,
            set_surface_pan,
            set_surface_mute,
            set_surface_selected,
            set_surface_solo,
            set_surface_rec_arm,
            set_play_state,
            set_repeat_state,
            set_track_title,
            set_auto_mode,
            reset_cached_vol_pan_states,
            on_track_selection,
            extended,
        };
        let ptr = unsafe { kumi_csurf_new(&callbacks) };
        (!ptr.is_null()).then_some(Surface { ptr })
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.ptr.cast()
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { kumi_csurf_delete(self.ptr) }
    }
}

fn forward(ctx: *mut c_void, call: Call) -> c_int {
    // SAFETY: ctx is the Handler that Surface::new stored.
    let handler: Handler = unsafe { std::mem::transmute::<*mut c_void, Handler>(ctx) };
    crate::guard("control surface", 0, || handler(call))
}

extern "C" fn run(ctx: *mut c_void) {
    forward(ctx, Call::Run);
}
extern "C" fn set_track_list_change(ctx: *mut c_void) {
    forward(ctx, Call::TrackListChange);
}
extern "C" fn set_surface_volume(ctx: *mut c_void, track: *mut MediaTrack, volume: f64) {
    forward(ctx, Call::Volume(track, volume));
}
extern "C" fn set_surface_pan(ctx: *mut c_void, track: *mut MediaTrack, pan: f64) {
    forward(ctx, Call::Pan(track, pan));
}
extern "C" fn set_surface_mute(ctx: *mut c_void, track: *mut MediaTrack, mute: bool) {
    forward(ctx, Call::Mute(track, mute));
}
extern "C" fn set_surface_selected(ctx: *mut c_void, track: *mut MediaTrack, selected: bool) {
    forward(ctx, Call::Selected(track, selected));
}
extern "C" fn set_surface_solo(ctx: *mut c_void, track: *mut MediaTrack, solo: bool) {
    forward(ctx, Call::Solo(track, solo));
}
extern "C" fn set_surface_rec_arm(ctx: *mut c_void, track: *mut MediaTrack, armed: bool) {
    forward(ctx, Call::RecArm(track, armed));
}
extern "C" fn set_play_state(ctx: *mut c_void, play: bool, pause: bool, rec: bool) {
    forward(ctx, Call::PlayState { play, pause, rec });
}
extern "C" fn set_repeat_state(ctx: *mut c_void, repeat: bool) {
    forward(ctx, Call::Repeat(repeat));
}
extern "C" fn set_track_title(ctx: *mut c_void, track: *mut MediaTrack, title: *const c_char) {
    let title = crate::guard("control surface", String::new(), || unsafe { api::string(title) });
    forward(ctx, Call::TrackTitle(track, title));
}
extern "C" fn set_auto_mode(ctx: *mut c_void, mode: c_int) {
    forward(ctx, Call::AutoMode(mode));
}
extern "C" fn reset_cached_vol_pan_states(ctx: *mut c_void) {
    forward(ctx, Call::ResetCachedVolPan);
}
extern "C" fn on_track_selection(ctx: *mut c_void, track: *mut MediaTrack) {
    forward(ctx, Call::TrackSelection(track));
}
extern "C" fn extended(ctx: *mut c_void, call: c_int, parm1: *mut c_void, parm2: *mut c_void, parm3: *mut c_void) -> c_int {
    forward(ctx, Call::Extended { call, parm1, parm2, parm3 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static SEEN: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
    }

    fn record(call: Call) -> i32 {
        let panics = matches!(call, Call::Repeat(_));
        SEEN.with(|s| s.borrow_mut().push(call));
        if panics {
            panic!("a handler bug");
        }
        0
    }

    #[test]
    fn calls_through_the_vtable_reach_rust_in_order() {
        let surface = Surface::new(record).unwrap();
        let track = 0x1234 as *mut MediaTrack;
        unsafe { kumi_csurf_test_drive(surface.ptr, track) };
        let seen = SEEN.with(|s| s.take());
        assert_eq!(
            seen,
            [
                Call::Run,
                Call::TrackListChange,
                Call::Volume(track, 0.5),
                Call::Selected(track, true),
                Call::PlayState { play: true, pause: false, rec: true },
                Call::TrackTitle(track, "Bass".into()),
                Call::TrackSelection(track),
                Call::Extended {
                    call: api::CSURF_EXT_SETFXCHANGE,
                    parm1: track.cast(),
                    parm2: std::ptr::null_mut(),
                    parm3: std::ptr::null_mut()
                },
            ]
        );
    }

    #[test]
    fn a_panicking_handler_stays_on_the_rust_side() {
        // What REAPER's call reaches: the extern "C" callback, with the handler as its context.
        set_repeat_state(record as Handler as *mut c_void, true);
        assert_eq!(SEEN.with(|s| s.take()), [Call::Repeat(true)]);
    }
}
