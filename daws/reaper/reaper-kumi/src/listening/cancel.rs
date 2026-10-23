//! The render window's Cancel, pressed from a tick REAPER gives while its render runs (REAPER's render loop still calls
//! Kumi's own timer, see `inside`). Each platform finds the progress window ("Rendering to file…") its own way and clicks
//! its Cancel so that the caller returns first: AppKit's windows and a one-shot run-loop timer on macOS (measured), the
//! process's windows and a posted click on Windows (not measured there). REAPER then asks whether to keep the incomplete
//! files ("REAPER Render Incomplete!", unless its preference says): answered No. Elsewhere nothing is pressed, and the
//! render runs to its end (its files are then deleted).

/// Presses Cancel in REAPER's render window: false when no such window or button was found.
#[cfg(not(test))]
pub fn press() -> bool {
    platform::press()
}

/// Tests ask the fake REAPER.
#[cfg(test)]
pub fn press() -> bool {
    crate::reaper::fake::press_render_cancel()
}

/// REAPER's alert about the files a cancelled render left, answered "No" (Kumi deletes them itself): false when none is up.
#[cfg(not(test))]
pub fn answer_incomplete() -> bool {
    platform::answer_incomplete()
}

#[cfg(test)]
pub fn answer_incomplete() -> bool {
    false
}

/// The titles of REAPER's windows, for the log (what the render window is called where it couldn't be found).
#[cfg(not(test))]
pub fn titles() -> Vec<String> {
    platform::titles()
}

#[cfg(test)]
pub fn titles() -> Vec<String> {
    vec![]
}

/// Whether a window's title is the render's progress window's ("Rendering to file…"): never the render settings dialog
/// ("Render to File"), which REAPER keeps open behind it and whose Cancel would tear the render down with a prompt.
#[cfg_attr(test, allow(dead_code))]
fn is_render_window(title: &str) -> bool {
    title.to_lowercase().starts_with("rendering")
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod platform {
    use std::ffi::{c_char, c_void, CStr};

    type Id = *mut c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Id;
        fn objc_msgSend();
    }

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
        static kCFRunLoopCommonModes: Id;
        fn CFRunLoopGetMain() -> Id;
        fn CFAbsoluteTimeGetCurrent() -> f64;
        fn CFRunLoopTimerCreate(
            allocator: Id,
            fire: f64,
            interval: f64,
            flags: usize,
            order: isize,
            callout: extern "C" fn(Id, *mut c_void),
            context: *mut Context,
        ) -> Id;
        fn CFRunLoopAddTimer(run_loop: Id, timer: Id, mode: Id);
        fn CFRelease(object: Id);
    }

    fn sel(name: &CStr) -> Id {
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// `[receiver selector]` returning an object.
    unsafe fn object(receiver: Id, selector: &CStr) -> Id {
        let send = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Id) -> Id>(objc_msgSend) };
        unsafe { send(receiver, sel(selector)) }
    }

    /// `[receiver selector]` returning a BOOL.
    unsafe fn yes(receiver: Id, selector: &CStr) -> bool {
        let send = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Id) -> i8>(objc_msgSend) };
        unsafe { send(receiver, sel(selector)) != 0 }
    }

    /// `[receiver count]`.
    unsafe fn count(receiver: Id) -> usize {
        let send = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Id) -> usize>(objc_msgSend) };
        unsafe { send(receiver, sel(c"count")) }
    }

    /// `[receiver objectAtIndex:index]`.
    unsafe fn at(receiver: Id, index: usize) -> Id {
        let send = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Id, usize) -> Id>(objc_msgSend) };
        unsafe { send(receiver, sel(c"objectAtIndex:"), index) }
    }

    /// `[receiver isKindOfClass:class]`.
    unsafe fn is_kind(receiver: Id, class: Id) -> bool {
        let send = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Id, Id) -> i8>(objc_msgSend) };
        unsafe { send(receiver, sel(c"isKindOfClass:"), class) != 0 }
    }

    unsafe fn text(string: Id) -> String {
        if string.is_null() {
            return String::new();
        }
        let utf8 = unsafe { object(string, c"UTF8String") } as *const c_char;
        if utf8.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(utf8) }.to_string_lossy().into_owned()
        }
    }

    unsafe fn app() -> Id {
        let class = unsafe { objc_getClass(c"NSApplication".as_ptr()) };
        if class.is_null() {
            return class;
        }
        unsafe { object(class, c"sharedApplication") }
    }

    unsafe fn windows() -> Vec<Id> {
        let app = unsafe { app() };
        if app.is_null() {
            return vec![];
        }
        let list = unsafe { object(app, c"windows") };
        if list.is_null() {
            return vec![];
        }
        (0..unsafe { count(list) }).map(|i| unsafe { at(list, i) }).filter(|w| !w.is_null()).collect()
    }

    /// Every view under `view` that is of `class`, depth first.
    unsafe fn views_of(view: Id, class: Id, depth: usize, found: &mut Vec<Id>) {
        if view.is_null() || depth > 12 {
            return;
        }
        if unsafe { is_kind(view, class) } {
            found.push(view);
        }
        let subviews = unsafe { object(view, c"subviews") };
        if subviews.is_null() {
            return;
        }
        for i in 0..unsafe { count(subviews) } {
            unsafe { views_of(at(subviews, i), class, depth + 1, found) };
        }
    }

    unsafe fn buttons(window: Id) -> Vec<(Id, String)> {
        let class = unsafe { objc_getClass(c"NSButton".as_ptr()) };
        let mut found = Vec::new();
        if !class.is_null() {
            unsafe { views_of(object(window, c"contentView"), class, 0, &mut found) };
        }
        found.into_iter().map(|b| (b, unsafe { text(object(b, c"title")) })).collect()
    }

    /// A button a timer clicks: found again when the timer fires, never held until then (REAPER frees the progress window,
    /// and its button, when the render ends between the press and the timer).
    #[derive(Clone, Copy)]
    enum Click {
        RenderCancel = 1,
        IncompleteNo = 2,
    }

    /// Clicks a button from a timer of its own, so whatever the click opens (an alert's modal loop) runs outside the caller,
    /// and Kumi's render timer goes on firing inside it. The timer holds which button, not the button: it looks it up again,
    /// and clicks nothing when it's gone.
    fn click_soon(click: Click) {
        extern "C" fn fired(_timer: Id, info: *mut c_void) {
            crate::guard("render cancel", (), || {
                let button = match info as usize {
                    1 => render_cancel(),
                    2 => incomplete_no(),
                    _ => None,
                };
                if let Some(button) = button {
                    let send = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Id, Id)>(objc_msgSend) };
                    unsafe { send(button, sel(c"performClick:"), std::ptr::null_mut()) };
                }
            });
        }
        let mut context = Context {
            version: 0,
            info: click as usize as *mut c_void,
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        let timer = unsafe { CFRunLoopTimerCreate(std::ptr::null_mut(), CFAbsoluteTimeGetCurrent(), 0.0, 0, 0, fired, &mut context) };
        if !timer.is_null() {
            unsafe {
                CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
                CFRelease(timer);
            }
        }
    }

    /// The Cancel of the render's progress window ("Rendering to file…"), visible: never the render settings dialog behind it.
    fn render_cancel() -> Option<Id> {
        for window in unsafe { windows() } {
            if !super::is_render_window(&unsafe { text(object(window, c"title")) }) || !unsafe { yes(window, c"isVisible") } {
                continue;
            }
            if let Some((cancel, _)) = unsafe { buttons(window) }.into_iter().find(|(_, label)| label == "Cancel") {
                return Some(cancel);
            }
        }
        None
    }

    pub fn press() -> bool {
        if render_cancel().is_none() {
            return false;
        }
        click_soon(Click::RenderCancel);
        true
    }

    /// REAPER's alert about the files a cancelled render left ("Save incomplete rendered files?"), answered No: Kumi
    /// deletes them itself. False when no such alert is up.
    pub fn answer_incomplete() -> bool {
        if incomplete_no().is_none() {
            return false;
        }
        click_soon(Click::IncompleteNo);
        true
    }

    /// The No of REAPER's alert about a cancelled render's incomplete files, when it's up.
    fn incomplete_no() -> Option<Id> {
        let app = unsafe { app() };
        if app.is_null() {
            return None;
        }
        let modal = unsafe { object(app, c"modalWindow") };
        if modal.is_null() {
            return None;
        }
        let title = unsafe { text(object(modal, c"title")) };
        let class = unsafe { objc_getClass(c"NSTextField".as_ptr()) };
        let mut fields = Vec::new();
        if !class.is_null() {
            unsafe { views_of(object(modal, c"contentView"), class, 0, &mut fields) };
        }
        let words: Vec<String> = fields.into_iter().map(|f| unsafe { text(object(f, c"stringValue")) }).collect();
        let about = title.to_lowercase().contains("incomplete") || words.iter().any(|w| w.to_lowercase().contains("incomplete"));
        if !about {
            return None;
        }
        unsafe { buttons(modal) }.into_iter().find(|(_, label)| label == "No").map(|(no, _)| no)
    }

    /// Every window: its title, whether it shows, its buttons; and the modal one's.
    pub fn titles() -> Vec<String> {
        let mut out: Vec<String> = unsafe { windows() }
            .into_iter()
            .map(|w| {
                let labels: Vec<String> = unsafe { buttons(w) }.into_iter().map(|(_, l)| l).collect();
                format!("{:?} visible={} buttons={labels:?}", unsafe { text(object(w, c"title")) }, unsafe { yes(w, c"isVisible") })
            })
            .collect();
        let app = unsafe { app() };
        if !app.is_null() {
            let modal = unsafe { object(app, c"modalWindow") };
            if !modal.is_null() {
                out.push(format!("modal: {:?}", unsafe { text(object(modal, c"title")) }));
            }
        }
        out
    }
}

#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
mod platform {
    use std::ffi::c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(callback: extern "system" fn(*mut c_void, isize) -> i32, data: isize) -> i32;
        fn EnumChildWindows(parent: *mut c_void, callback: extern "system" fn(*mut c_void, isize) -> i32, data: isize) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
        fn GetWindowTextW(hwnd: *mut c_void, text: *mut u16, max: i32) -> i32;
        fn GetClassNameW(hwnd: *mut c_void, name: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(hwnd: *mut c_void) -> i32;
        fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
    }

    const BM_CLICK: u32 = 0x00F5;
    const WM_COMMAND: u32 = 0x0111;
    const IDNO: usize = 7;

    fn text_of(hwnd: *mut c_void, class: bool) -> String {
        let mut text = [0u16; 256];
        let len = unsafe {
            if class {
                GetClassNameW(hwnd, text.as_mut_ptr(), text.len() as i32)
            } else {
                GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32)
            }
        };
        String::from_utf16_lossy(&text[..len.max(0) as usize])
    }

    /// This process's visible top-level windows.
    fn windows() -> Vec<*mut c_void> {
        extern "system" fn each(hwnd: *mut c_void, data: isize) -> i32 {
            let found = unsafe { &mut *(data as *mut Vec<*mut c_void>) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid == std::process::id() && unsafe { IsWindowVisible(hwnd) } != 0 {
                found.push(hwnd);
            }
            1
        }
        let mut found: Vec<*mut c_void> = Vec::new();
        unsafe { EnumWindows(each, &mut found as *mut Vec<*mut c_void> as isize) };
        found
    }

    /// A window's buttons, with their text.
    fn buttons(parent: *mut c_void) -> Vec<(*mut c_void, String)> {
        extern "system" fn each(hwnd: *mut c_void, data: isize) -> i32 {
            let found = unsafe { &mut *(data as *mut Vec<*mut c_void>) };
            found.push(hwnd);
            1
        }
        let mut children: Vec<*mut c_void> = Vec::new();
        unsafe { EnumChildWindows(parent, each, &mut children as *mut Vec<*mut c_void> as isize) };
        children
            .into_iter()
            .filter(|c| text_of(*c, true).eq_ignore_ascii_case("Button"))
            .map(|c| (c, text_of(c, false).replace('&', "")))
            .collect()
    }

    /// The progress window's Cancel, clicked by a message the dialog's own loop handles (the caller returns first).
    pub fn press() -> bool {
        for hwnd in windows() {
            if !super::is_render_window(&text_of(hwnd, false)) {
                continue;
            }
            if let Some((cancel, _)) = buttons(hwnd).into_iter().find(|(_, label)| label == "Cancel") {
                unsafe { PostMessageW(cancel, BM_CLICK, 0, 0) };
                return true;
            }
        }
        false
    }

    /// REAPER's "Save incomplete rendered files?" message box (titled "REAPER Render Incomplete!"), answered No.
    pub fn answer_incomplete() -> bool {
        for hwnd in windows() {
            if text_of(hwnd, false).to_lowercase().contains("incomplete") {
                unsafe { PostMessageW(hwnd, WM_COMMAND, IDNO, 0) };
                return true;
            }
        }
        false
    }

    pub fn titles() -> Vec<String> {
        windows()
            .into_iter()
            .map(|hwnd| {
                let labels: Vec<String> = buttons(hwnd).into_iter().map(|(_, l)| l).collect();
                format!("{:?} buttons={labels:?}", text_of(hwnd, false))
            })
            .collect()
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
#[cfg_attr(test, allow(dead_code))]
mod platform {
    pub fn press() -> bool {
        false
    }

    pub fn titles() -> Vec<String> {
        vec![]
    }

    pub fn answer_incomplete() -> bool {
        false
    }
}
