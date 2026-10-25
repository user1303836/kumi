//! Kumi's REAPER extension: loaded by REAPER from UserPlugins, it serves Kumi's bridge on loopback.
//!
//! REAPER calls in on its main thread only: the entry point, the registered timer, the hidden control surface and
//! atexit. Each is guarded so a Rust panic never unwinds into REAPER. The socket threads never touch REAPER: they
//! queue work that the timer and the surface's Run() drain (see `extension`).

pub mod api;
pub mod changes;
pub mod csurf;
pub mod extension;
pub mod focus;
// M5 listening (m5-reaper).
pub mod listening;
pub mod log;
pub mod midi;
pub mod projectconfig;
pub mod queue;
pub mod read;
pub mod reaper;
pub mod refs;
pub mod rows;
pub mod scripts;
pub mod server;
pub mod setup;
pub mod window;

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use api::{Api, PluginInfo};
use csurf::{Call, Surface};
use extension::{Driver, Extension, Snapshot};
use queue::Job;
use server::{Config, Hub, Server};

/// Runs `f`, turning a panic into `fallback` and a log line: for every call that comes from REAPER or a thread.
pub fn guard<T>(edge: &str, fallback: T, f: impl FnOnce() -> T) -> T {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let why = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned());
            log::line(&format!("panic in {edge}: {}", why.unwrap_or_default()));
            fallback
        }
    }
}

type Register = unsafe extern "C" fn(name: *const c_char, info: *mut c_void) -> c_int;

/// How often a bridge without a server tries again (no reference yet, or every port taken).
const SERVER_RETRY: Duration = Duration::from_secs(5);

struct Loaded {
    register: Register,
    extension: Extension,
    surface: RefCell<Option<Surface>>,
    hub: Hub,
    jobs: Sender<Job>,
    snapshot: Arc<Mutex<Snapshot>>,
    server: RefCell<Option<Server>>,
    last_try: Cell<Option<Instant>>,
    last_failure: RefCell<String>,
    stopped: Cell<bool>,
}

thread_local! {
    static LOADED: RefCell<Option<Rc<Loaded>>> = const { RefCell::new(None) };
}

// The Rc is cloned out, so REAPER may unload the extension from inside a call (the quit action) without a borrow
// still held.
fn loaded() -> Option<Rc<Loaded>> {
    LOADED.with(|l| l.borrow().clone())
}

impl Loaded {
    fn register(&self, name: &CStr, info: *mut c_void) -> bool {
        unsafe { (self.register)(name.as_ptr(), info) != 0 }
    }

    fn tick(&self, driver: Driver) {
        if self.stopped.get() {
            return;
        }
        self.extension.tick(driver);
        // REAPER may have unloaded the extension inside that tick (the quit action runs the atexit hooks): nothing
        // started now would ever be stopped.
        if self.stopped.get() {
            return;
        }
        self.ensure_server();
        if let Some(server) = self.server.borrow_mut().as_mut() {
            // Looked at under its lock, and copied only when the discovery file is to be written (the project changed):
            // a copy of it every tick was most of the tick's allocations, and the file isn't written under the lock.
            let due = {
                let snapshot = self.extension.snapshot_ref();
                server.needs_publish(&snapshot).then(|| snapshot.clone())
            };
            if let Some(snapshot) = due {
                server.publish(&snapshot);
            }
        }
    }

    fn ensure_server(&self) {
        if self.stopped.get() || self.server.borrow().is_some() || self.last_try.get().is_some_and(|t| t.elapsed() < SERVER_RETRY) {
            return;
        }
        self.last_try.set(Some(Instant::now()));
        let resource = self.extension.reaper().resource_path();
        let reference_path = server::reference_path(Path::new(&resource));
        let reference = match daw_bridge::files::Reference::read(&reference_path) {
            Ok(reference) => reference,
            Err(error) => {
                self.note(format!("not serving: {} ({error})", reference_path.display()));
                return;
            }
        };
        let config = Config::from_reference(&reference);
        log::set_dir(config.log_dir.clone());
        // (run-lua's scripts go to the bridge's own folder, beside its secret: Kumi keeps it owner-only)
        reaper::generic::set_scripts_dir(config.secret_file.parent().map(|root| root.join("scripts")));
        // M5 listening (m5-reaper): renders, captures and scratch notes go into Kumi's folder beside the discovery folder.
        if let Some(folder) = listening::folder_beside(&config.discovery_dir) {
            self.extension.set_listening_folder(folder);
        }
        if let Some(folder) = changes::kumi_files::kumi_folder_beside(&config.discovery_dir) {
            self.extension.set_kumi_folder(folder);
        }
        log::line(&format!("a dialog over the main window is noticed through: {}", window::describe()));
        let started =
            config.bind().and_then(|listener| Server::start(listener, config, self.hub.clone(), self.jobs.clone(), self.snapshot.clone()));
        match started {
            Ok(server) => {
                log::line(&format!("serving on 127.0.0.1:{} (REAPER {})", server.port(), self.extension.reaper().app_version()));
                *self.server.borrow_mut() = Some(server);
            }
            Err(error) => self.note(format!("couldn't listen: {error}")),
        }
    }

    // A retry's failure, logged when it differs from the last one.
    fn note(&self, message: String) {
        if *self.last_failure.borrow() != message {
            log::line(&message);
            *self.last_failure.borrow_mut() = message;
        }
    }

    // Idempotent: atexit, then the unload entry call.
    fn stop(&self, reason: &str) {
        if self.stopped.replace(true) {
            return;
        }
        // A plan's open undo block is closed while REAPER can still be asked to.
        self.extension.stop();
        self.register(c"-timer", on_timer as *mut c_void);
        self.register(c"-hookpostcommand2", on_post_command2 as *mut c_void);
        self.register(c"-hookpostcommand", on_post_command as *mut c_void);
        self.register(c"-projectconfig", projectconfig::REGISTRATION.as_ptr());
        if let Some(surface) = self.surface.borrow_mut().take() {
            self.register(c"-csurf_inst", surface.as_ptr());
            drop(surface);
        }
        // Queued requests are answered "closing" (the main thread, here, will never run them).
        self.extension.cancel_jobs();
        if let Some(server) = self.server.borrow_mut().take() {
            server.stop(reason);
        }
        log::line(&format!("stopped: {reason}"));
    }
}

extern "C" fn on_timer() {
    guard("timer", (), || {
        if let Some(loaded) = loaded() {
            loaded.tick(Driver::Timer);
        }
    })
}

fn on_surface(call: Call) -> i32 {
    let Some(loaded) = loaded() else { return 0 };
    if loaded.stopped.get() {
        return 0;
    }
    if call == Call::Run {
        loaded.tick(Driver::Surface);
        return 0;
    }
    loaded.extension.on_surface(call)
}

/// REAPER ran an action (any, from a menu, a key, a script): the undo history may have moved. Which of the two hooks
/// REAPER calls depends on how the action was started (`Main_OnCommand` fires the first; a key press fires both), so both
/// are registered; the history is looked at on the next tick whichever ran.
extern "C" fn on_post_command(_command: c_int, _flag: c_int) {
    guard("post-command", (), || {
        if let Some(loaded) = loaded() {
            loaded.extension.nudge();
        }
    })
}

extern "C" fn on_post_command2(
    _section: *mut c_void,
    _command: c_int,
    _val: c_int,
    _valhw: c_int,
    _relmode: c_int,
    _hwnd: *mut c_void,
    _proj: *mut c_void,
) {
    on_post_command(0, 0)
}

extern "C" fn on_exit() {
    guard("atexit", (), || {
        if let Some(loaded) = loaded() {
            loaded.stop("quit");
        }
    })
}

fn load(info: &PluginInfo) -> c_int {
    if info.caller_version != api::REAPER_PLUGIN_VERSION {
        return 0;
    }
    let (Some(register), Some(get_func)) = (info.register, info.get_func) else { return 0 };
    if loaded().is_some() {
        return 1;
    }
    let api = Api::load(|name| unsafe { get_func(name.as_ptr()) });
    let hub = Hub::default();
    let (jobs, jobs_rx) = queue::channel();
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let extension = Extension::new(api, jobs_rx, Box::new(hub.clone()), snapshot.clone());
    let Some(surface) = Surface::new(on_surface) else { return 0 };
    let loaded = Rc::new(Loaded {
        register,
        extension,
        surface: RefCell::new(None),
        hub,
        jobs,
        snapshot,
        server: RefCell::new(None),
        last_try: Cell::new(None),
        last_failure: RefCell::new(String::new()),
        stopped: Cell::new(false),
    });
    loaded.register(c"ext_name", c"Kumi".as_ptr() as *mut c_void);
    loaded.register(c"csurf_inst", surface.as_ptr());
    // REAPER's own word that a project was loaded or made in some tab (the pointer and the file can't always say).
    loaded.register(c"projectconfig", projectconfig::REGISTRATION.as_ptr());
    loaded.register(c"hookpostcommand", on_post_command as *mut c_void);
    loaded.register(c"hookpostcommand2", on_post_command2 as *mut c_void);
    *loaded.surface.borrow_mut() = Some(surface);
    loaded.register(c"timer", on_timer as *mut c_void);
    // REAPER 7.55+; older ones still call the entry point to unload.
    loaded.register(c"atexit", on_exit as *mut c_void);
    LOADED.with(|l| *l.borrow_mut() = Some(loaded.clone()));
    // The load hook counts each load for the project REAPER says it's loading.
    projectconfig::ask_with(
        || LOADED.with(|l| l.try_borrow().ok()?.as_ref()?.extension.reaper().project_in_load_save().map(|project| project.addr())),
        || LOADED.with(|l| l.try_borrow().ok()?.as_ref()?.extension.reaper().current_project().map(|project| project.ptr.addr())),
    );
    // The server starts from the first tick: once registered, REAPER keeps calling this module, so nothing after the
    // registrations may fail the entry (a panic there would have REAPER unload a module it still has callbacks into).
    1
}

fn unload() {
    if let Some(loaded) = LOADED.with(|l| l.borrow_mut().take()) {
        loaded.stop("shutdown");
    }
}

/// REAPER's entry point: `info` to load, null to unload.
///
/// # Safety
/// Called by REAPER on its main thread with a valid `reaper_plugin_info_t` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ReaperPluginEntry(_instance: *mut c_void, info: *mut PluginInfo) -> c_int {
    guard("entry", 0, || match unsafe { info.as_ref() } {
        Some(info) => load(info),
        None => {
            unload();
            0
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake;
    use daw_bridge::files::{Reference, REFERENCE_SCHEMA};
    use std::path::PathBuf;
    use std::ptr::null_mut;
    use std::sync::atomic::AtomicBool;

    // What REAPER was asked to register, in order, on this test's thread (the extension is thread-local too).
    thread_local! {
        static REGISTERED: RefCell<Vec<(String, usize)>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn register(name: *const c_char, info: *mut c_void) -> c_int {
        let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        REGISTERED.with(|r| r.borrow_mut().push((name, info as usize)));
        1
    }

    fn registered() -> Vec<String> {
        REGISTERED.with(|r| r.borrow().iter().map(|(name, _)| name.clone()).collect())
    }

    fn hook(name: &str) -> extern "C" fn() {
        let pointer = REGISTERED.with(|r| r.borrow().iter().rev().find(|(n, _)| n == name).map(|(_, p)| *p)).expect("registered");
        unsafe { std::mem::transmute::<usize, extern "C" fn()>(pointer) }
    }

    fn info(caller_version: c_int) -> PluginInfo {
        PluginInfo { caller_version, hwnd_main: null_mut(), register: Some(register), get_func: Some(fake::get_func) }
    }

    /// A fake REAPER whose resource folder holds a reference file, so the bridge can serve: returns the discovery folder.
    fn reaper_with_a_reference() -> (tempfile::TempDir, PathBuf) {
        fake::reset();
        REGISTERED.with(|r| r.borrow_mut().clear());
        let dir = tempfile::tempdir().unwrap();
        let resource = dir.path().join("REAPER");
        std::fs::create_dir_all(resource.join("UserPlugins")).unwrap();
        let discovery = dir.path().join("kumi/discovery");
        let reference = Reference {
            schema: REFERENCE_SCHEMA.into(),
            daw: "reaper".into(),
            secret_file: dir.path().join("kumi/bridge.secret").to_string_lossy().into_owned(),
            discovery_dir: discovery.to_string_lossy().into_owned(),
            port: None,
            log_dir: Some(dir.path().join("kumi/logs").to_string_lossy().into_owned()),
        };
        std::fs::write(resource.join("UserPlugins/kumi-bridge-reference.json"), reference.to_json()).unwrap();
        fake::with(|s| s.resource = resource.to_string_lossy().into_owned());
        (dir, discovery)
    }

    fn discovery_files(dir: &Path) -> usize {
        daw_bridge::files::scan_discovery(dir, daw_bridge::Daw::Reaper).len()
    }

    #[test]
    fn the_entry_registers_once_serves_from_the_first_tick_and_unregisters_on_unload() {
        let (_dir, discovery) = reaper_with_a_reference();
        // A REAPER from another SDK version is refused before anything is registered.
        assert_eq!(unsafe { ReaperPluginEntry(null_mut(), &mut info(0x1)) }, 0);
        assert!(registered().is_empty());

        assert_eq!(unsafe { ReaperPluginEntry(null_mut(), &mut info(api::REAPER_PLUGIN_VERSION)) }, 1);
        assert_eq!(registered(), ["ext_name", "csurf_inst", "projectconfig", "hookpostcommand", "hookpostcommand2", "timer", "atexit"]);
        assert_eq!(discovery_files(&discovery), 0, "no server before the first tick");
        // Loading again registers nothing more.
        assert_eq!(unsafe { ReaperPluginEntry(null_mut(), &mut info(api::REAPER_PLUGIN_VERSION)) }, 1);
        assert_eq!(registered().len(), 7);

        hook("timer")();
        assert_eq!(discovery_files(&discovery), 1);
        assert!(loaded().unwrap().server.borrow().is_some());

        // Unloading says goodbye to REAPER and to Kumi; the quit hook coming after it does nothing more.
        assert_eq!(unsafe { ReaperPluginEntry(null_mut(), null_mut()) }, 0);
        assert_eq!(registered()[7..], ["-timer", "-hookpostcommand2", "-hookpostcommand", "-projectconfig", "-csurf_inst"]);
        assert_eq!(discovery_files(&discovery), 0);
        hook("atexit")();
        on_timer();
        assert_eq!(registered().len(), 12);
    }

    thread_local! {
        static QUIT_RUNS_ATEXIT: Cell<bool> = const { Cell::new(false) };
    }

    fn reaper_runs_atexit_inside_the_quit_action(command: i32) {
        if command == reaper::ACTION_QUIT && QUIT_RUNS_ATEXIT.get() {
            hook("atexit")();
        }
    }

    #[test]
    fn a_quit_that_unloads_inside_the_tick_leaves_no_server_behind() {
        let (_dir, discovery) = reaper_with_a_reference();
        assert_eq!(unsafe { ReaperPluginEntry(null_mut(), &mut info(api::REAPER_PLUGIN_VERSION)) }, 1);
        hook("timer")();
        assert_eq!(discovery_files(&discovery), 1);

        // REAPER has run a while: the last start of a server is old enough that a tick would try again.
        let loaded_now = loaded().unwrap();
        loaded_now.last_try.set(None);
        QUIT_RUNS_ATEXIT.set(true);
        fake::with(|s| s.on_action = Some(reaper_runs_atexit_inside_the_quit_action));
        let (job, waiter) = queue::job("quit", serde_json::json!({}));
        loaded_now.jobs.send(job).unwrap();
        hook("timer")();
        let wait = AtomicBool::new(false);
        assert!(matches!(waiter.wait(Duration::from_secs(1), &wait), queue::Outcome::Answered(Ok(_))));
        // The next tick runs the quit action, and REAPER's atexit hooks run inside it.
        hook("timer")();
        assert!(loaded_now.stopped.get());
        assert!(loaded_now.server.borrow().is_none(), "a server was started after the extension stopped");
        assert_eq!(discovery_files(&discovery), 0);
        assert_eq!(registered().iter().filter(|name| *name == "-timer").count(), 1);
        // The unload call that follows finds nothing left to do.
        assert_eq!(unsafe { ReaperPluginEntry(null_mut(), null_mut()) }, 0);
        assert_eq!(registered().iter().filter(|name| *name == "-timer").count(), 1);
    }
}
