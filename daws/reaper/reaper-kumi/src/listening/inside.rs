//! A timer of Kumi's own for the length of a render: REAPER doesn't call an extension's timer while its render runs inside
//! that timer's call (measured in 7.82), but its render loop keeps the main thread's events going. The timer is the
//! platform's own (a CoreFoundation run-loop timer in the common modes on macOS, a thread timer on Windows) and calls
//! `fire` on the main thread, inside REAPER's render loop, every 50 ms. Elsewhere there is none.

use std::cell::Cell;
use std::ffi::c_void;

/// What a firing calls: its data, and the function given it.
type Call = (*const c_void, fn(*const c_void));

thread_local! {
    /// What a firing calls, while a timer is running (main thread only).
    static CALL: Cell<Option<Call>> = const { Cell::new(None) };
}

/// Calls the registered function, if a timer is running.
#[cfg_attr(test, allow(dead_code))]
fn fire() {
    if let Some((data, call)) = CALL.with(Cell::get) {
        call(data);
    }
}

/// A running timer, stopped when dropped.
pub struct Timer {
    #[allow(dead_code)]
    handle: platform::Handle,
}

impl Timer {
    /// Starts calling `call(data)` every 50 ms from the main thread's event loop, until dropped. `data` must outlive the
    /// timer. None where the platform has no such timer.
    pub fn start(data: *const c_void, call: fn(*const c_void)) -> Option<Timer> {
        CALL.with(|c| c.set(Some((data, call))));
        match platform::start() {
            Some(handle) => Some(Timer { handle }),
            None => {
                CALL.with(|c| c.set(None));
                None
            }
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        platform::stop(&self.handle);
        CALL.with(|c| c.set(None));
    }
}

/// Tests have no event loop: the fake's render calls the extension's tick itself.
#[cfg(test)]
mod platform {
    pub struct Handle;

    pub fn start() -> Option<Handle> {
        None
    }

    pub fn stop(_: &Handle) {}
}

#[cfg(all(not(test), target_os = "macos"))]
mod platform {
    use std::ffi::c_void;

    type Ref = *mut c_void;

    #[repr(C)]
    struct Context {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopCommonModes: Ref;
        fn CFRunLoopGetMain() -> Ref;
        fn CFAbsoluteTimeGetCurrent() -> f64;
        fn CFRunLoopTimerCreate(
            allocator: Ref,
            fire_date: f64,
            interval: f64,
            flags: usize,
            order: isize,
            callout: extern "C" fn(Ref, *mut c_void),
            context: *mut Context,
        ) -> Ref;
        fn CFRunLoopAddTimer(run_loop: Ref, timer: Ref, mode: Ref);
        fn CFRunLoopTimerInvalidate(timer: Ref);
        fn CFRelease(object: Ref);
    }

    pub struct Handle(Ref);

    extern "C" fn fired(_timer: Ref, _info: *mut c_void) {
        crate::guard("render timer", (), super::fire);
    }

    pub fn start() -> Option<Handle> {
        let mut context = Context {
            version: 0,
            info: std::ptr::null_mut(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        let timer =
            unsafe { CFRunLoopTimerCreate(std::ptr::null_mut(), CFAbsoluteTimeGetCurrent() + 0.05, 0.05, 0, 0, fired, &mut context) };
        if timer.is_null() {
            return None;
        }
        unsafe { CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes) };
        Some(Handle(timer))
    }

    pub fn stop(handle: &Handle) {
        unsafe {
            CFRunLoopTimerInvalidate(handle.0);
            CFRelease(handle.0);
        }
    }
}

#[cfg(all(not(test), windows))]
mod platform {
    use std::ffi::c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetTimer(hwnd: *mut c_void, id: usize, ms: u32, callback: Option<extern "system" fn(*mut c_void, u32, usize, u32)>) -> usize;
        fn KillTimer(hwnd: *mut c_void, id: usize) -> i32;
    }

    pub struct Handle(usize);

    extern "system" fn fired(_hwnd: *mut c_void, _msg: u32, _id: usize, _time: u32) {
        crate::guard("render timer", (), super::fire);
    }

    pub fn start() -> Option<Handle> {
        let id = unsafe { SetTimer(std::ptr::null_mut(), 0, 50, Some(fired)) };
        (id != 0).then_some(Handle(id))
    }

    pub fn stop(handle: &Handle) {
        unsafe { KillTimer(std::ptr::null_mut(), handle.0) };
    }
}

#[cfg(all(not(test), not(windows), not(target_os = "macos")))]
mod platform {
    pub struct Handle;

    pub fn start() -> Option<Handle> {
        None
    }

    pub fn stop(_: &Handle) {}
}
