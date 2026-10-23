//! The extension REAPER loads opens with the system's own loader and has REAPER's entry point: as the tests build it,
//! or the build named by REAPER_KUMI_ARTIFACT (CI's release build, after the platform's strip). On macOS, dyld_info
//! must find its LINKEDIT string pool aligned: rustc's strip can misalign it on macOS 27, and dyld then refuses the
//! library ("mis-aligned LINKEDIT string pool").

use std::{
    ffi::{c_char, c_void, CStr, CString},
    path::PathBuf,
};

const NAME: &str = if cfg!(target_os = "macos") {
    "libreaper_kumi.dylib"
} else if cfg!(windows) {
    "reaper_kumi.dll"
} else {
    "libreaper_kumi.so"
};

/// The library to load: the one named, or the one this test's build made beside it.
fn artifact() -> PathBuf {
    if let Some(path) = std::env::var_os("REAPER_KUMI_ARTIFACT").filter(|path| !path.is_empty()) {
        return path.into();
    }
    let deps = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let profile = deps.parent().unwrap().to_path_buf();
    [profile.join(NAME), deps.join(NAME)]
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {NAME} beside {}", deps.display()))
}

#[cfg(unix)]
mod loader {
    use super::*;
    unsafe extern "C" {
        fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }
    const RTLD_NOW: i32 = 2;

    /// Opens the library and finds `symbol`: what went wrong otherwise.
    pub fn open(path: &std::path::Path, symbol: &str) -> Result<(), String> {
        let path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let symbol = CString::new(symbol).unwrap();
        let said = || unsafe {
            let error = dlerror();
            if error.is_null() {
                "no reason given".to_string()
            } else {
                CStr::from_ptr(error).to_string_lossy().into_owned()
            }
        };
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            return Err(said());
        }
        if unsafe { dlsym(handle, symbol.as_ptr()) }.is_null() {
            return Err(said());
        }
        Ok(())
    }
}

#[cfg(windows)]
mod loader {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    /// Opens the library and finds `symbol`: what went wrong otherwise.
    pub fn open(path: &std::path::Path, symbol: &str) -> Result<(), String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let symbol = CString::new(symbol).unwrap();
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        if module.is_null() {
            return Err(format!("LoadLibraryW failed ({})", unsafe { GetLastError() }));
        }
        if unsafe { GetProcAddress(module, symbol.as_ptr()) }.is_null() {
            return Err(format!("GetProcAddress failed ({})", unsafe { GetLastError() }));
        }
        Ok(())
    }
}

#[test]
fn the_extension_loads_and_has_reapers_entry_point() {
    let path = artifact();
    if cfg!(target_os = "macos") {
        // dyld_info comes with macOS's developer tools; without it, loading below is the check.
        if let Ok(out) = std::process::Command::new("dyld_info").arg("-segments").arg(&path).output() {
            let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(!said.contains("mis-aligned"), "dyld_info finds {} misaligned:\n{said}", path.display());
        }
    }
    loader::open(&path, "ReaperPluginEntry").unwrap_or_else(|reason| panic!("{} doesn't load: {reason}", path.display()));
}
