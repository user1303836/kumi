//! Whether REAPER is in a modal dialog. A dialog (a plug-in asking to authorize, a render's progress, a save prompt)
//! runs a modal loop of its own, from which REAPER's timers and control surface still call in: work started then would
//! run inside that dialog. Each platform asks its own way:
//! - Windows: user32's IsWindowEnabled on the main window, which a modal dialog disables.
//! - macOS: AppKit's `[NSApp modalWindow]`, which is set while a modal session runs (REAPER's SWELL exports no
//!   IsWindowEnabled and hands plug-ins no table of functions there; measured on 7.82).
//! - Linux: SWELL's IsWindowEnabled, from the table REAPER hands to `SWELL_dllMain`, else among REAPER's exports
//!   (not measured).

use std::ffi::c_void;

/// Whether the main window takes input; None when this platform can't say.
#[cfg(not(test))]
pub fn is_enabled(hwnd: *mut c_void) -> Option<bool> {
    platform::is_enabled(hwnd)
}

/// Tests ask the fake REAPER.
#[cfg(test)]
pub fn is_enabled(_hwnd: *mut c_void) -> Option<bool> {
    Some(crate::reaper::fake::with(|s| s.main_enabled))
}

/// The title of the window that has the keyboard focus, when this platform can say. REAPER's API names the focused FX
/// window and the MIDI editor but no other window; a floating window such as a track's routing and I/O is only known by
/// its title. (The routing matrix is a dock tab in 7.82, not a window of its own: nothing says whether it is in front.)
#[cfg(not(test))]
pub fn front_title() -> Option<String> {
    platform::front_title()
}

/// Tests ask the fake REAPER.
#[cfg(test)]
pub fn front_title() -> Option<String> {
    crate::reaper::fake::with(|s| {
        s.front_title_asked += 1;
        s.front_title.clone()
    })
}

/// What this platform asks, for the log.
#[cfg(not(test))]
pub fn describe() -> &'static str {
    platform::describe()
}

#[cfg(test)]
pub fn describe() -> &'static str {
    "the test's"
}

#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
mod platform {
    use std::ffi::c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn IsWindowEnabled(hwnd: *mut c_void) -> i32;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetWindowTextW(hwnd: *mut c_void, text: *mut u16, max: i32) -> i32;
    }

    pub fn is_enabled(hwnd: *mut c_void) -> Option<bool> {
        Some(unsafe { IsWindowEnabled(hwnd) } != 0)
    }

    /// The foreground window's title.
    pub fn front_title() -> Option<String> {
        let window = unsafe { GetForegroundWindow() };
        if window.is_null() {
            return None;
        }
        let mut text = [0u16; 512];
        let len = unsafe { GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) };
        (len > 0).then(|| String::from_utf16_lossy(&text[..len as usize]))
    }

    pub fn describe() -> &'static str {
        "user32 IsWindowEnabled on the main window"
    }
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod platform {
    use std::ffi::{c_char, c_void};

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    /// `[receiver selector]` for a method that takes nothing and returns an object.
    unsafe fn send(receiver: *mut c_void, selector: &std::ffi::CStr) -> *mut c_void {
        // SAFETY: objc_msgSend is called as the method's own type, which the callers know.
        let send = unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void>(objc_msgSend)
        };
        unsafe { send(receiver, sel_registerName(selector.as_ptr())) }
    }

    /// Whether the main window takes input: no modal session is running. (`[NSApp modalWindow]` is the window of the
    /// session `runModalForWindow:` runs, which is how NSAlert and the open and save panels show.)
    pub fn is_enabled(_hwnd: *mut c_void) -> Option<bool> {
        let class = unsafe { objc_getClass(c"NSApplication".as_ptr()) };
        if class.is_null() {
            return None;
        }
        let app = unsafe { send(class, c"sharedApplication") };
        if app.is_null() {
            return None;
        }
        Some(unsafe { send(app, c"modalWindow") }.is_null())
    }

    /// The title of `[NSApp keyWindow]`, the window that has the keyboard focus.
    pub fn front_title() -> Option<String> {
        let class = unsafe { objc_getClass(c"NSApplication".as_ptr()) };
        if class.is_null() {
            return None;
        }
        let app = unsafe { send(class, c"sharedApplication") };
        let key = if app.is_null() { app } else { unsafe { send(app, c"keyWindow") } };
        if key.is_null() {
            return None;
        }
        let title = unsafe { send(key, c"title") };
        let text = if title.is_null() { title } else { unsafe { send(title, c"UTF8String") } };
        // SAFETY: UTF8String is a NUL-terminated C string that lives as long as the NSString.
        (!text.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(text as *const c_char) }.to_string_lossy().into_owned())
    }

    pub fn describe() -> &'static str {
        "AppKit's modal window"
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
#[cfg_attr(test, allow(dead_code))]
mod platform {
    use std::ffi::{c_char, c_int, c_void};
    use std::sync::atomic::{AtomicPtr, Ordering};

    /// REAPER's `SWELLAPI_GetFunc`: name to function, kept from `SWELL_dllMain`.
    static GET_FUNC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    /// REAPER calls this on a module that exports it, when the module is loaded and unloaded, with the function that
    /// finds SWELL's calls by name (null from a REAPER that has none to give: the module is wanted all the same).
    ///
    /// # Safety
    /// Called by REAPER with its own `get_func` or null.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn SWELL_dllMain(_instance: *mut c_void, reason: u32, get_func: *mut c_void) -> c_int {
        const DLL_PROCESS_ATTACH: u32 = 1;
        const DLL_PROCESS_DETACH: u32 = 0;
        match reason {
            DLL_PROCESS_ATTACH => GET_FUNC.store(get_func, Ordering::Release),
            DLL_PROCESS_DETACH => GET_FUNC.store(std::ptr::null_mut(), Ordering::Release),
            _ => {}
        }
        1
    }

    type GetFunc = unsafe extern "C" fn(name: *const c_char) -> *mut c_void;
    type IsWindowEnabled = unsafe extern "C" fn(hwnd: *mut c_void) -> c_int;

    // (Older glibc keeps dlsym in libdl.)
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    /// A SWELL function by name: from the table REAPER handed over, else from REAPER's own exports.
    fn swell(name: &std::ffi::CStr) -> Option<*mut c_void> {
        let get_func = GET_FUNC.load(Ordering::Acquire);
        if !get_func.is_null() {
            // SAFETY: REAPER gave this function to SWELL_dllMain.
            let get_func = unsafe { std::mem::transmute::<*mut c_void, GetFunc>(get_func) };
            let function = unsafe { get_func(name.as_ptr()) };
            if !function.is_null() {
                return Some(function);
            }
        }
        // RTLD_DEFAULT is null on Linux.
        let function = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
        (!function.is_null()).then_some(function)
    }

    pub fn is_enabled(hwnd: *mut c_void) -> Option<bool> {
        let function = swell(c"IsWindowEnabled")?;
        // SAFETY: SWELL's IsWindowEnabled(HWND) -> BOOL.
        let function = unsafe { std::mem::transmute::<*mut c_void, IsWindowEnabled>(function) };
        Some(unsafe { function(hwnd) } != 0)
    }

    /// Not measured here: the focused window's title is not asked of SWELL.
    pub fn front_title() -> Option<String> {
        None
    }

    pub fn describe() -> &'static str {
        if swell(c"IsWindowEnabled").is_some() {
            "SWELL's IsWindowEnabled on the main window"
        } else {
            "nothing: REAPER gives this platform no SWELL IsWindowEnabled"
        }
    }
}
