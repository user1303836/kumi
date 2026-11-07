//! What each system decides for a DAW's bridge, checked on macOS and Windows too by the Extended workflow: whether a
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

/// Kumi's folders and files for a DAW's bridge are made owner-only with Windows' own calls (`lifecycle::native_acl`):
/// a first install runs one PowerShell, for the secret's first write (owner.rs's, which REAPER's extension loads too),
/// and an upgrade, a rollback, an uninstall and the doctor's look at the secret run none. Each is owner-only by owner.rs's
/// own check.
#[cfg(windows)]
#[test]
fn a_first_install_on_windows_runs_one_powershell_and_what_follows_none() {
    use daw_bridge::lifecycle::is_owner_only;
    use daw_bridge::owner::acl_runs_on_this_thread as runs;
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("REAPER").join("UserPlugins");
    let target = Target {
        daw: Daw::Reaper,
        dir: plugins.clone(),
        name: "reaper_kumi.dll".into(),
        folder: false,
        reference: plugins.join("kumi-bridge-reference.json"),
    };
    let (one, two) = (root.path().join("one.dll"), root.path().join("two.dll"));
    std::fs::write(&one, b"extension").unwrap();
    std::fs::write(&two, b"extension, newer").unwrap();
    let life = Lifecycle::new(&root.path().join("kumi"), target);
    let before = runs();
    life.install(&one, "0.1.0").unwrap();
    assert_eq!(runs() - before, 1, "a first install: the secret's first write, and nothing else");
    let installed = runs();
    life.install(&two, "0.1.1").unwrap();
    life.rollback().unwrap();
    assert!(is_owner_only(&life.paths.secret()), "the doctor's look at the secret");
    assert_eq!(runs(), installed, "an upgrade, a rollback and the doctor's look: no PowerShell");
    let paths = &life.paths;
    for path in [paths.root.clone(), paths.discovery(), paths.logs(), paths.generations(), paths.receipt(), paths.secret()] {
        assert!(daw_bridge::owner::is_owner_only(&path), "owner.rs's check: {}", path.display());
    }
    let checked = runs();
    life.uninstall(false).unwrap();
    assert_eq!(runs(), checked, "an uninstall: no PowerShell");
}

/// The user-owned DACL matrix stays in parity with the legacy PowerShell checks.
/// The token's default owner is also accepted natively, covered separately below.
#[cfg(windows)]
mod windows_acl_shapes {
    use daw_bridge::lifecycle::native_acl::{
        is_kumis_shape, make_kumis_folder_shape, make_kumis_shape,
        shapes::{own_foreign_user, own_mine, write, write_raw, Ace, Whose, FULL, INHERITABLE, INHERITED, INHERIT_ONLY},
    };
    use daw_bridge::owner;
    use std::path::{Path, PathBuf};

    const EVERYONE: &str = "S-1-1-0";
    const READ: u32 = 0x0012_0089;
    const WRITE: u32 = 0x0012_0116;
    const GENERIC_ALL: u32 = 0x1000_0000;
    /// Handed down to folders only (CONTAINER_INHERIT_ACE).
    const TO_FOLDERS: u32 = 0x2;

    /// owner.rs's own checks, `FILE_CHECKS` for a file and `DIRECTORY_CHECKS` for a folder (as `owner::is_owner_only`
    /// picks them), on every path in one PowerShell: the code each would exit with.
    fn checks(paths: &[&Path]) -> Vec<i32> {
        let source = include_str!("../../src/owner.rs");
        let script = |name: &str| -> String {
            let opening = format!("const {name}: &str = \"");
            let start = source.find(&opening).unwrap_or_else(|| panic!("owner.rs's {name}")) + opening.len();
            let text = &source[start..start + source[start..].find("\";").expect("its end")];
            assert!(!text.contains(['\\', '"']), "{name} is read as it's written");
            text.replace("exit ", "return ")
        };
        let (file, folder) = (script("FILE_CHECKS"), script("DIRECTORY_CHECKS"));
        let batch = format!(
            "$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;$file={{ param($p) {file} }};$folder={{ param($p) {folder} }};foreach ($p in ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:KUMI_TEST_PATHS)) -split \"`n\")) {{ $code=1; try {{ if ([System.IO.Directory]::Exists($p)) {{ $code=& $folder $p }} else {{ $code=& $file $p }} }} catch {{ $code=1 }}; \"code $code\" }}"
        );
        let joined = paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n");
        use base64::{engine::general_purpose::STANDARD, Engine};
        let powershell = format!("{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", std::env::var("SystemRoot").unwrap());
        let said = std::process::Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &batch])
            .env("KUMI_TEST_PATHS", STANDARD.encode(joined.as_bytes()))
            .env_remove("PSModulePath")
            .output()
            .expect("PowerShell starts");
        String::from_utf8_lossy(&said.stdout).lines().filter_map(|line| line.trim().strip_prefix("code ")?.parse().ok()).collect()
    }

    fn allow(flags: u32, mask: u32, whose: Whose) -> Ace {
        Ace { allow: true, flags, mask, whose }
    }

    /// Where Windows' own calls fail (as one can), owner.rs's PowerShell steps make Kumi's folders and files as they did
    /// before the fast path: seven runs for a first install and two for an upgrade, each owner-only by owner.rs's check.
    /// The doctor's look at a secret of another shape is owner.rs's check too, which refuses it.
    #[test]
    fn where_windows_own_calls_fail_owner_rs_steps_take_over_as_before() {
        use super::{Daw, Lifecycle, Target};
        use daw_bridge::lifecycle::{is_owner_only, native_acl::shapes::refuse_on_this_thread};
        use daw_bridge::owner::acl_runs_on_this_thread as runs;
        let root = tempfile::tempdir().unwrap();
        let plugins = root.path().join("REAPER").join("UserPlugins");
        let target = Target {
            daw: Daw::Reaper,
            dir: plugins.clone(),
            name: "reaper_kumi.dll".into(),
            folder: false,
            reference: plugins.join("kumi-bridge-reference.json"),
        };
        let (one, two) = (root.path().join("one.dll"), root.path().join("two.dll"));
        std::fs::write(&one, b"extension").unwrap();
        std::fs::write(&two, b"extension, newer").unwrap();
        let life = Lifecycle::new(&root.path().join("kumi"), target);
        refuse_on_this_thread(true);
        let before = runs();
        let installed = life.install(&one, "0.1.0").map(drop);
        let first = runs() - before;
        let upgraded = life.install(&two, "0.1.1").map(drop);
        let upgrade = runs() - before - first;
        refuse_on_this_thread(false);
        installed.unwrap();
        upgraded.unwrap();
        assert_eq!((first, upgrade), (7, 2), "owner.rs's PowerShell runs, as before: a first install's, then an upgrade's");
        let paths = &life.paths;
        for path in [paths.root.clone(), paths.discovery(), paths.logs(), paths.generations(), paths.receipt(), paths.secret()] {
            assert!(owner::is_owner_only(&path), "owner.rs's check: {}", path.display());
        }
        write(&paths.secret(), Some(&[allow(0, FULL, Whose::Me), allow(0, READ, Whose::Sid(EVERYONE))]), true);
        let looked = runs();
        assert!(!is_owner_only(&paths.secret()), "a secret everyone may read isn't owner-only");
        assert_eq!(runs() - looked, 1, "the doctor's look at it is owner.rs's check");
    }

    #[test]
    fn the_tokens_default_owner_is_accepted_but_only_the_users_single_ace_is_private() {
        use daw_bridge::lifecycle::native_acl::shapes;
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("default-owner");
        std::fs::create_dir(&folder).unwrap();
        let file = root.path().join("default-owner.json");
        std::fs::write(&file, "{}").unwrap();
        if std::env::var_os("CI").is_some() {
            assert!(!shapes::default_owner_is_user(), "elevated CI must cover a distinct TokenOwner");
        }
        for (path, flags) in [(&folder, INHERITABLE), (&file, 0)] {
            assert!(shapes::has_default_owner(path));
            write(path, Some(&[allow(flags, FULL, Whose::Me)]), true);
            assert!(is_kumis_shape(path), "the actual TokenOwner is accepted: {}", path.display());
            assert!(shapes::has_default_owner(path), "checking must not take ownership");
            write(path, Some(&[allow(flags, FULL, Whose::Sid("S-1-5-32-544"))]), true);
            let administrators_ace = is_kumis_shape(path);
            write(path, Some(&[allow(flags, FULL, Whose::Me)]), true);
            assert!(!administrators_ace, "accepting TokenOwner must not authorize an Administrators ACE");
        }
    }

    #[test]
    fn a_default_owned_render_keeps_its_owner_when_its_inherited_user_rule_is_protected() {
        use daw_bridge::lifecycle::native_acl::{is_kumis_shape_handle, protect_inherited_render, shapes};
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        write(root.path(), Some(&[allow(INHERITABLE, FULL, Whose::Me)]), true);
        let path = root.path().join("render.wav");
        std::fs::write(&path, b"render").unwrap();
        assert!(shapes::has_default_owner(&path));
        assert!(!is_kumis_shape(&path), "the render starts with an inherited, unprotected DACL");
        let open = || std::fs::OpenOptions::new().access_mode(0x0006_0001).share_mode(1).open(&path).unwrap();
        let file = open();
        protect_inherited_render(&file).unwrap();
        assert!(is_kumis_shape_handle(&file, false));
        drop(file);
        assert!(is_kumis_shape(&path));
        assert!(shapes::has_default_owner(&path), "protection must not take ownership");
        if own_foreign_user(&path) {
            let before = shapes::descriptor(&path);
            let file = open();
            let refused = protect_inherited_render(&file).is_err();
            assert!(!is_kumis_shape_handle(&file, false));
            drop(file);
            let after = shapes::descriptor(&path);
            own_mine(&path);
            assert!(refused, "a foreign user must not pass the already-protected fast path");
            assert_eq!(before, after);
        } else {
            assert!(std::env::var_os("CI").is_none(), "CI must exercise a foreign render owner");
        }
    }

    #[test]
    fn the_fast_path_says_owner_only_only_where_the_powershell_check_does() {
        let folder = tempfile::tempdir().unwrap();
        // A folder that hands nothing down, so an unprotected DACL holds only what's written on it.
        write(folder.path(), Some(&[allow(0, FULL, Whose::Me)]), true);
        let made = |name: &str| {
            let path = folder.path().join(name);
            std::fs::write(&path, "{}").unwrap();
            own_mine(&path);
            path
        };
        let made_folder = |name: &str| {
            let path = folder.path().join(name);
            std::fs::create_dir(&path).unwrap();
            own_mine(&path);
            path
        };
        let in_temp = tempfile::NamedTempFile::new().unwrap();
        let folder_in_temp = tempfile::tempdir().unwrap();
        // (name, path, the check's code where the shape fixes it, whether the fast path takes it where that's fixed)
        let mut shapes: Vec<(&str, PathBuf, Option<i32>, Option<bool>)> = vec![
            ("a new file in TEMP", in_temp.path().to_path_buf(), None, None),
            ("a new folder in TEMP", folder_in_temp.path().to_path_buf(), None, Some(false)),
        ];
        let natively = made("natively.json");
        make_kumis_shape(&natively).unwrap();
        shapes.push(("made natively", natively, Some(0), Some(true)));
        let scripted = folder.path().join("scripted.json");
        owner::write_owner_only(&scripted, b"{}").unwrap();
        shapes.push(("written by owner.rs (SECURE_FILE)", scripted, Some(0), Some(true)));
        let cases: Vec<(&str, Option<Vec<Ace>>, bool, Option<i32>)> = vec![
            ("Kumi's shape, written raw", Some(vec![allow(0, FULL, Whose::Me)]), true, Some(0)),
            ("a second rule, for everyone", Some(vec![allow(0, FULL, Whose::Me), allow(0, READ, Whose::Sid(EVERYONE))]), true, Some(4)),
            (
                "two rules for you that add up to full control",
                Some(vec![allow(0, READ, Whose::Me), allow(0, FULL & !READ, Whose::Me)]),
                true,
                None,
            ),
            // Windows tidies a file's ACEs as it writes them (flags dropped, generic rights mapped): the check decides.
            ("its one rule inherited", Some(vec![allow(INHERITED, FULL, Whose::Me)]), true, None),
            ("its one rule inheritable", Some(vec![allow(INHERITABLE, FULL, Whose::Me)]), true, None),
            ("its one rule inherit-only", Some(vec![allow(INHERITABLE | INHERIT_ONLY, FULL, Whose::Me)]), true, None),
            ("generic rights in place of full control", Some(vec![allow(0, GENERIC_ALL, Whose::Me)]), true, None),
            ("full control and generic rights besides", Some(vec![allow(0, FULL | GENERIC_ALL, Whose::Me)]), true, None),
            ("read and write, not full control", Some(vec![allow(0, READ | WRITE, Whose::Me)]), true, Some(8)),
            ("a deny rule", Some(vec![Ace { allow: false, flags: 0, mask: FULL, whose: Whose::Me }]), true, Some(7)),
            ("a rule for everyone alone", Some(vec![allow(0, FULL, Whose::Sid(EVERYONE))]), true, Some(5)),
            ("not protected", Some(vec![allow(0, FULL, Whose::Me)]), false, Some(3)),
            ("an empty DACL", Some(vec![]), true, Some(4)),
            ("a null DACL", None, true, None),
        ];
        for (n, (name, aces, protected, expected)) in cases.into_iter().enumerate() {
            let path = made(&format!("case-{n}.json"));
            write(&path, aces.as_deref(), protected);
            shapes.push((name, path, expected, expected.map(|code| code == 0)));
        }
        // Written whole, flags kept: an inherited ACE in a protected DACL, and an inherit-only one.
        for (name, flags, expected) in [
            ("its one rule marked inherited, written whole", INHERITED, Some(6)),
            ("its one rule inherit-only, written whole", INHERITABLE | INHERIT_ONLY, None),
        ] {
            let path = made(&format!("{}.json", name.replace([' ', ','], "-")));
            write_raw(&path, &[allow(flags, FULL, Whose::Me)]);
            shapes.push((name, path, expected, expected.map(|code| code == 0)));
        }
        // Folders. DIRECTORY_CHECKS asks only for the owner and the protection, so it passes folders whose rules aren't
        // Kumi's shape, which the fast path leaves to it.
        let native_folder = made_folder("natively");
        make_kumis_folder_shape(&native_folder).unwrap();
        shapes.push(("a folder made natively", native_folder, Some(0), Some(true)));
        let scripted_folder = folder.path().join("scripted");
        owner::create_owner_only_dir(&scripted_folder).unwrap();
        shapes.push(("a folder made by owner.rs (SECURE_DIRECTORY)", scripted_folder, Some(0), Some(true)));
        let folder_cases: Vec<(&str, Option<Vec<Ace>>, bool, Option<i32>, Option<bool>)> = vec![
            ("a folder, Kumi's shape written raw", Some(vec![allow(INHERITABLE, FULL, Whose::Me)]), true, Some(0), Some(true)),
            ("a folder whose one rule has no flags", Some(vec![allow(0, FULL, Whose::Me)]), true, Some(0), Some(false)),
            ("a folder whose one rule goes to folders only", Some(vec![allow(TO_FOLDERS, FULL, Whose::Me)]), true, Some(0), Some(false)),
            (
                "a folder whose one rule is handed down only",
                Some(vec![allow(INHERITABLE | INHERIT_ONLY, FULL, Whose::Me)]),
                true,
                Some(0),
                Some(false),
            ),
            (
                "a folder with a second rule, for everyone",
                Some(vec![allow(INHERITABLE, FULL, Whose::Me), allow(INHERITABLE, READ, Whose::Sid(EVERYONE))]),
                true,
                Some(0),
                Some(false),
            ),
            ("a folder's one rule, read and write", Some(vec![allow(INHERITABLE, READ | WRITE, Whose::Me)]), true, Some(0), Some(false)),
            ("a folder's one rule, for everyone", Some(vec![allow(INHERITABLE, FULL, Whose::Sid(EVERYONE))]), true, Some(0), Some(false)),
            ("a folder's one rule, generic rights", Some(vec![allow(INHERITABLE, GENERIC_ALL, Whose::Me)]), true, None, None),
            ("a folder not protected", Some(vec![allow(INHERITABLE, FULL, Whose::Me)]), false, Some(3), Some(false)),
            ("a folder with a null DACL", None, true, None, Some(false)),
        ];
        for (n, (name, aces, protected, code, fast)) in folder_cases.into_iter().enumerate() {
            let path = made_folder(&format!("folder-{n}"));
            write(&path, aces.as_deref(), protected);
            shapes.push((name, path, code, fast));
        }
        // A foreign user remains foreign even on elevated runners whose default owner is Administrators.
        let handed = made("handed-over.json");
        write(&handed, Some(&[allow(0, FULL, Whose::Me)]), true);
        let handed_folder = made_folder("handed-over");
        write(&handed_folder, Some(&[allow(INHERITABLE, FULL, Whose::Me)]), true);
        if own_foreign_user(&handed) && own_foreign_user(&handed_folder) {
            shapes.push(("owned by a foreign user", handed, Some(2), Some(false)));
            shapes.push(("a folder owned by a foreign user", handed_folder, Some(2), Some(false)));
        } else {
            assert!(std::env::var_os("CI").is_none(), "CI must exercise foreign-user ownership");
            println!("skipped: foreign user owner (this account lacks SeRestorePrivilege)");
        }
        // Every shape in one PowerShell, owner.rs's own checks, held to `owner::is_owner_only` (one PowerShell a path)
        // on a file, a folder, one Kumi made and the last; then the whole matrix before any assertion.
        let paths: Vec<&Path> = shapes.iter().map(|(_, path, _, _)| path.as_path()).collect();
        let codes = checks(&paths);
        assert_eq!(codes.len(), shapes.len(), "one code a path: {codes:?}");
        for at in [0, 1, 2, shapes.len() - 1] {
            assert_eq!(owner::is_owner_only(paths[at]), codes[at] == 0, "{}: one run a path says what the batch does", shapes[at].0);
        }
        let rows: Vec<_> = shapes
            .iter()
            .zip(&codes)
            .map(|((name, path, code, fast), check)| (*name, is_kumis_shape(path), *check, *code, *fast))
            .collect();
        let said = rows
            .iter()
            .map(|(name, fast, check, _, _)| format!("{name}: fast path {fast}, PowerShell check {check}"))
            .collect::<Vec<_>>()
            .join("\n");
        println!("{said}");
        // Each gets back a DACL that lets this account remove it (a deny, an empty or an inherit-only DACL leaves it
        // none), so the folder goes when the test ends.
        for (_, path, _, _) in &shapes {
            write(path, Some(&[allow(0, FULL, Whose::Me)]), true);
        }
        for (name, fast, check, code, expected_fast) in &rows {
            assert!(!fast || *check == 0, "{name}: the fast path passed what the check refuses\n{said}");
            if let Some(code) = code {
                assert_eq!(check, code, "{name}: the shape isn't what it says\n{said}");
            }
            if let Some(expected_fast) = expected_fast {
                assert_eq!(fast, expected_fast, "{name}: Kumi's own shapes take the fast path, and only they\n{said}");
            }
        }
    }

    /// A file or folder the fast path takes by its own path goes to the PowerShell check by any other way to it: a share,
    /// a drive mapped to one, a junction or a link on the way. Nothing is written there either.
    #[test]
    fn the_fast_path_leaves_shares_and_links_to_the_powershell_check() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("bridge.secret");
        std::fs::write(&file, "{}").unwrap();
        make_kumis_shape(&file).unwrap();
        assert!(is_kumis_shape(&file), "the file itself takes the fast path");
        let logs = folder.path().join("logs");
        std::fs::create_dir(&logs).unwrap();
        make_kumis_folder_shape(&logs).unwrap();
        assert!(is_kumis_shape(&logs), "the folder itself takes the fast path");
        let local = file.display().to_string();
        let (letter, rest) = local.split_once(":\\").unwrap();
        let folder_rest = logs.display().to_string().split_once(":\\").unwrap().1.to_string();
        for share in [format!(r"\\localhost\{letter}$"), format!(r"\\?\UNC\localhost\{letter}$")] {
            let (shared, shared_folder) = (format!(r"{share}\{rest}"), format!(r"{share}\{folder_rest}"));
            assert!(!is_kumis_shape(Path::new(&shared)), "{shared}");
            assert!(make_kumis_shape(Path::new(&shared)).is_err(), "nothing written by a share's path: {shared}");
            assert!(!is_kumis_shape(Path::new(&shared_folder)), "{shared_folder}");
            assert!(make_kumis_folder_shape(Path::new(&shared_folder)).is_err(), "nothing written by a share's path: {shared_folder}");
        }
        // The same file by a drive-relative path, from its folder: the drive's working folder is the test's own, and goes
        // back to what it was (Windows can't remove a process's working folder, and other tests start from it).
        struct Back(PathBuf);
        impl Drop for Back {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.0);
            }
        }
        let back = Back(std::env::current_dir().unwrap());
        std::env::set_current_dir(folder.path()).unwrap();
        let relative = PathBuf::from(format!("{letter}:bridge.secret"));
        let relative_folder = PathBuf::from(format!("{letter}:logs"));
        let (reached, fast, written) = (relative.is_file(), is_kumis_shape(&relative), make_kumis_shape(&relative).is_ok());
        let (folder_reached, folder_fast, folder_written) =
            (relative_folder.is_dir(), is_kumis_shape(&relative_folder), make_kumis_folder_shape(&relative_folder).is_ok());
        drop(back);
        assert!(reached && folder_reached, "the drive-relative paths reach the file and the folder");
        assert!(!fast && !written, "a drive-relative path goes to PowerShell");
        assert!(!folder_fast && !folder_written, "a folder's drive-relative path goes to PowerShell");
        // A drive mapped to that share, which Windows calls remote: CI's runners can map one.
        let free = ('G'..='Y').rev().find(|drive| !Path::new(&format!("{drive}:\\")).exists());
        let mapped = free.filter(|drive| {
            std::process::Command::new("net")
                .args(["use", &format!("{drive}:"), &format!(r"\\localhost\{letter}$")])
                .output()
                .is_ok_and(|made| made.status.success())
        });
        match mapped {
            Some(drive) => {
                let (through, through_folder) =
                    (PathBuf::from(format!(r"{drive}:\{rest}")), PathBuf::from(format!(r"{drive}:\{folder_rest}")));
                let reached = through.is_file() && through_folder.is_dir();
                let (fast, written) = (is_kumis_shape(&through), make_kumis_shape(&through).is_ok());
                let (folder_fast, folder_written) = (is_kumis_shape(&through_folder), make_kumis_folder_shape(&through_folder).is_ok());
                let _ = std::process::Command::new("net").args(["use", &format!("{drive}:"), "/delete", "/y"]).output();
                assert!(reached, "the mapped drive reaches the file and the folder");
                assert!(!fast && !written && !folder_fast && !folder_written, "a mapped drive goes to PowerShell");
            }
            None => {
                assert!(std::env::var_os("CI").is_none(), "this runner couldn't map a drive to its own share");
                println!("skipped: a mapped drive (no drive could be mapped)");
            }
        }
        // Through a junction to its folder, which needs no privilege to make; the junction itself is no plain folder.
        let parent = tempfile::tempdir().unwrap();
        let junction = parent.path().join("through");
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&junction).arg(folder.path()).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
        let linked = junction.join("bridge.secret");
        assert!(linked.is_file());
        assert!(!is_kumis_shape(&linked), "{}", linked.display());
        assert!(make_kumis_shape(&linked).is_err(), "nothing written through a junction");
        assert!(
            !is_kumis_shape(&junction.join("logs")) && make_kumis_folder_shape(&junction.join("logs")).is_err(),
            "a folder through a junction"
        );
        assert!(!is_kumis_shape(&junction) && make_kumis_folder_shape(&junction).is_err(), "the junction itself");
        // A link to the file itself, where this account may make one.
        let link = parent.path().join("link.json");
        if std::os::windows::fs::symlink_file(&file, &link).is_ok() {
            assert!(!is_kumis_shape(&link), "{}", link.display());
        }
        let _ = std::fs::remove_dir(&junction);
    }
}
