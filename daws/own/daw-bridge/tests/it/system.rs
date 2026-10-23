//! What each system decides for a DAW's bridge, checked on CI's macOS and Windows runs too (the `ci-os` set): whether a
//! pid is a running process of this user's (whether a bridge's discovery file is stale), and macOS's downloaded mark,
//! which Kumi's own copy of a bridge never keeps.

use daw_bridge::daw::Daw;
use daw_bridge::files::process_alive;
use daw_bridge::lifecycle::{Lifecycle, Target};

#[test]
fn this_users_running_process_is_alive_and_an_ended_one_or_another_users_isnt() {
    assert!(process_alive(std::process::id()));
    let mut child = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "exit"]).spawn().unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let ended = child.id();
    child.wait().unwrap();
    assert!(!process_alive(ended));
    assert!(!process_alive(0));
    // pid 1 is root's (init, launchd): another user's process, unless these tests run as root.
    #[cfg(unix)]
    if unsafe { libc::geteuid() } != 0 {
        assert!(!process_alive(1));
    }
}

/// A bundle a browser downloaded carries macOS's mark into every copy made from it: the copy Kumi puts in the DAW's folder
/// loses it, on install and on rollback; the file it came from keeps it (not Kumi's to change).
#[cfg(target_os = "macos")]
#[test]
fn kumis_copy_of_a_bridge_never_keeps_macos_downloaded_mark() {
    use daw_bridge::detect::quarantined;
    use std::os::unix::ffi::OsStrExt;
    let mark = |path: &std::path::Path| {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let value = b"0083;00000000;Safari;";
        let set = unsafe { libc::setxattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
        assert_eq!(set, 0, "{}", std::io::Error::last_os_error());
    };
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("REAPER").join("UserPlugins");
    let target = Target {
        daw: Daw::Reaper,
        dir: plugins.clone(),
        name: "reaper_kumi.dylib".into(),
        folder: false,
        reference: plugins.join("kumi-bridge-reference.json"),
    };
    let life = Lifecycle::new(&root.path().join("kumi"), target);
    let (one, two) = (root.path().join("one.dylib"), root.path().join("two.dylib"));
    for (source, bytes) in [(&one, b"one"), (&two, b"two")] {
        std::fs::write(source, bytes).unwrap();
        mark(source);
        assert!(quarantined(source));
    }
    life.install(&one, "0.1.0").unwrap();
    assert!(!quarantined(&plugins.join("reaper_kumi.dylib")));
    life.install(&two, "0.2.0").unwrap();
    // The rollback places the generation kept from the marked download: unmarked all the same.
    life.rollback().unwrap();
    assert_eq!(std::fs::read(plugins.join("reaper_kumi.dylib")).unwrap(), b"one");
    assert!(!quarantined(&plugins.join("reaper_kumi.dylib")));
    assert!(quarantined(&one), "the download stays as it was");
    // A tool folder, every file in it.
    let tools = root.path().join("Tools");
    let source = root.path().join("source").join("com.kumi.Bridge.xrnx");
    std::fs::create_dir_all(source.join("payload")).unwrap();
    std::fs::write(source.join("main.lua"), "-- main\n").unwrap();
    std::fs::write(source.join("payload").join("bridge.lua"), "-- bridge\n").unwrap();
    for path in [source.clone(), source.join("main.lua"), source.join("payload"), source.join("payload").join("bridge.lua")] {
        mark(&path);
    }
    let tool = Target {
        daw: Daw::Renoise,
        dir: tools.clone(),
        name: "com.kumi.Bridge.xrnx".into(),
        folder: true,
        reference: tools.join("com.kumi.Bridge.xrnx").join("bridge-reference.json"),
    };
    Lifecycle::new(&root.path().join("kumi"), tool).install(&source, "1.0.0").unwrap();
    for path in ["", "main.lua", "payload", "payload/bridge.lua"] {
        assert!(!quarantined(&tools.join("com.kumi.Bridge.xrnx").join(path)), "{path}");
    }
}

/// A running REAPER on Windows holds its extension's DLL mapped: an update or a rollback sets the held copy aside and
/// puts the new one in place, the next action once REAPER has let go removes what was set aside, and an uninstall
/// names the copy it couldn't remove (nothing of Kumi's will be left to clean it up). Any DLL serves to hold: REAPER's
/// extension isn't built here.
#[cfg(windows)]
#[test]
fn a_dll_a_running_reaper_holds_is_set_aside_by_an_update_and_named_by_an_uninstall() {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut std::ffi::c_void;
        fn FreeLibrary(module: *mut std::ffi::c_void) -> i32;
    }
    let hold = |path: &std::path::Path| {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        assert!(!module.is_null(), "{}: {}", path.display(), std::io::Error::last_os_error());
        module
    };
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("REAPER").join("UserPlugins");
    let target = Target {
        daw: Daw::Reaper,
        dir: plugins.clone(),
        name: "reaper_kumi.dll".into(),
        folder: false,
        reference: plugins.join("kumi-bridge-reference.json"),
    };
    let system = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap()).join("System32").join("version.dll");
    let (one, two) = (root.path().join("one.dll"), root.path().join("two.dll"));
    std::fs::copy(&system, &one).unwrap();
    let mut bytes = std::fs::read(&system).unwrap();
    bytes.push(0);
    std::fs::write(&two, &bytes).unwrap();
    let set_aside = || -> Vec<std::path::PathBuf> {
        std::fs::read_dir(&plugins)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("reaper_kumi.dll.kumi-old-"))
            .map(|entry| entry.path())
            .collect()
    };
    let life = Lifecycle::new(&root.path().join("kumi"), target.clone());
    life.install(&one, "0.1.0").unwrap();
    let held = hold(&target.path());
    // Updated and rolled back while REAPER holds the first copy: each time the copy asked for is in place.
    life.install(&two, "0.1.1").unwrap();
    assert_eq!(std::fs::read(target.path()).unwrap(), bytes);
    // (the held copy is set aside, unless this Windows renames over a mapped file outright)
    assert!(set_aside().len() <= 1, "{:?}", set_aside());
    life.rollback().unwrap();
    assert_eq!(std::fs::read(target.path()).unwrap(), std::fs::read(&one).unwrap());
    assert!(set_aside().len() <= 1, "{:?}", set_aside());
    // REAPER quit: the next action removes it.
    assert_ne!(unsafe { FreeLibrary(held) }, 0);
    life.install(&two, "0.1.1").unwrap();
    assert!(set_aside().is_empty(), "{:?}", set_aside());
    // An uninstall while REAPER holds the extension: it's out of REAPER's way, and the copy left is named.
    let held = hold(&target.path());
    let removed = life.uninstall(true).unwrap();
    assert!(!target.path().exists());
    // Whatever stays in REAPER's folder is named, for the producer to delete once REAPER has quit.
    let left = removed.history[0].left.clone().map(std::path::PathBuf::from);
    assert_eq!(set_aside(), left.iter().cloned().collect::<Vec<_>>());
    assert_ne!(unsafe { FreeLibrary(held) }, 0);
    if let Some(left) = left {
        std::fs::remove_file(&left).unwrap();
    }
}

/// Each owner-only permission set on Windows is a PowerShell run (seconds, with Defender): a folder an earlier action
/// made and locked isn't locked again, so an update that finds the bridge in place runs none.
#[cfg(windows)]
#[test]
fn an_up_to_date_update_on_windows_runs_no_powershell() {
    use daw_bridge::owner::acl_runs_on_this_thread as runs;
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("bridges").join("reaper");
    let before = runs();
    daw_bridge::owner::create_owner_only_dir(&folder).unwrap();
    assert_eq!(runs() - before, 1, "a folder it made is locked");
    daw_bridge::owner::create_owner_only_dir(&folder).unwrap();
    assert_eq!(runs() - before, 1, "made already: left as it is");
    let plugins = root.path().join("REAPER").join("UserPlugins");
    let target = Target {
        daw: Daw::Reaper,
        dir: plugins.clone(),
        name: "reaper_kumi.dll".into(),
        folder: false,
        reference: plugins.join("kumi-bridge-reference.json"),
    };
    let source = root.path().join("reaper_kumi.dll");
    std::fs::write(&source, b"extension").unwrap();
    let life = Lifecycle::new(&root.path().join("kumi"), target);
    life.install(&source, "0.1.0").unwrap();
    let installed = runs();
    life.install(&source, "0.1.0").unwrap();
    assert_eq!(runs(), installed, "the bridge in place: nothing to lock");
}
