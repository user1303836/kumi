//! Logic Pro as the bridge sees it through the helper: which Logic to attach to, what its windows say about the open
//! project (its saved path, or its title while unsaved) and whether it can take work, and the identity the wire
//! carries. Pure: the helper's replies come in as JSON, the state comes out; nothing here talks to a process.

use std::path::Path;
use std::time::Duration;

use daw_bridge::auth::sha256_hex;
use daw_bridge::wire::{Identity, Project};
use daw_bridge::Daw;
use percent_encoding::percent_decode_str;
use serde_json::{json, Map, Value};

use crate::BRIDGE_VERSION;

/// Logic Pro's bundle identifiers: the App Store build and the one some installs carry. The bundle id is the
/// identity's `resource`: what tells one Logic from another, as REAPER's resource folder does.
pub const BUNDLES: [&str; 2] = ["com.apple.logic10", "com.apple.mobilelogic"];
/// How often Logic's windows are read.
pub const POLL: Duration = Duration::from_millis(500);
/// The main window's title ends with the view it shows; the project's own name comes before it.
const TRACKS_SUFFIX: &str = " - Tracks";
/// The longest project name, identity and path the discovery file and the welcome carry (in characters): hosts skip a
/// discovery file over 64 KiB, and a title can be as long as its author likes.
pub const MAX_NAME: usize = 256;
pub const MAX_PATH: usize = 1024;

/// Why Logic can't take work now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// The main or focused window is a dialog, or a sheet is open.
    Modal,
    /// Logic reports no window at all: its Space isn't the current one.
    Away,
    /// The main window has no title yet: a project is loading.
    Loading,
}

impl Busy {
    pub fn code(self) -> &'static str {
        match self {
            Busy::Modal => "modal",
            Busy::Away => "away",
            Busy::Loading => "loading",
        }
    }

    pub fn from_code(code: &str) -> Option<Busy> {
        match code {
            "modal" => Some(Busy::Modal),
            "away" => Some(Busy::Away),
            "loading" => Some(Busy::Loading),
            _ => None,
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            Busy::Modal => "Logic Pro is showing a dialog",
            Busy::Away => "Logic Pro's window isn't on this Space (or is minimized)",
            Busy::Loading => "Logic Pro is loading a project",
        }
    }
}

/// A permission the helper lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// `AXIsProcessTrusted` is false: nothing of Logic can be read or changed.
    Accessibility,
}

impl Permission {
    pub fn code(self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
        }
    }
}

/// A running app from the helper's `apps`: `{bundle, pid, name, active, hidden}`, plus `version`, `path` (the app
/// bundle) and `launched` (unix ms) when the helper gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub pid: u32,
    pub bundle: String,
    pub name: String,
    pub version: Option<String>,
    pub path: Option<String>,
    pub launched: Option<u64>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_string)
}

pub fn parse_apps(reply: &Value) -> Vec<App> {
    let Some(apps) = reply.get("apps").and_then(Value::as_array) else { return vec![] };
    apps.iter()
        .filter_map(|app| {
            let pid = u32::try_from(app.get("pid")?.as_u64()?).ok()?;
            Some(App {
                pid,
                bundle: text(app, "bundle").unwrap_or_default(),
                name: text(app, "name").unwrap_or_default(),
                version: text(app, "version"),
                path: text(app, "path"),
                launched: app.get("launched").and_then(Value::as_u64),
            })
        })
        .collect()
}

/// The Logic to attach to when several run: the one launched last, or without launch times the highest pid.
pub fn newest(apps: &[App]) -> Option<&App> {
    apps.iter().max_by_key(|app| (app.launched.unwrap_or(0), app.pid))
}

/// A window from the helper's `windows`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Window {
    pub title: String,
    pub role: String,
    pub subrole: String,
    pub main: bool,
    pub focused: bool,
    /// `AXDocument`: the saved project's URL or path.
    pub document: Option<String>,
    pub minimized: bool,
    /// A sheet or a modal panel.
    pub modal: bool,
}

impl Window {
    pub fn dialog(&self) -> bool {
        self.role == "AXDialog" || self.subrole == "AXDialog" || self.subrole == "AXSystemDialog"
    }

    pub fn sheet(&self) -> bool {
        self.role == "AXSheet" || self.modal
    }

    /// A window that can be the project's: not a dialog or a floating window (a sheet over it is reported on the
    /// window itself, which stays the project's).
    pub fn standard(&self) -> bool {
        self.role == "AXWindow" && !self.dialog() && self.subrole != "AXFloatingWindow"
    }
}

/// The windows and the Window menu's titles.
pub fn parse_windows(reply: &Value) -> (Vec<Window>, Vec<String>) {
    let windows = reply
        .get("windows")
        .and_then(Value::as_array)
        .map(|windows| {
            windows
                .iter()
                .map(|window| Window {
                    title: text(window, "title").unwrap_or_default(),
                    role: text(window, "role").unwrap_or_default(),
                    subrole: text(window, "subrole").unwrap_or_default(),
                    main: window["main"] == Value::Bool(true),
                    focused: window["focused"] == Value::Bool(true),
                    document: text(window, "document"),
                    minimized: window["minimized"] == Value::Bool(true),
                    modal: window["modal"] == Value::Bool(true),
                })
                .collect()
        })
        .unwrap_or_default();
    let listed = reply
        .get("listed")
        .and_then(Value::as_array)
        .map(|titles| titles.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    (windows, listed)
}

/// Text cut to `max` characters, the last of them an ellipsis when something was cut.
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// A saved project's id: no UUID exists in every project, so it is made from the package's path, as the REAPER
/// extension makes one for a project without an id.
pub fn project_id(path: &str) -> String {
    sha256_hex(format!("logic-project:{path}").as_bytes())[..32].to_string()
}

/// The project's path from a window's `AXDocument`: a file URL or a path, decoded, without a trailing slash, and cut
/// to the package when the URL names something inside it.
pub fn document_path(document: &str) -> String {
    let mut path = match document.strip_prefix("file://") {
        Some(rest) => {
            // `file://localhost/…`: a host before the path.
            let rest = if rest.starts_with('/') { rest } else { rest.find('/').map(|at| &rest[at..]).unwrap_or("") };
            percent_decode_str(rest).decode_utf8_lossy().into_owned()
        }
        None => document.to_string(),
    };
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    for package in [".logicx/", ".logic/"] {
        if let Some(at) = path.find(package) {
            path.truncate(at + package.len() - 1);
        }
    }
    path
}

/// The project's name from its path: the package's name without its extension.
pub fn project_name(path: &str) -> String {
    Path::new(path).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// An unsaved project's identity from its window title: the title without ` - Tracks` (or whichever view the window
/// shows after the last ` - `).
pub fn identity_of(title: &str) -> String {
    let title = title.trim();
    if let Some(name) = title.strip_suffix(TRACKS_SUFFIX) {
        return name.to_string();
    }
    title.rsplit_once(" - ").map(|(name, _)| name).unwrap_or(title).to_string()
}

/// The project a window shows: by its document when saved, by its title when not; None while the title is empty
/// (loading).
pub fn project_of(window: &Window) -> Option<Project> {
    if let Some(document) = window.document.as_deref() {
        let path = document_path(document);
        return Some(Project {
            id: Some(project_id(&path)),
            name: Some(cut(&project_name(&path), MAX_NAME)),
            path: Some(cut(&path, MAX_PATH)),
            extra: Map::new(),
        });
    }
    if window.title.trim().is_empty() {
        return None;
    }
    let identity = cut(&identity_of(&window.title), MAX_NAME);
    let mut extra = Map::new();
    extra.insert("identity".into(), identity.clone().into());
    Some(Project { id: None, name: Some(identity), path: None, extra })
}

/// Whether a standard window is a project's: it names a document, or its title is a project's ("… - Tracks").
fn is_project(window: &Window) -> bool {
    window.standard() && (window.document.is_some() || window.title.ends_with(TRACKS_SUFFIX))
}

/// The window the project is read from: a project's window, the main one first, then the focused one; failing that
/// the main standard window, then the focused one (Logic's own windows, the Control Surface Setup among them, are
/// standard windows too, so a project's window wins over them); never a dialog, a sheet or a floating window.
pub fn project_window(windows: &[Window]) -> Option<&Window> {
    windows
        .iter()
        .find(|window| window.main && is_project(window))
        .or_else(|| windows.iter().find(|window| window.focused && is_project(window)))
        .or_else(|| windows.iter().find(|window| is_project(window)))
        .or_else(|| windows.iter().find(|window| window.main && window.standard()))
        .or_else(|| windows.iter().find(|window| window.focused && window.standard()))
}

fn busy_of(windows: &[Window], project_window: Option<&Window>) -> Option<Busy> {
    if windows.is_empty() {
        return Some(Busy::Away);
    }
    let main = windows.iter().find(|window| window.main);
    let focused = windows.iter().find(|window| window.focused);
    if main.is_some_and(Window::dialog) || focused.is_some_and(Window::dialog) || windows.iter().any(Window::sheet) {
        return Some(Busy::Modal);
    }
    if project_window.is_some_and(|window| window.title.trim().is_empty() && window.document.is_none()) {
        return Some(Busy::Loading);
    }
    // A minimized project window is in the Dock, not on the Space: nothing of it can be read (`AXMainWindow` is nil).
    if project_window.is_some_and(|window| window.minimized && !window.main) {
        return Some(Busy::Away);
    }
    None
}

/// What tells one project from another: its id when saved, its identity when not.
pub fn project_key(project: Option<&Project>) -> (Option<String>, Option<String>) {
    let Some(project) = project else { return (None, None) };
    (project.id.clone(), project.extra.get("identity").and_then(Value::as_str).map(str::to_string))
}

/// The Logic the bridge is attached to, as of the last look: what `status`, the welcome and the discovery file say.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// Logic's pid.
    pub pid: u32,
    /// Logic's bundle identifier.
    pub resource: String,
    /// Logic's `CFBundleShortVersionString`; empty when unknown.
    pub daw_version: String,
    pub project: Option<Project>,
    pub busy: Option<Busy>,
    pub permission: Option<Permission>,
    /// The project window's title as last seen, and whether it was the main window: the readers address it by title
    /// when another of Logic's windows (the Control Surface Setup, say) is main.
    pub window_title: Option<String>,
    pub window_is_main: bool,
}

impl Model {
    pub fn new(pid: u32, resource: &str, daw_version: &str) -> Model {
        Model {
            pid,
            resource: resource.into(),
            daw_version: daw_version.into(),
            project: None,
            busy: None,
            permission: None,
            window_title: None,
            window_is_main: true,
        }
    }

    /// Where the readers address the project window: the main window, or the project's window by its title when
    /// another window is main.
    pub fn window_root(&self) -> crate::ax::Root {
        match &self.window_title {
            Some(title) if !self.window_is_main && !title.is_empty() => crate::ax::Root::Title(title.clone()),
            _ => crate::ax::Root::Main,
        }
    }

    pub fn attach(app: &App, daw_version: &str) -> Model {
        Model::new(app.pid, &app.bundle, daw_version)
    }

    /// What `trusted` answered: whether the permission changed.
    pub fn on_trusted(&mut self, trusted: bool) -> bool {
        let before = self.permission;
        self.permission = (!trusted).then_some(Permission::Accessibility);
        if !trusted {
            // Nothing can be read: not busy, and the project stays what was last seen.
            self.busy = None;
        }
        self.permission != before
    }

    /// A `windows` reply: whether the project (its id or identity) changed.
    pub fn on_windows(&mut self, reply: &Value) -> bool {
        let (windows, listed) = parse_windows(reply);
        self.look(&windows, &listed)
    }

    /// Logic's windows as reported: the project and whether Logic is busy, from them. Whether the project changed.
    pub fn look(&mut self, windows: &[Window], _listed: &[String]) -> bool {
        let before = project_key(self.project.as_ref());
        let window = project_window(windows);
        self.busy = busy_of(windows, window);
        if let Some(window) = window {
            self.window_title = Some(window.title.clone());
            self.window_is_main = window.main;
        }
        if windows.is_empty() {
            // Away: nothing can be read, and the project stays what it was. (The Window menu still lists the project
            // window's title, but a title can't give a saved project's path, so it isn't followed from there.)
        } else if let Some(window) = window {
            // Loading (no title, no document) keeps the project too: the title comes back with the project.
            if let Some(project) = project_of(window) {
                self.project = Some(project);
            }
        } else if !windows.iter().any(Window::standard) {
            // Only dialogs, sheets or floating windows: no project is open (the chooser after a close).
            self.project = None;
        }
        project_key(self.project.as_ref()) != before
    }

    pub fn identity(&self) -> Identity {
        Identity {
            daw: Daw::Logic.id().into(),
            bridge: BRIDGE_VERSION.into(),
            daw_version: self.daw_version.clone(),
            pid: self.pid,
            resource: self.resource.clone(),
            project: self.project.clone(),
        }
    }

    /// What `status` answers, flat as the REAPER extension's and as the host reads it: the project (or null), why
    /// Logic can't take work (`false` when it can), and the permission the helper lacks (or null). The identity itself
    /// is the welcome's.
    pub fn status(&self) -> Value {
        json!({
            "project": self.project,
            "busy": self.busy.map_or(Value::Bool(false), |busy| busy.code().into()),
            "permission": self.permission.map(Permission::code),
        })
    }
}

/// The app bundle of an executable inside it: `…/Logic Pro.app/Contents/MacOS/Logic Pro` → `…/Logic Pro.app`.
pub fn bundle_of_executable(executable: &str) -> Option<String> {
    let at = executable.rfind(".app/Contents/MacOS/")?;
    Some(executable[..at + 4].to_string())
}

/// Logic's version: what the helper's `apps` says, else (macOS) read from the app bundle's Info.plist, else unknown.
pub fn app_version(app: &App) -> String {
    if let Some(version) = app.version.as_deref().filter(|version| !version.is_empty()) {
        return version.to_string();
    }
    #[cfg(target_os = "macos")]
    {
        let bundle = app
            .path
            .as_deref()
            .and_then(|path| if path.ends_with(".app") { Some(path.to_string()) } else { bundle_of_executable(path) })
            .or_else(|| macos::executable_of(app.pid).and_then(|executable| bundle_of_executable(&executable)));
        if let Some(version) = bundle.and_then(|bundle| macos::bundle_version(Path::new(&bundle))) {
            return version;
        }
    }
    String::new()
}

#[cfg(target_os = "macos")]
pub mod macos {
    use std::path::Path;
    use std::process::Command;

    /// A process's executable path, from `ps`.
    pub fn executable_of(pid: u32) -> Option<String> {
        let output = Command::new(crate::tools::PS).args(["-o", "comm=", "-p", &pid.to_string()]).output().ok()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }

    /// An app bundle's `CFBundleShortVersionString`, read with `defaults` (its Info.plist is binary).
    pub fn bundle_version(bundle: &Path) -> Option<String> {
        let info = bundle.join("Contents").join("Info");
        let output = Command::new(crate::tools::DEFAULTS).args(["read"]).arg(&info).arg("CFBundleShortVersionString").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!version.is_empty()).then_some(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(title: &str, document: Option<&str>) -> Window {
        Window {
            title: title.into(),
            role: "AXWindow".into(),
            subrole: "AXStandardWindow".into(),
            main: true,
            focused: true,
            document: document.map(str::to_string),
            ..Window::default()
        }
    }

    #[test]
    fn a_saved_project_is_its_path_and_an_unsaved_one_its_title() {
        let saved = project_of(&window("Song - Tracks", Some("file:///Users/kumi/Music/Logic/My%20Song.logicx/"))).unwrap();
        assert_eq!(saved.path.as_deref(), Some("/Users/kumi/Music/Logic/My Song.logicx"));
        assert_eq!(saved.name.as_deref(), Some("My Song"));
        assert_eq!(saved.id.as_deref(), Some(project_id("/Users/kumi/Music/Logic/My Song.logicx").as_str()));
        assert_eq!(saved.id.as_ref().unwrap().len(), 32);
        assert!(saved.extra.is_empty());
        // A URL into the package names the package; a plain path and a localhost URL work too.
        assert_eq!(document_path("file:///a/b/Song.logicx/Alternatives/000/ProjectData"), "/a/b/Song.logicx");
        assert_eq!(document_path("/a/b/Song.logicx"), "/a/b/Song.logicx");
        assert_eq!(document_path("file://localhost/a/b/Song.logicx"), "/a/b/Song.logicx");
        assert_eq!(document_path("file:///a/Old/Old.logic/"), "/a/Old/Old.logic");

        let unsaved = project_of(&window("Untitled - Tracks", None)).unwrap();
        assert_eq!(unsaved.id, None);
        assert_eq!(unsaved.path, None);
        assert_eq!(unsaved.name.as_deref(), Some("Untitled"));
        assert_eq!(unsaved.extra.get("identity"), Some(&json!("Untitled")));
        assert_eq!(identity_of("My - Song - Mixer"), "My - Song");
        assert_eq!(identity_of("Song"), "Song");
        assert_eq!(project_of(&window("", None)), None);
        // Long titles and paths are cut as the discovery file and the welcome carry them.
        let long = project_of(&window(&"é".repeat(1000), None)).unwrap();
        assert_eq!(long.extra["identity"].as_str().unwrap().chars().count(), MAX_NAME);
        let long = project_of(&window("x", Some(&format!("/m/{}.logicx", "p".repeat(5000))))).unwrap();
        assert_eq!(long.path.unwrap().chars().count(), MAX_PATH);
        assert_eq!(long.id.unwrap().len(), 32);
    }

    #[test]
    fn the_newest_logic_is_attached_to_and_the_apps_are_read_from_the_reply() {
        let apps = parse_apps(&json!({"apps": [
            {"bundle": "com.apple.logic10", "pid": 100, "name": "Logic Pro", "active": true, "hidden": false, "version": "12.3.1"},
            {"bundle": "com.apple.logic10", "pid": 200, "name": "Logic Pro", "active": false, "hidden": false},
            {"bundle": "com.apple.logic10", "name": "no pid"},
        ]}));
        assert_eq!(apps.len(), 2);
        assert_eq!(newest(&apps).unwrap().pid, 200);
        let with_times = parse_apps(&json!({"apps": [
            {"bundle": "com.apple.logic10", "pid": 100, "launched": 2000},
            {"bundle": "com.apple.logic10", "pid": 200, "launched": 1000},
        ]}));
        assert_eq!(newest(&with_times).unwrap().pid, 100);
        assert_eq!(newest(&[]), None);
        assert_eq!(app_version(&apps[0]), "12.3.1");
        assert_eq!(
            bundle_of_executable("/Applications/Logic Pro.app/Contents/MacOS/Logic Pro").as_deref(),
            Some("/Applications/Logic Pro.app")
        );
        assert_eq!(bundle_of_executable("/usr/bin/true"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundles_version_is_read_from_its_info_plist() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Logic Pro.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>CFBundleShortVersionString</key><string>12.3.1</string></dict></plist>\n",
        )
        .unwrap();
        assert_eq!(macos::bundle_version(&bundle).as_deref(), Some("12.3.1"));
        assert_eq!(macos::bundle_version(&dir.path().join("Nothing.app")), None);
        let me = macos::executable_of(std::process::id()).unwrap();
        assert!(me.contains("logic_kumi") || me.contains("logic-kumi"), "{me}");
        let app = App {
            pid: 1,
            bundle: "com.apple.logic10".into(),
            name: "Logic Pro".into(),
            version: None,
            path: Some(bundle.to_string_lossy().into_owned()),
            launched: None,
        };
        assert_eq!(app_version(&app), "12.3.1");
    }

    #[test]
    fn the_windows_say_whether_logic_is_busy_and_which_project_is_open() {
        let mut model = Model::new(42, "com.apple.logic10", "12.3.1");
        let song = window("Song - Tracks", Some("file:///m/Song.logicx"));
        assert!(model.look(std::slice::from_ref(&song), &[]));
        assert_eq!(model.busy, None);
        assert_eq!(model.project.as_ref().unwrap().id.as_deref(), Some(project_id("/m/Song.logicx").as_str()));
        // The same project again: no change.
        assert!(!model.look(std::slice::from_ref(&song), &[]));

        // Away: no window at all; the project stays.
        assert!(!model.look(&[], &["Song - Tracks".into()]));
        assert_eq!(model.busy, Some(Busy::Away));
        assert!(model.project.is_some());
        // Minimized into the Dock: away too, and still the project.
        let minimized = Window { minimized: true, main: false, focused: false, ..song.clone() };
        assert!(!model.look(&[minimized], &["Song - Tracks".into()]));
        assert_eq!(model.busy, Some(Busy::Away));

        // A dialog as the main window, the project window behind it: modal, project kept.
        let dialog = Window {
            title: "Import".into(),
            role: "AXWindow".into(),
            subrole: "AXDialog".into(),
            main: true,
            focused: true,
            ..Window::default()
        };
        let behind = Window { main: false, focused: false, ..song.clone() };
        assert!(!model.look(&[dialog.clone(), behind.clone()], &[]));
        assert_eq!(model.busy, Some(Busy::Modal));
        // A sheet reported on the project window, and a dialog as the focused window, are modal too.
        let sheet = Window { modal: true, ..song.clone() };
        assert!(!model.look(&[sheet], &[]));
        assert_eq!(model.busy, Some(Busy::Modal));
        let focused_dialog = Window { main: false, ..dialog.clone() };
        let main_song = Window { focused: false, ..song.clone() };
        assert!(!model.look(&[main_song, focused_dialog], &[]));
        assert_eq!(model.busy, Some(Busy::Modal));

        // Loading: the main window has no title yet; the project stays until it has one.
        assert!(!model.look(&[window("", None)], &[]));
        assert_eq!(model.busy, Some(Busy::Loading));
        assert!(model.project.is_some());
        // Then another project appears: a change.
        assert!(model.look(&[window("Other - Tracks", Some("file:///m/Other.logicx"))], &[]));
        assert_eq!(model.project.as_ref().unwrap().name.as_deref(), Some("Other"));
        assert_eq!(model.busy, None);

        // Saved as: the path changes, so the id does.
        assert!(model.look(&[window("Other - Tracks", Some("file:///m/Other%202.logicx"))], &[]));
        // Only a dialog left (the project chooser after a close): no project.
        assert!(model.look(&[dialog], &[]));
        assert_eq!((model.project.as_ref(), model.busy), (None, Some(Busy::Modal)));
        // Logic's own Control Surface Setup as the main window, the project's window behind it: still the project.
        let setup = Window {
            title: "Control Surface Setup".into(),
            role: "AXWindow".into(),
            subrole: "AXStandardWindow".into(),
            main: true,
            focused: true,
            ..Window::default()
        };
        let behind = Window { main: false, focused: false, ..window("Other - Tracks", Some("file:///m/Other%202.logicx")) };
        assert!(model.look(&[setup.clone(), behind], &[]), "a project again, after the chooser");
        assert_eq!((model.project.as_ref().map(|p| p.name.clone().unwrap()), model.busy), (Some("Other 2".into()), None));
        assert_eq!(model.window_root(), crate::ax::Root::Title("Other - Tracks".into()));
        assert!(model.look(&[window("Other - Tracks", Some("file:///m/Other%202.logicx"))], &[]) == false);
        assert_eq!(model.window_root(), crate::ax::Root::Main);
        // A floating window alone isn't a project window either.
        let floating = Window {
            title: "Channel EQ".into(),
            role: "AXWindow".into(),
            subrole: "AXFloatingWindow".into(),
            main: true,
            ..Window::default()
        };
        assert!(model.look(&[floating], &[]), "the project went away");
        assert_eq!((model.project.as_ref(), model.busy), (None, None));
        // An unsaved project: its identity.
        assert!(model.look(&[window("Untitled - Tracks", None)], &[]));
        assert_eq!(project_key(model.project.as_ref()), (None, Some("Untitled".into())));
    }

    #[test]
    fn status_is_the_project_with_busy_and_the_permission_and_the_identity_is_the_welcomes() {
        let mut model = Model::new(42, "com.apple.logic10", "12.3.1");
        model.on_windows(&json!({"windows": [{"title": "Song - Tracks", "role": "AXWindow", "subrole": "AXStandardWindow", "main": true, "focused": true, "document": "file:///m/Song.logicx"}], "listed": ["Song - Tracks"]}));
        let status = model.status();
        assert_eq!(status["project"]["path"], "/m/Song.logicx");
        assert_eq!(status["busy"], false);
        assert_eq!(status["permission"], Value::Null);
        assert_eq!(status.as_object().unwrap().keys().collect::<Vec<_>>(), ["project", "busy", "permission"]);
        let identity = serde_json::to_value(model.identity()).unwrap();
        assert_eq!(identity["daw"], "logic");
        assert_eq!(identity["pid"], 42);
        assert_eq!(identity["resource"], "com.apple.logic10");
        assert_eq!(identity["dawVersion"], "12.3.1");
        assert_eq!(identity["bridge"], BRIDGE_VERSION);
        assert_eq!(identity["project"], status["project"]);
        // Away: busy is the word for it.
        model.on_windows(&json!({"windows": []}));
        assert_eq!(model.status()["busy"], "away");
        // Not trusted: the permission is named, and nothing counts as busy.
        assert!(model.on_trusted(false));
        assert_eq!((&model.status()["permission"], &model.status()["busy"]), (&json!("accessibility"), &json!(false)));
        assert!(!model.on_trusted(false));
        assert!(model.on_trusted(true));
        assert_eq!(model.status()["permission"], Value::Null);
    }
}
