//! Finding REAPER, Renoise and Logic Pro on this computer: the apps, their versions and architectures, REAPER's
//! resource folders (portable installs included) and Renoise's per-version preferences folders. Read-only: nothing
//! is launched and no permission is asked for.
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

/// A string value from a macOS app's `Contents/Info.plist`, in its XML or its binary form (the App Store's apps,
/// Logic Pro among them, keep a binary one).
fn plist_value(app: &Path, key: &str) -> Option<String> {
    let bytes = std::fs::read(app.join("Contents").join("Info.plist")).ok()?;
    if bytes.starts_with(b"bplist") {
        return bplist_string(&bytes, key);
    }
    let text = String::from_utf8(bytes).ok()?;
    let at = text.find(&format!("<key>{key}</key>"))?;
    let rest = &text[at..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_string())
}

/// The string under `key` in a binary plist's top dictionary (`bplist00`). Only what detection needs: the trailer, the
/// offset table, one dictionary, and ASCII or UTF-16 strings; anything else answers None.
fn bplist_string(bytes: &[u8], key: &str) -> Option<String> {
    if bytes.len() < 40 || !bytes.starts_with(b"bplist0") {
        return None;
    }
    let trailer = &bytes[bytes.len() - 32..];
    let offset_size = trailer[6] as usize;
    let ref_size = trailer[7] as usize;
    let be64 = |at: usize| u64::from_be_bytes(trailer[at..at + 8].try_into().expect("8 bytes"));
    let count = usize::try_from(be64(8)).ok()?;
    let top = usize::try_from(be64(16)).ok()?;
    let table = usize::try_from(be64(24)).ok()?;
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) || count > 1 << 20 {
        return None;
    }
    let uint = |at: usize, size: usize| -> Option<usize> {
        let value = bytes.get(at..at + size)?.iter().fold(0u64, |n, byte| (n << 8) | *byte as u64);
        usize::try_from(value).ok()
    };
    let offset = |index: usize| -> Option<usize> { (index < count).then(|| uint(table + index * offset_size, offset_size)).flatten() };
    // An object's type (its marker's high nibble), count (the low one, or the int object after it when that is 0xF), and
    // where its contents start.
    let header = |at: usize| -> Option<(u8, usize, usize)> {
        let marker = *bytes.get(at)?;
        let (kind, small) = (marker >> 4, (marker & 0x0F) as usize);
        if small == 0x0F && matches!(kind, 0x4 | 0x5 | 0x6 | 0xA | 0xC | 0xD) {
            let int_marker = *bytes.get(at + 1)?;
            if int_marker >> 4 != 0x1 {
                return None;
            }
            let size = 1usize << (int_marker & 0x0F);
            return Some((kind, uint(at + 2, size)?, at + 2 + size));
        }
        Some((kind, small, at + 1))
    };
    let string_at = |index: usize| -> Option<String> {
        let (kind, n, start) = header(offset(index)?)?;
        match kind {
            0x5 => String::from_utf8(bytes.get(start..start + n)?.to_vec()).ok(),
            0x6 => {
                let units: Vec<u16> =
                    bytes.get(start..start + n * 2)?.chunks(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
                String::from_utf16(&units).ok()
            }
            _ => None,
        }
    };
    let (kind, n, start) = header(offset(top)?)?;
    if kind != 0xD {
        return None;
    }
    (0..n).find_map(|i| {
        let key_ref = uint(start + i * ref_size, ref_size)?;
        (string_at(key_ref).as_deref() == Some(key)).then(|| string_at(uint(start + (n + i) * ref_size, ref_size)?)).flatten()
    })
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

/// Logic Pro's bundle ids: the App Store build, and the Creator Studio build.
pub const LOGIC_BUNDLES: [&str; 2] = ["com.apple.logic10", "com.apple.mobilelogic"];
/// The oldest Logic Pro Kumi's bridge serves (Logic Pro 12, on Apple Silicon).
pub const LOGIC_MIN: Version = Version(12, 0, 0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicInstall {
    /// The app bundle (`/Applications/Logic Pro.app`).
    pub app: PathBuf,
    /// Its bundle id, one of [`LOGIC_BUNDLES`] (whatever its plist says for the app `KUMI_LOGIC_APP` names).
    pub bundle: String,
    /// `CFBundleShortVersionString`, when it reads as one.
    pub version: Option<Version>,
    /// The app's executable's architectures (universal on the App Store build).
    pub archs: Vec<Arch>,
}

fn logic_install(app: &Path) -> LogicInstall {
    let bundle = plist_value(app, "CFBundleIdentifier").unwrap_or_default();
    let version = plist_value(app, "CFBundleShortVersionString").and_then(|version| Version::parse(&version));
    let executable = plist_value(app, "CFBundleExecutable").unwrap_or_else(|| "Logic Pro".into());
    let archs = binary_archs(&app.join("Contents").join("MacOS").join(executable));
    LogicInstall { app: app.to_path_buf(), bundle, version, archs }
}

/// Every Logic Pro on this Mac: `/Applications/Logic Pro.app` first, then any other app in `/Applications` or
/// `~/Applications` whose bundle id is Logic's. `KUMI_LOGIC_APP` names the app instead, which then is the only one (and
/// none when it isn't there).
pub fn detect_logic(env: &Env) -> Vec<LogicInstall> {
    if let Some(app) = env.var_path("KUMI_LOGIC_APP") {
        return if app.is_dir() { vec![logic_install(&app)] } else { vec![] };
    }
    if env.os != Os::Mac {
        return vec![];
    }
    let usual = env.system("/Applications").join("Logic Pro.app");
    let mut candidates = vec![usual.clone()];
    for dir in [env.system("/Applications"), env.home.join("Applications")] {
        candidates.extend(entries(&dir).into_iter().filter(|app| name_of(app).ends_with(".app")));
    }
    let mut found: Vec<LogicInstall> = vec![];
    for app in candidates {
        if !app.is_dir() || found.iter().any(|install| install.app == app) {
            continue;
        }
        let install = logic_install(&app);
        if app == usual || LOGIC_BUNDLES.contains(&install.bundle.as_str()) {
            found.push(install);
        }
    }
    found
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

/// The pids of this DAW's running processes, from one `ps` (or `tasklist`); empty when it can't be read.
pub fn running(daw: Daw) -> Vec<u32> {
    let listing = if cfg!(windows) {
        std::process::Command::new("tasklist").args(["/FO", "CSV", "/NH"]).output()
    } else {
        std::process::Command::new("ps").args(["-axo", "pid=,comm="]).output()
    };
    let Ok(output) = listing else { return vec![] };
    parse_processes(&String::from_utf8_lossy(&output.stdout), cfg!(windows))
        .into_iter()
        .filter(|(_, name)| process_is(daw, name))
        .map(|(pid, _)| pid)
        .collect()
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
        Daw::Logic => base == "Logic Pro",
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

    /// A binary plist of one dictionary of ASCII strings, as `plutil -convert binary1` lays one out.
    fn bplist(pairs: &[(&str, &str)]) -> Vec<u8> {
        let ascii = |text: &str| -> Vec<u8> {
            let mut object = if text.len() < 15 { vec![0x50 | text.len() as u8] } else { vec![0x5F, 0x10, text.len() as u8] };
            object.extend(text.as_bytes());
            object
        };
        let mut out = b"bplist00".to_vec();
        let mut offsets = vec![out.len()];
        out.push(0xD0 | pairs.len() as u8);
        out.extend((1..=pairs.len()).map(|i| i as u8));
        out.extend((1..=pairs.len()).map(|i| (pairs.len() + i) as u8));
        for text in pairs.iter().map(|(key, _)| *key).chain(pairs.iter().map(|(_, value)| *value)) {
            offsets.push(out.len());
            out.extend(ascii(text));
        }
        let table = out.len();
        out.extend(offsets.iter().map(|offset| *offset as u8));
        out.extend([0, 0, 0, 0, 0, 0, 1, 1]);
        out.extend((1 + 2 * pairs.len() as u64).to_be_bytes());
        out.extend(0u64.to_be_bytes());
        out.extend((table as u64).to_be_bytes());
        out
    }

    #[test]
    fn logic_by_name_or_bundle_id_in_applications_or_where_the_override_says() {
        let root = tempfile::tempdir().unwrap();
        let none = |_: &str| None;
        let apps = root.path().join("sys/Applications");
        // The App Store build keeps a binary Info.plist, and a universal executable.
        let logic = apps.join("Logic Pro.app");
        let plist = bplist(&[
            ("CFBundleExecutable", "Logic Pro"),
            ("CFBundleIdentifier", "com.apple.logic10"),
            ("CFBundleShortVersionString", "12.3.1"),
        ]);
        touch(&logic.join("Contents/Info.plist"), &plist);
        touch(&logic.join("Contents/MacOS/Logic Pro"), &mach_universal());
        // The Creator Studio build under another name (an XML plist), and an app that isn't Logic.
        let studio = apps.join("Logic.app");
        touch(
            &studio.join("Contents/Info.plist"),
            b"<plist><dict><key>CFBundleExecutable</key><string>Logic</string><key>CFBundleIdentifier</key><string>com.apple.mobilelogic</string><key>CFBundleShortVersionString</key><string>12.4</string></dict></plist>",
        );
        touch(&studio.join("Contents/MacOS/Logic"), &[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01]);
        touch(&apps.join("Other.app/Contents/Info.plist"), b"<key>CFBundleIdentifier</key><string>com.example.other</string>");
        let found = detect_logic(&env(Os::Mac, root.path(), &none));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((&found[0].app, found[0].bundle.as_str(), found[0].version), (&logic, "com.apple.logic10", Some(Version(12, 3, 1))));
        assert_eq!(found[0].archs, [Arch::X86_64, Arch::Arm64]);
        assert_eq!(
            (&found[1].app, found[1].bundle.as_str(), found[1].version),
            (&studio, "com.apple.mobilelogic", Some(Version(12, 4, 0)))
        );
        assert_eq!(found[1].archs, [Arch::Arm64]);
        assert!(found[0].version < found[1].version && found[0].version >= Some(LOGIC_MIN));
        // Only on a Mac.
        assert!(detect_logic(&env(Os::Linux, root.path(), &none)).is_empty());

        // The override is the only one, wherever it is; one that isn't there finds nothing.
        let lab = root.path().join("home/lab/Logic Pro.app");
        touch(
            &lab.join("Contents/Info.plist"),
            &bplist(&[("CFBundleIdentifier", "com.apple.logic10"), ("CFBundleShortVersionString", "11.2")]),
        );
        let lab_text = lab.to_string_lossy().into_owned();
        let vars = move |name: &str| (name == "KUMI_LOGIC_APP").then(|| lab_text.clone());
        let found = detect_logic(&env(Os::Mac, root.path(), &vars));
        assert_eq!(found.len(), 1);
        assert_eq!((&found[0].app, found[0].version, found[0].archs.len()), (&lab, Some(Version(11, 2, 0)), 0));
        let nowhere = root.path().join("nowhere/Logic Pro.app").to_string_lossy().into_owned();
        let vars = move |name: &str| (name == "KUMI_LOGIC_APP").then(|| nowhere.clone());
        assert!(detect_logic(&env(Os::Mac, root.path(), &vars)).is_empty());
        assert!(process_is(Daw::Logic, "/Applications/Logic Pro.app/Contents/MacOS/Logic Pro") && !process_is(Daw::Logic, "Logic"));
    }

    #[test]
    fn a_binary_plist_reads_its_strings_and_refuses_what_it_isnt() {
        let plist = bplist(&[("Short", "x"), ("CFBundleShortVersionString", "7.82"), ("Name", "a long value, past fifteen characters")]);
        assert_eq!(bplist_string(&plist, "CFBundleShortVersionString").as_deref(), Some("7.82"));
        assert_eq!(bplist_string(&plist, "Name").as_deref(), Some("a long value, past fifteen characters"));
        assert_eq!(bplist_string(&plist, "Short").as_deref(), Some("x"));
        assert_eq!(bplist_string(&plist, "Missing"), None);
        // A UTF-16 value, as a plist with a non-ASCII string is written: "ab" becomes "ét" (two units, marker 0x62).
        let mut utf16 = bplist(&[("Key", "ab")]);
        let at = utf16.windows(3).position(|window| window == [0x52, b'a', b'b']).unwrap();
        utf16.splice(at..at + 3, [0x62, 0, 0xe9, 0, 0x74]);
        // (the value is the last object, so only the offset table moved: the trailer says where)
        let end = utf16.len();
        let table = u64::from_be_bytes(utf16[end - 8..].try_into().unwrap()) + 2;
        utf16[end - 8..].copy_from_slice(&table.to_be_bytes());
        assert_eq!(bplist_string(&utf16, "Key").as_deref(), Some("\u{e9}t"));
        assert_eq!(bplist_string(b"bplist00", "Key"), None);
        assert_eq!(bplist_string(&plist[..plist.len() - 1], "Short"), None);
        assert_eq!(bplist_string(b"<?xml version=\"1.0\"?>", "Key"), None);
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
