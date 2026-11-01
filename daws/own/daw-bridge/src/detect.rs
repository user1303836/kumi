//! Finding REAPER and Renoise on this computer: the apps, their versions and architectures, REAPER's resource
//! folders (portable installs included) and Renoise's per-version preferences folders. Read-only: nothing is
//! launched and no permission is asked for.
use crate::daw::Daw;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// What detection reads: the platform, the user's home, environment variables, and (for tests) a folder standing in
/// for the file system's root, under which system paths like `/Applications` are looked up.
pub struct Env<'a> {
    pub os: Os,
    pub home: PathBuf,
    pub var: &'a dyn Fn(&str) -> Option<String>,
    pub root: Option<PathBuf>,
}

impl Env<'_> {
    fn var(&self, name: &str) -> Option<String> {
        (self.var)(name).filter(|value| !value.is_empty())
    }

    /// A system path (absolute, `/`-separated), under `root` when testing.
    fn system(&self, path: &str) -> PathBuf {
        match &self.root {
            Some(root) => root.join(path.trim_start_matches('/')),
            None => PathBuf::from(path),
        }
    }

    fn var_path(&self, name: &str) -> Option<PathBuf> {
        self.var(name).map(PathBuf::from)
    }

    fn config_home(&self) -> PathBuf {
        self.var_path("XDG_CONFIG_HOME").unwrap_or_else(|| self.home.join(".config"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86_64,
    Arm64,
    /// Windows on ARM's emulation-compatible x64 ABI.
    Arm64ec,
    X86,
    Arm,
    Other,
}

/// The architectures a Mach-O (thin or universal), PE or ELF binary holds; empty when it isn't one.
pub fn binary_archs(path: &Path) -> Vec<Arch> {
    let Ok(bytes) = read_head(path, 4096) else { return vec![] };
    archs_of(&bytes)
}

fn read_head(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = vec![];
    std::fs::File::open(path)?.take(limit as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn mach_arch(cputype: u32) -> Arch {
    match cputype {
        0x0100_0007 => Arch::X86_64,
        0x0100_000c => Arch::Arm64,
        7 => Arch::X86,
        12 => Arch::Arm,
        _ => Arch::Other,
    }
}

pub fn archs_of(bytes: &[u8]) -> Vec<Arch> {
    let u32_at = |offset: usize, big: bool| -> Option<u32> {
        let slice: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
        Some(if big { u32::from_be_bytes(slice) } else { u32::from_le_bytes(slice) })
    };
    let u16_le = |offset: usize| -> Option<u16> { Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?)) };
    match bytes.get(..4) {
        Some([0xca, 0xfe, 0xba, 0xbe]) => {
            let count = u32_at(4, true).unwrap_or(0).min(16) as usize;
            (0..count).filter_map(|index| u32_at(8 + index * 20, true)).map(mach_arch).collect()
        }
        Some([0xcf, 0xfa, 0xed, 0xfe]) | Some([0xce, 0xfa, 0xed, 0xfe]) => u32_at(4, false).map(mach_arch).into_iter().collect(),
        Some([0x7f, b'E', b'L', b'F']) => {
            let machine = u16_le(18).unwrap_or(0);
            vec![match machine {
                0x3e => Arch::X86_64,
                0xb7 => Arch::Arm64,
                0x03 => Arch::X86,
                0x28 => Arch::Arm,
                _ => Arch::Other,
            }]
        }
        Some([b'M', b'Z', ..]) => {
            let Some(pe) = u32_at(0x3c, false) else { return vec![] };
            if bytes.get(pe as usize..pe as usize + 4) != Some(b"PE\0\0") {
                return vec![];
            }
            vec![match u16_le(pe as usize + 4).unwrap_or(0) {
                0x8664 => Arch::X86_64,
                0xaa64 => Arch::Arm64,
                0xa641 => Arch::Arm64ec,
                0x014c => Arch::X86,
                _ => Arch::Other,
            }]
        }
        _ => vec![],
    }
}

/// A value from a macOS app's `Contents/Info.plist` (XML form).
fn plist_value(app: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(app.join("Contents").join("Info.plist")).ok()?;
    let at = text.find(&format!("<key>{key}</key>"))?;
    let rest = &text[at..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_string())
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|entries| entries.flatten().map(|entry| entry.path()).collect()).unwrap_or_default();
    found.sort();
    found
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Where a REAPER resource folder came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceFrom {
    /// `KUMI_REAPER_RESOURCE_DIR`.
    Override,
    /// A `reaper.ini` beside the app.
    Portable,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaperInstall {
    /// `REAPER.app`, `reaper.exe` or the `reaper` binary; None when only the resource folder was found.
    pub app: Option<PathBuf>,
    pub version: Option<String>,
    pub archs: Vec<Arch>,
    pub resource: PathBuf,
    pub resource_from: ResourceFrom,
    /// The resource folder has a `reaper.ini`: REAPER has run with it.
    pub opened: bool,
}

impl ReaperInstall {
    pub fn user_plugins(&self) -> PathBuf {
        self.resource.join("UserPlugins")
    }
}

fn reaper_apps(env: &Env) -> Vec<PathBuf> {
    let mut candidates = vec![];
    match env.os {
        Os::Mac => {
            for dir in [env.system("/Applications"), env.home.join("Applications")] {
                for name in ["REAPER.app", "REAPER64.app"] {
                    candidates.push(dir.join(name));
                }
            }
        }
        Os::Windows => {
            for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
                if let Some(dir) = env.var_path(var) {
                    candidates.push(dir.join("REAPER (x64)").join("reaper.exe"));
                    candidates.push(dir.join("REAPER (ARM64)").join("reaper.exe"));
                    candidates.push(dir.join("REAPER").join("reaper.exe"));
                }
            }
        }
        Os::Linux => {
            candidates.push(env.system("/opt/REAPER/reaper"));
            candidates.push(env.home.join("opt").join("REAPER").join("reaper"));
            candidates.push(env.system("/usr/local/bin/reaper"));
            candidates.push(env.system("/usr/bin/reaper"));
        }
    }
    let mut found: Vec<PathBuf> = vec![];
    for candidate in candidates {
        if candidate.exists() && !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

fn reaper_binary(os: Os, app: &Path) -> PathBuf {
    match os {
        Os::Mac => {
            let macos = app.join("Contents").join("MacOS");
            ["REAPER", "REAPER64"].iter().map(|name| macos.join(name)).find(|path| path.exists()).unwrap_or_else(|| macos.join("REAPER"))
        }
        _ => std::fs::canonicalize(app).unwrap_or_else(|_| app.to_path_buf()),
    }
}

/// The folder a portable install keeps `reaper.ini` in: beside the app (macOS) or the binary.
fn reaper_portable_dir(os: Os, app: &Path) -> Option<PathBuf> {
    let dir = match os {
        Os::Mac => app.parent()?.to_path_buf(),
        _ => reaper_binary(os, app).parent()?.to_path_buf(),
    };
    dir.join("reaper.ini").exists().then_some(dir)
}

fn reaper_default_resource(env: &Env) -> PathBuf {
    match env.os {
        Os::Mac => env.home.join("Library").join("Application Support").join("REAPER"),
        Os::Windows => env.var_path("APPDATA").unwrap_or_else(|| env.home.join("AppData").join("Roaming")).join("REAPER"),
        Os::Linux => {
            let config = env.config_home().join("REAPER");
            let legacy = env.home.join(".reaper");
            if !config.exists() && legacy.exists() {
                legacy
            } else {
                config
            }
        }
    }
}

fn reaper_described(env: &Env, app: Option<&PathBuf>, resource: PathBuf, from: ResourceFrom) -> ReaperInstall {
    let version = app.filter(|_| env.os == Os::Mac).and_then(|app| plist_value(app, "CFBundleShortVersionString"));
    let archs = app.map(|app| binary_archs(&reaper_binary(env.os, app))).unwrap_or_default();
    let opened = resource.join("reaper.ini").exists();
    ReaperInstall { app: app.cloned(), version, archs, resource, resource_from: from, opened }
}

/// Every REAPER this computer has: one row per resource folder (the default one, each portable install, or the one
/// `KUMI_REAPER_RESOURCE_DIR` names, which then is the only one).
pub fn detect_reaper(env: &Env) -> Vec<ReaperInstall> {
    let apps = reaper_apps(env);
    let describe = |app: Option<&PathBuf>, resource: PathBuf, from: ResourceFrom| reaper_described(env, app, resource, from);
    if let Some(resource) = env.var_path("KUMI_REAPER_RESOURCE_DIR") {
        let app = apps.iter().find(|app| reaper_portable_dir(env.os, app).as_ref() == Some(&resource)).or(apps.first());
        return vec![describe(app, resource, ResourceFrom::Override)];
    }
    let mut found = vec![];
    let mut default_app = None;
    for app in &apps {
        match reaper_portable_dir(env.os, app) {
            Some(dir) => found.push(describe(Some(app), dir, ResourceFrom::Portable)),
            None => default_app = default_app.or(Some(app)),
        }
    }
    let resource = reaper_default_resource(env);
    if default_app.is_some() || resource.exists() {
        found.insert(0, describe(default_app, resource, ResourceFrom::Default));
    }
    found
}

/// `installs`, and the resource folder Kumi's extension was put in (`placed`: the extension's path, from its receipt)
/// when detection didn't find it: a portable REAPER outside the folders Kumi looks in, named once with
/// `KUMI_REAPER_RESOURCE_DIR`. Not while that variable names a folder: that one is the only one then.
pub fn with_placed(env: &Env, mut installs: Vec<ReaperInstall>, placed: Option<&Path>) -> Vec<ReaperInstall> {
    let resource = placed.and_then(Path::parent).filter(|plugins| name_of(plugins) == "UserPlugins").and_then(Path::parent);
    let Some(resource) = resource.filter(|resource| resource.is_dir()) else { return installs };
    let same = |other: &Path| other == resource || std::fs::canonicalize(other).ok() == std::fs::canonicalize(resource).ok();
    if env.var("KUMI_REAPER_RESOURCE_DIR").is_some() || installs.iter().any(|install| same(&install.resource)) {
        return installs;
    }
    // The app beside its reaper.ini, when it's there.
    let names: &[&str] = match env.os {
        Os::Mac => &["REAPER.app", "REAPER64.app"],
        Os::Windows => &["reaper.exe"],
        Os::Linux => &["reaper"],
    };
    let app = names.iter().map(|name| resource.join(name)).find(|app| app.exists());
    installs.push(reaper_described(env, app.as_ref(), resource.to_path_buf(), ResourceFrom::Portable));
    installs
}

/// `installs`, with the REAPERs running now first (`programs`: their executables, as [`programs`] reads them), so the
/// one the producer has open is the one Kumi's extension goes in. One detection didn't find (a portable REAPER outside
/// the folders Kumi looks in, on another drive say) is added: its resource folder is the `reaper.ini` beside it, else
/// the default one. Not while `KUMI_REAPER_RESOURCE_DIR` names a folder: that one is the only one then.
pub fn with_running(env: &Env, mut installs: Vec<ReaperInstall>, programs: &[PathBuf]) -> Vec<ReaperInstall> {
    if env.var("KUMI_REAPER_RESOURCE_DIR").is_some() {
        return installs;
    }
    let same = |left: &Path, right: &Path| {
        left == right || std::fs::canonicalize(left).is_ok_and(|left| std::fs::canonicalize(right).is_ok_and(|right| left == right))
    };
    let mut running: Vec<ReaperInstall> = vec![];
    for program in programs {
        let app = app_of(env.os, program);
        let portable = reaper_portable_dir(env.os, &app);
        let resource = portable.clone().unwrap_or_else(|| reaper_default_resource(env));
        let mine =
            |install: &ReaperInstall| install.app.as_deref().is_some_and(|known| same(known, &app)) || same(&install.resource, &resource);
        if running.iter().any(mine) {
            continue;
        }
        let install = match installs.iter().position(mine) {
            // Found by detection already: first now, with its app when detection had only the folder.
            Some(at) => {
                let found = installs.remove(at);
                match found.app {
                    Some(_) => found,
                    None => reaper_described(env, Some(&app), found.resource, found.resource_from),
                }
            }
            None => {
                reaper_described(env, Some(&app), resource, if portable.is_some() { ResourceFrom::Portable } else { ResourceFrom::Default })
            }
        };
        running.push(install);
    }
    running.extend(installs);
    running
}

/// A Renoise version, from `V3.5.4` or `3.5.4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('V').unwrap_or(text);
        let mut parts = text.split('.');
        let version = Version(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next().unwrap_or("0").parse().ok()?);
        parts.next().is_none().then_some(version)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The oldest Renoise whose scripting API (6.2) Kumi's tool needs.
pub const RENOISE_MIN: Version = Version(3, 5, 0);
/// The oldest REAPER Kumi's extension is built for.
pub const REAPER_MIN: Version = Version(7, 0, 0);

impl Version {
    /// The version an app says, from its first word ("3.4.4 Demo", "7.22"); None when it doesn't start with one.
    pub fn of_app(text: &str) -> Option<Version> {
        let word = text.split_whitespace().next()?;
        let numbers: String = word.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        Version::parse(numbers.trim_end_matches('.'))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenoiseApp {
    pub path: PathBuf,
    pub version: Option<String>,
    pub demo: Option<bool>,
    pub archs: Vec<Arch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenoisePrefs {
    pub version: Version,
    /// The `V<x.y.z>` folder.
    pub dir: PathBuf,
}

impl RenoisePrefs {
    pub fn tools(&self) -> PathBuf {
        self.dir.join("Scripts").join("Tools")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenoiseInstall {
    pub apps: Vec<RenoiseApp>,
    /// Every `V<x.y.z>` folder found, oldest first.
    pub prefs: Vec<RenoisePrefs>,
    /// Where Kumi's tool goes: the newest folder at or above 3.5. None until a Renoise that new has run once (Kumi
    /// never makes a version folder itself: Renoise copies an older one's settings into a new one on first run).
    pub target: Option<RenoisePrefs>,
    /// `KUMI_RENOISE_PREFS_DIR` chose the folders.
    pub overridden: bool,
}

fn renoise_apps(env: &Env) -> Vec<RenoiseApp> {
    let mut found = vec![];
    match env.os {
        Os::Mac => {
            for dir in [env.system("/Applications"), env.home.join("Applications")] {
                for app in entries(&dir) {
                    let name = name_of(&app);
                    if name.starts_with("Renoise") && name.ends_with(".app") {
                        let version = plist_value(&app, "CFBundleShortVersionString");
                        let demo = Some(name.contains("Demo") || version.as_deref().is_some_and(|v| v.contains("Demo")));
                        let binary = entries(&app.join("Contents").join("MacOS")).into_iter().next();
                        let archs = binary.map(|binary| binary_archs(&binary)).unwrap_or_default();
                        found.push(RenoiseApp { path: app, version, demo, archs });
                    }
                }
            }
        }
        Os::Windows => {
            for var in ["ProgramFiles", "ProgramW6432"] {
                let Some(dir) = env.var_path(var) else { continue };
                for folder in entries(&dir) {
                    let name = name_of(&folder);
                    let exe = folder.join("Renoise.exe");
                    if name.starts_with("Renoise") && exe.exists() && !found.iter().any(|app: &RenoiseApp| app.path == exe) {
                        let version = name.split_whitespace().nth(1).map(str::to_string);
                        found.push(RenoiseApp { archs: binary_archs(&exe), path: exe, version, demo: None });
                    }
                }
            }
        }
        Os::Linux => {
            for dir in [env.system("/usr/local/share"), env.system("/usr/share"), env.system("/opt"), env.home.join(".local").join("share")]
            {
                for folder in entries(&dir) {
                    let name = name_of(&folder);
                    let binary = folder.join("renoise");
                    if name.starts_with("renoise-") && binary.is_file() {
                        let version = name.strip_prefix("renoise-").map(str::to_string);
                        found.push(RenoiseApp { archs: binary_archs(&binary), path: binary, version, demo: None });
                    }
                }
            }
        }
    }
    found
}

fn renoise_prefs_roots(env: &Env) -> Vec<PathBuf> {
    match env.os {
        Os::Mac => vec![env.home.join("Library").join("Preferences").join("Renoise")],
        Os::Windows => vec![env.var_path("APPDATA").unwrap_or_else(|| env.home.join("AppData").join("Roaming")).join("Renoise")],
        Os::Linux => vec![env.config_home().join("Renoise"), env.home.join(".renoise")],
    }
}

fn version_folders(root: &Path) -> Vec<RenoisePrefs> {
    entries(root)
        .into_iter()
        .filter(|dir| dir.is_dir())
        .filter_map(|dir| {
            let name = name_of(&dir);
            Version::parse(name.strip_prefix('V')?).map(|version| RenoisePrefs { version, dir })
        })
        .collect()
}

/// Renoise's apps and preferences folders. `KUMI_RENOISE_PREFS_DIR` names a `V<x.y.z>` folder, or the folder that
/// holds them.
pub fn detect_renoise(env: &Env) -> RenoiseInstall {
    let apps = renoise_apps(env);
    let (mut prefs, overridden) = match env.var_path("KUMI_RENOISE_PREFS_DIR") {
        Some(dir) => {
            let name = name_of(&dir);
            match name.strip_prefix('V').and_then(Version::parse) {
                Some(version) => (vec![RenoisePrefs { version, dir }], true),
                None => (version_folders(&dir), true),
            }
        }
        None => (renoise_prefs_roots(env).iter().flat_map(|root| version_folders(root)).collect::<Vec<_>>(), false),
    };
    prefs.sort_by_key(|prefs| prefs.version);
    let target = prefs.iter().rev().find(|prefs| prefs.version >= RENOISE_MIN).cloned();
    RenoiseInstall { apps, prefs, target, overridden }
}

/// What a Renoise version folder's `Config.xml` says that Kumi cares about.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenoiseConfig {
    /// The built-in OSC server is on (it takes unauthenticated commands from the network).
    pub osc_server: bool,
    /// Kumi's tool is in the disabled tools list.
    pub tool_disabled: bool,
}

pub fn renoise_config(prefs: &Path, tool_id: &str) -> Option<RenoiseConfig> {
    let text = std::fs::read_to_string(prefs.join("Config.xml")).ok()?;
    let element = |name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{name}>"))?;
        Some(text[start..start + end].trim().to_string())
    };
    Some(RenoiseConfig {
        osc_server: element("OscServerEnabled").is_some_and(|value| value.eq_ignore_ascii_case("true")),
        tool_disabled: element("DisabledScriptingTools").is_some_and(|value| value.contains(tool_id)),
    })
}

/// Where Renoise's user library may be, likeliest first: in the Documents folder Windows says it keeps (OneDrive, or the
/// producer, can move it out of the home folder), and in the home folder's, where it's guessed when Windows doesn't say or
/// on any other system.
pub fn renoise_user_libraries(home: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    let documents = windows_documents();
    #[cfg(not(windows))]
    let documents = None;
    user_libraries_in(documents, home)
}

fn user_libraries_in(documents: Option<PathBuf>, home: &Path) -> Vec<PathBuf> {
    let library = |documents: &Path| documents.join("Renoise").join("User Library");
    let guess = library(&home.join("Documents"));
    let mut libraries: Vec<PathBuf> = documents.map(|documents| library(&documents)).into_iter().collect();
    if !libraries.contains(&guess) {
        libraries.push(guess);
    }
    libraries
}

/// The Documents folder Windows keeps for this user, wherever it was moved to; None when it won't say.
#[cfg(windows)]
fn windows_documents() -> Option<PathBuf> {
    use std::{ffi::c_void, os::windows::ffi::OsStringExt};
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    // FOLDERID_Documents: {FDD39AD0-238F-46AF-ADB4-6C85480369C7}.
    const DOCUMENTS: Guid = Guid(0xFDD39AD0, 0x238F, 0x46AF, [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7]);
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(id: *const Guid, flags: u32, token: *mut c_void, path: *mut *mut u16) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(memory: *mut c_void);
    }
    let mut path: *mut u16 = std::ptr::null_mut();
    // SAFETY: the id and the out pointer live through the call, which sets the pointer to a string it allocates, ended by a
    // zero: it's read up to that zero, and freed as the call asks, whatever the call returned.
    let result = unsafe { SHGetKnownFolderPath(&DOCUMENTS, 0, std::ptr::null_mut(), &mut path) };
    let found = (result == 0 && !path.is_null()).then(|| {
        let length = (0..).take_while(|&at| unsafe { *path.add(at) } != 0).count();
        PathBuf::from(std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(path, length) }))
    });
    if !path.is_null() {
        unsafe { CoTaskMemFree(path.cast()) };
    }
    found
}

/// What stands in for the listing of this computer's processes when it's set: lines of "pid program", as `ps` writes
/// them (other lines list nothing). The isolated test runner sets it to `none`, so no test sees a DAW running on the
/// computer it runs on.
pub const PROCESSES: &str = "KUMI_PROCESSES";

/// This computer's processes (pid and program; macOS's `ps` names each program by its path), from one `ps`
/// (`tasklist` on Windows), or what `KUMI_PROCESSES` says, in `var` or this process's environment; empty when the
/// listing can't be read.
pub fn list_processes(var: &dyn Fn(&str) -> Option<String>) -> Vec<(u32, String)> {
    listed_as(stand_in(var).as_deref())
}

fn stand_in(var: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    var(PROCESSES).or_else(|| std::env::var(PROCESSES).ok())
}

/// The stand-in's processes, else this computer's.
fn listed_as(stand_in: Option<&str>) -> Vec<(u32, String)> {
    match stand_in {
        Some(listing) => parse_processes(listing, false),
        None => listing_now(),
    }
}

/// This computer's processes, listed now: Windows' from a snapshot (tasklist, when one can't be taken).
fn listing_now() -> Vec<(u32, String)> {
    #[cfg(windows)]
    {
        let listed = snapshot();
        if !listed.is_empty() {
            return listed;
        }
    }
    let listing = if cfg!(windows) {
        std::process::Command::new("tasklist").args(["/FO", "CSV", "/NH"]).output()
    } else {
        std::process::Command::new("ps").args(["-axo", "pid=,comm="]).output()
    };
    let Ok(output) = listing else { return vec![] };
    parse_processes(&String::from_utf8_lossy(&output.stdout), cfg!(windows))
}

/// Windows' processes by their image names, from a Toolhelp snapshot: a moment's work, where tasklist takes a good part
/// of a second. Empty when it can't be taken.
#[cfg(windows)]
fn snapshot() -> Vec<(u32, String)> {
    type Handle = *mut std::ffi::c_void;
    /// PROCESSENTRY32W: Windows fills in every field; Kumi reads the pid and the name.
    #[repr(C)]
    #[allow(dead_code)]
    struct Entry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        exe: [u16; 260],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut Entry) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut Entry) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    const SNAPPROCESS: u32 = 0x2;
    let mut listed = vec![];
    // SAFETY: the snapshot is checked before use and closed once; `size` tells each call the entry's layout, and the name
    // is read up to its end within the array.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(SNAPPROCESS, 0);
        if snapshot.is_null() || snapshot as isize == -1 {
            return listed;
        }
        let mut entry: Entry = std::mem::zeroed();
        entry.size = std::mem::size_of::<Entry>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let end = entry.exe.iter().position(|unit| *unit == 0).unwrap_or(entry.exe.len());
            listed.push((entry.pid, String::from_utf16_lossy(&entry.exe[..end])));
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    listed
}

/// How long one listing answers for every DAW: choosing the DAW a session works with, saying which others are open,
/// and each look for a DAW the producer opened list the processes once.
const LISTING: std::time::Duration = std::time::Duration::from_secs(2);

type Listed = std::rc::Rc<Vec<(u32, String)>>;

thread_local! {
    static KEPT: std::cell::RefCell<Option<(std::time::Instant, Option<String>, Listed)>> = const { std::cell::RefCell::new(None) };
    static LISTINGS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// [`list_processes`], made once and reused for a moment, so every DAW asked whether it runs reads the same listing.
pub fn processes(var: &dyn Fn(&str) -> Option<String>) -> Listed {
    let stand_in = stand_in(var);
    let kept = KEPT.with(|kept| kept.borrow().clone()).filter(|(at, said, _)| *said == stand_in && at.elapsed() < LISTING);
    if let Some((_, _, listed)) = kept {
        return listed;
    }
    LISTINGS.with(|count| count.set(count.get() + 1));
    let listed = std::rc::Rc::new(listed_as(stand_in.as_deref()));
    KEPT.with(|kept| *kept.borrow_mut() = Some((std::time::Instant::now(), stand_in, listed.clone())));
    listed
}

/// How many listings [`processes`] has made on this thread.
pub fn listings() -> u64 {
    LISTINGS.with(std::cell::Cell::get)
}

/// The pids of this DAW's running processes, from a listing made now; empty when it can't be read.
pub fn running(daw: Daw) -> Vec<u32> {
    list_processes(&|_| None).into_iter().filter(|(_, name)| process_is(daw, name)).map(|(pid, _)| pid).collect()
}

/// The programs of the DAW's processes in a listing, by their paths: as listed where the listing names a path (macOS's
/// `ps`, a stand-in), else as the system names the process's own (Linux's `/proc`, Windows' image name). One the system
/// won't say is left out.
pub fn programs(daw: Daw, listing: &[(u32, String)]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = vec![];
    for (pid, name) in listing.iter().filter(|(_, name)| process_is(daw, name)) {
        let path = if name.contains(['/', '\\']) { Some(PathBuf::from(name)) } else { program_of(*pid) };
        if let Some(path) = path.filter(|path| !found.contains(path)) {
            found.push(path);
        }
    }
    found
}

/// The DAW's programs running now, from the listing kept a moment ([`processes`]).
pub fn running_programs(daw: Daw, env: &Env) -> Vec<PathBuf> {
    programs(daw, &processes(env.var))
}

#[cfg(target_os = "linux")]
fn program_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(windows)]
fn program_of(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn QueryFullProcessImageNameW(process: Handle, flags: u32, name: *mut u16, size: *mut u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let mut name = vec![0u16; 32_768];
    let mut size = name.len() as u32;
    // SAFETY: the handle is checked before use and closed once; the buffer holds `size` UTF-16 units, which the call
    // reduces to the length it wrote.
    let read = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let read = QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut size);
        CloseHandle(process);
        read
    };
    (read != 0).then(|| PathBuf::from(std::ffi::OsString::from_wide(&name[..size as usize])))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn program_of(_pid: u32) -> Option<PathBuf> {
    None
}

/// The app a program belongs to, as detection names apps: on macOS the `.app` it's inside, elsewhere the program.
pub fn app_of(os: Os, program: &Path) -> PathBuf {
    let bundle = (os == Os::Mac).then(|| program.ancestors().find(|dir| dir.extension().is_some_and(|ext| ext == "app"))).flatten();
    bundle.unwrap_or(program).to_path_buf()
}

pub fn parse_processes(listing: &str, windows: bool) -> Vec<(u32, String)> {
    listing
        .lines()
        .filter_map(|line| {
            if windows {
                let fields: Vec<&str> = line.split("\",\"").map(|field| field.trim_matches('"')).collect();
                Some((fields.get(1)?.parse().ok()?, fields.first()?.to_string()))
            } else {
                let line = line.trim_start();
                let (pid, name) = line.split_once(char::is_whitespace)?;
                Some((pid.parse().ok()?, name.trim().to_string()))
            }
        })
        .collect()
}

/// Whether a process name (a path or an image name) is this DAW's main program.
pub fn process_is(daw: Daw, name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base);
    match daw {
        Daw::Reaper => matches!(base, "REAPER" | "REAPER64" | "reaper"),
        Daw::Renoise => base == "Renoise" || base == "renoise" || base.starts_with("Renoise ") || base.starts_with("renoise-"),
    }
}

/// Whether macOS marked the file as downloaded (Gatekeeper then blocks a library loading from it).
pub fn quarantined(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
        let name = c"com.apple.quarantine";
        let size = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, libc::XATTR_NOFOLLOW) };
        size >= 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

/// Takes macOS's downloaded mark off a file or folder and everything in it: a copy keeps the mark of what it was copied
/// from, and a DAW won't load a marked library. For Kumi's own copies only, never what isn't Kumi's.
pub fn unquarantine(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let name = c"com.apple.quarantine";
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        if unsafe { libc::removexattr(c_path.as_ptr(), name.as_ptr(), libc::XATTR_NOFOLLOW) } != 0 {
            let error = std::io::Error::last_os_error();
            if !unmarked(error.raw_os_error()) {
                return Err(error);
            }
        }
        if std::fs::symlink_metadata(path)?.is_dir() {
            for entry in std::fs::read_dir(path)? {
                unquarantine(&entry?.path())?;
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Ok(())
    }
}

/// Whether removing the downloaded mark failed only because there's none to remove: the file has no mark, or its disk
/// keeps no extended attributes at all (an NFS share, a FAT disk), where no mark can be either.
#[cfg(target_os = "macos")]
fn unmarked(errno: Option<i32>) -> bool {
    matches!(errno, Some(libc::ENOATTR) | Some(libc::ENOTSUP))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renoises_user_library_is_looked_for_in_the_documents_folder_the_system_says_then_the_homes() {
        let home = Path::new("home");
        let in_home = home.join("Documents").join("Renoise").join("User Library");
        let moved = Path::new("OneDrive").join("Documents");
        assert_eq!(user_libraries_in(Some(moved.clone()), home), [moved.join("Renoise").join("User Library"), in_home.clone()]);
        // Not moved, or the system doesn't say: the home folder's, once.
        assert_eq!(user_libraries_in(Some(home.join("Documents")), home), [in_home.clone()]);
        assert_eq!(user_libraries_in(None, home), [in_home]);
    }

    /// The call itself, which only Windows has.
    #[cfg(windows)]
    #[test]
    fn windows_says_where_its_documents_folder_is() {
        assert!(windows_documents().is_some_and(|documents| documents.is_absolute()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_disk_without_extended_attributes_has_no_mark_to_take_off() {
        assert!(unmarked(Some(libc::ENOATTR)));
        assert!(unmarked(Some(libc::ENOTSUP)), "an NFS share or a FAT disk: no mark, nothing to fail over");
        assert!(!unmarked(Some(libc::EACCES)));
        assert!(!unmarked(None));
    }

    fn env<'a>(os: Os, root: &Path, vars: &'a dyn Fn(&str) -> Option<String>) -> Env<'a> {
        Env { os, home: root.join("home"), var: vars, root: Some(root.join("sys")) }
    }

    fn touch(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn mach_universal() -> Vec<u8> {
        let mut bytes = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2];
        for cpu in [0x0100_0007u32, 0x0100_000c] {
            bytes.extend(cpu.to_be_bytes());
            bytes.extend([0u8; 16]);
        }
        bytes
    }

    #[test]
    fn architectures_of_each_binary_format() {
        assert_eq!(archs_of(&mach_universal()), [Arch::X86_64, Arch::Arm64]);
        assert_eq!(archs_of(&[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01]), [Arch::Arm64]);
        let mut elf = vec![0x7f, b'E', b'L', b'F'];
        elf.resize(18, 0);
        elf.extend([0xb7, 0]);
        assert_eq!(archs_of(&elf), [Arch::Arm64]);
        let mut pe = vec![0u8; 0x80];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c] = 0x40;
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x44..0x46].copy_from_slice(&0xa641u16.to_le_bytes());
        assert_eq!(archs_of(&pe), [Arch::Arm64ec]);
        assert!(archs_of(b"#!/bin/sh").is_empty());
    }

    #[test]
    fn reaper_on_macos_default_and_portable() {
        let root = tempfile::tempdir().unwrap();
        let none = |_: &str| None;
        let app = root.path().join("sys/Applications/REAPER.app");
        touch(
            &app.join("Contents/Info.plist"),
            b"<plist><dict><key>CFBundleShortVersionString</key>\n<string>7.82</string></dict></plist>",
        );
        touch(&app.join("Contents/MacOS/REAPER"), &mach_universal());
        let lab = root.path().join("home/lab/REAPER.app");
        touch(&lab.join("Contents/MacOS/REAPER"), &mach_universal());
        // A portable install outside the searched folders isn't found without the override.
        let found = detect_reaper(&env(Os::Mac, root.path(), &none));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version.as_deref(), Some("7.82"));
        assert_eq!(found[0].archs, [Arch::X86_64, Arch::Arm64]);
        assert_eq!(found[0].resource, root.path().join("home/Library/Application Support/REAPER"));
        assert!(!found[0].opened);

        // A reaper.ini beside the app makes it portable.
        let portable = root.path().join("home/Applications/REAPER.app");
        touch(&portable.join("Contents/MacOS/REAPER"), &mach_universal());
        touch(&root.path().join("home/Applications/reaper.ini"), b"[REAPER]\n");
        let found = detect_reaper(&env(Os::Mac, root.path(), &none));
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].resource_from, ResourceFrom::Portable);
        assert_eq!(found[1].resource, root.path().join("home/Applications"));
        assert!(found[1].opened);

        // The override wins and is the only one.
        let lab_dir = root.path().join("home/lab").to_string_lossy().into_owned();
        let vars = move |name: &str| (name == "KUMI_REAPER_RESOURCE_DIR").then(|| lab_dir.clone());
        let found = detect_reaper(&env(Os::Mac, root.path(), &vars));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].resource_from, ResourceFrom::Override);
    }

    #[test]
    fn reaper_on_windows_and_linux() {
        let root = tempfile::tempdir().unwrap();
        let program_files = root.path().join("pf").to_string_lossy().into_owned();
        let appdata = root.path().join("appdata").to_string_lossy().into_owned();
        let vars = move |name: &str| match name {
            "ProgramFiles" => Some(program_files.clone()),
            "APPDATA" => Some(appdata.clone()),
            _ => None,
        };
        touch(&root.path().join("pf/REAPER (x64)/reaper.exe"), b"MZ");
        let found = detect_reaper(&env(Os::Windows, root.path(), &vars));
        assert_eq!(found[0].resource, root.path().join("appdata/REAPER"));
        assert!(found[0].app.as_ref().unwrap().ends_with("reaper.exe"));

        let none = |_: &str| None;
        touch(&root.path().join("home/.reaper/reaper.ini"), b"");
        let found = detect_reaper(&env(Os::Linux, root.path(), &none));
        assert_eq!(found[0].resource, root.path().join("home/.reaper"));
        assert!(found[0].app.is_none() && found[0].opened);
        touch(&root.path().join("home/opt/REAPER/reaper"), b"");
        touch(&root.path().join("home/opt/REAPER/reaper.ini"), b"");
        let found = detect_reaper(&env(Os::Linux, root.path(), &none));
        assert_eq!(found.iter().map(|install| install.resource_from).collect::<Vec<_>>(), [ResourceFrom::Default, ResourceFrom::Portable]);
    }

    /// A REAPER the producer has open is the one Kumi's extension goes in, wherever it runs from: a portable one outside
    /// the folders Kumi looks in is found by its program and the reaper.ini beside it, and comes first.
    #[test]
    fn the_reaper_running_comes_first_wherever_it_is() {
        let root = tempfile::tempdir().unwrap();
        let none = |_: &str| None;
        let app = root.path().join("sys/Applications/REAPER.app");
        touch(&app.join("Contents/MacOS/REAPER"), &mach_universal());
        touch(&root.path().join("home/Library/Application Support/REAPER/reaper.ini"), b"[REAPER]\n");
        // A portable REAPER on another drive, with its reaper.ini beside it: not found until it runs.
        let lab = root.path().join("Volumes/Music/REAPER Lab");
        touch(&lab.join("REAPER.app/Contents/Info.plist"), b"<key>CFBundleShortVersionString</key><string>7.82</string>");
        touch(&lab.join("REAPER.app/Contents/MacOS/REAPER"), &mach_universal());
        touch(&lab.join("reaper.ini"), b"[REAPER]\n");
        let mac = env(Os::Mac, root.path(), &none);
        let found = detect_reaper(&mac);
        assert_eq!(found.len(), 1);
        assert_eq!(with_running(&mac, found.clone(), &[]), found, "nothing running: as detection found them");

        let listing = format!("  101 {}\n  202 /usr/sbin/cfprefsd\n", lab.join("REAPER.app/Contents/MacOS/REAPER").display());
        let running = programs(Daw::Reaper, &parse_processes(&listing, false));
        assert_eq!(running, [lab.join("REAPER.app/Contents/MacOS/REAPER")]);
        let installs = with_running(&mac, found.clone(), &running);
        assert_eq!(installs.len(), 2);
        assert_eq!((installs[0].resource.clone(), installs[0].resource_from), (lab.clone(), ResourceFrom::Portable));
        assert_eq!(installs[0].app.as_deref(), Some(lab.join("REAPER.app").as_path()));
        assert_eq!(installs[0].version.as_deref(), Some("7.82"));
        assert!(installs[0].opened);
        assert_eq!(crate::install::reaper_install(&installs).unwrap().resource, lab, "the extension goes in the one open");

        // The default REAPER running: the one detection found comes first, once, with its app.
        let listing = format!("101 {}\n102 {}\n", app.join("Contents/MacOS/REAPER").display(), app.join("Contents/MacOS/REAPER").display());
        let installs = with_running(&mac, detect_reaper(&mac), &programs(Daw::Reaper, &parse_processes(&listing, false)));
        assert_eq!(installs, detect_reaper(&mac), "two of the same REAPER: one row");
        // Running from elsewhere without a reaper.ini beside it, it uses the default resource folder: that row, with this app.
        let elsewhere = root.path().join("home/Downloads/REAPER.app");
        touch(&elsewhere.join("Contents/MacOS/REAPER"), &mach_universal());
        let installs = with_running(&mac, vec![], &[elsewhere.join("Contents/MacOS/REAPER")]);
        assert_eq!(
            (installs[0].resource.clone(), installs[0].resource_from),
            (root.path().join("home/Library/Application Support/REAPER"), ResourceFrom::Default)
        );
        assert_eq!(installs[0].app.as_deref(), Some(elsewhere.as_path()));

        // KUMI_REAPER_RESOURCE_DIR names the only one.
        let chosen = root.path().join("home/chosen").to_string_lossy().into_owned();
        let vars = move |name: &str| (name == "KUMI_REAPER_RESOURCE_DIR").then(|| chosen.clone());
        let named = env(Os::Mac, root.path(), &vars);
        let found = detect_reaper(&named);
        assert_eq!(with_running(&named, found.clone(), &running), found);
    }

    /// The programs of a DAW's processes, by path: macOS's `ps` names them so; the app is the `.app` around a Mac's.
    #[test]
    fn a_daws_programs_and_their_apps() {
        let listing = parse_processes(
            "  1 /sbin/launchd\n 61153 /Users/me/lab/REAPER.app/Contents/MacOS/REAPER\n 7 /Applications/Renoise.app/Contents/MacOS/Renoise\n",
            false,
        );
        assert_eq!(programs(Daw::Reaper, &listing), [PathBuf::from("/Users/me/lab/REAPER.app/Contents/MacOS/REAPER")]);
        assert_eq!(programs(Daw::Renoise, &listing), [PathBuf::from("/Applications/Renoise.app/Contents/MacOS/Renoise")]);
        assert_eq!(app_of(Os::Mac, Path::new("/Users/me/lab/REAPER.app/Contents/MacOS/REAPER")), PathBuf::from("/Users/me/lab/REAPER.app"));
        assert_eq!(app_of(Os::Linux, Path::new("/opt/REAPER/reaper")), PathBuf::from("/opt/REAPER/reaper"));
        assert_eq!(app_of(Os::Windows, Path::new(r"D:\REAPER\reaper.exe")), PathBuf::from(r"D:\REAPER\reaper.exe"));
        assert_eq!(app_of(Os::Mac, Path::new("/usr/local/bin/reaper")), PathBuf::from("/usr/local/bin/reaper"), "no app around it");
    }

    /// KUMI_PROCESSES stands in for the listing, in the environment given (else this process's): lines of "pid
    /// program", anything else listing nothing. A listing is kept a moment, for that same stand-in only.
    #[test]
    fn a_stand_in_lists_the_processes_and_a_listing_is_kept_a_moment() {
        let renoise = |name: &str| (name == PROCESSES).then(|| "7 /Applications/Renoise.app/Contents/MacOS/Renoise".to_string());
        assert_eq!(list_processes(&renoise), [(7, "/Applications/Renoise.app/Contents/MacOS/Renoise".to_string())]);
        let nothing = |name: &str| (name == PROCESSES).then(|| "none".to_string());
        assert!(list_processes(&nothing).is_empty());
        let before = listings();
        assert_eq!(processes(&renoise).len(), 1);
        assert_eq!(processes(&renoise).len(), 1);
        assert_eq!(listings() - before, 1, "the same stand-in: one listing");
        assert!(processes(&nothing).is_empty(), "another stand-in: listed again");
        assert_eq!(listings() - before, 2);
        assert_eq!(
            running_programs(Daw::Renoise, &env(Os::Mac, Path::new("/"), &renoise)),
            [PathBuf::from("/Applications/Renoise.app/Contents/MacOS/Renoise")]
        );
    }

    /// This computer's listing has this test's own process in it, and where Kumi asks the system for a program's path
    /// (Linux's /proc, Windows' image name) it names this test's.
    #[test]
    fn this_computers_listing_has_this_test_in_it() {
        let pid = std::process::id();
        let listed = listing_now();
        assert!(listed.iter().any(|(listed, _)| *listed == pid), "{pid} in {listed:?}");
        if cfg!(any(target_os = "linux", windows)) {
            let canonical = |path: PathBuf| std::fs::canonicalize(&path).unwrap_or(path);
            assert_eq!(program_of(pid).map(canonical), std::env::current_exe().ok().map(canonical));
        }
    }

    #[test]
    fn renoise_targets_the_newest_version_folder_at_or_above_3_5() {
        let root = tempfile::tempdir().unwrap();
        let none = |_: &str| None;
        let prefs = root.path().join("home/Library/Preferences/Renoise");
        for version in ["V3.4.4", "V3.5.4", "V3.5.10", "Vx", "V4"] {
            std::fs::create_dir_all(prefs.join(version)).unwrap();
        }
        touch(
            &root.path().join("sys/Applications/Renoise.app/Contents/Info.plist"),
            b"<key>CFBundleShortVersionString</key><string>3.5.4</string>",
        );
        let found = detect_renoise(&env(Os::Mac, root.path(), &none));
        assert_eq!(found.prefs.iter().map(|p| p.version.to_string()).collect::<Vec<_>>(), ["3.4.4", "3.5.4", "3.5.10"]);
        assert_eq!(found.target.as_ref().unwrap().version, Version(3, 5, 10));
        assert_eq!(found.apps[0].version.as_deref(), Some("3.5.4"));
        assert_eq!(found.apps[0].demo, Some(false));

        std::fs::remove_dir(prefs.join("V3.5.10")).unwrap();
        std::fs::remove_dir(prefs.join("V3.5.4")).unwrap();
        assert!(detect_renoise(&env(Os::Mac, root.path(), &none)).target.is_none());

        let chosen = prefs.join("V3.4.4").to_string_lossy().into_owned();
        let vars = move |name: &str| (name == "KUMI_RENOISE_PREFS_DIR").then(|| chosen.clone());
        let found = detect_renoise(&env(Os::Linux, root.path(), &vars));
        assert!(found.overridden && found.target.is_none() && found.prefs.len() == 1);

        let xdg = root.path().join("xdg").to_string_lossy().into_owned();
        std::fs::create_dir_all(root.path().join("xdg/Renoise/V3.5.4")).unwrap();
        touch(&root.path().join("sys/usr/local/share/renoise-3.5.4/renoise"), b"");
        let vars = move |name: &str| (name == "XDG_CONFIG_HOME").then(|| xdg.clone());
        let found = detect_renoise(&env(Os::Linux, root.path(), &vars));
        assert_eq!(found.target.unwrap().dir, root.path().join("xdg/Renoise/V3.5.4"));
        assert_eq!(found.apps[0].version.as_deref(), Some("3.5.4"));
    }

    #[test]
    fn renoise_config_and_processes() {
        let root = tempfile::tempdir().unwrap();
        touch(
            &root.path().join("Config.xml"),
            b"<RenoisePrefs><OscServerEnabled>true</OscServerEnabled><DisabledScriptingTools>com.kumi.Bridge</DisabledScriptingTools></RenoisePrefs>",
        );
        assert_eq!(renoise_config(root.path(), "com.kumi.Bridge"), Some(RenoiseConfig { osc_server: true, tool_disabled: true }));
        let listing =
            "  101 /Applications/REAPER.app/Contents/MacOS/REAPER\n  202 /Applications/Renoise.app/Contents/MacOS/Renoise\n 303 zsh\n";
        let processes = parse_processes(listing, false);
        assert_eq!(processes.len(), 3);
        assert!(process_is(Daw::Reaper, &processes[0].1) && process_is(Daw::Renoise, &processes[1].1));
        assert!(!process_is(Daw::Reaper, "reaper_kumi_helper") && !process_is(Daw::Renoise, "zsh"));
        let windows = parse_processes("\"reaper.exe\",\"4242\",\"Console\",\"1\",\"300,000 K\"\r\n", true);
        assert_eq!(windows, [(4242, "reaper.exe".to_string())]);
        assert!(process_is(Daw::Reaper, "reaper.exe"));
    }
}
