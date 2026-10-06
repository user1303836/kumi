//! Willington, the native bindings Kumi's bridge carries into Live (`AbletonMcpBridge/willington`): off
//! until the producer turns them on with /willington. The switch is `willington.json` beside the bridge's
//! Remote Script, which the bridge reads again within a second of it changing, Live running.

use crate::config::remote_scripts_dir;
use ableton_mcp_server::delivery::{write_owner_file, REMOTE_SCRIPT_PACKAGE, WILLINGTON_CONFIG, WILLINGTON_FOLDER};
use futures::future::{FutureExt, LocalBoxFuture};
use kumi_common::time::now_ms_f64;
use kumi_runtime::{integrations::ableton::willington::WillingtonSwitch, system::Env};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, SystemTime},
};

/// What Kumi says when it starts while the bindings are off.
pub const OFF_AT_START: &str = "Willington bindings are OFF currently, type /willington to toggle them on";
pub const TURNED_ON: &str = "Willington bindings are ON: Kumi can map rack macros with their ranges, name macros and variations, and set rack chain zones, on the Live versions Willington supports. /willington again turns them off.";
pub const TURNED_ON_WITH_FOLLOW: &str = "Willington bindings are ON: Kumi can map rack macros with their ranges, name macros and variations, set rack chain zones and set Session clips' Follow Actions, on the Live versions Willington supports. /willington again turns them off.";
pub const TURNED_OFF: &str = "Willington bindings are OFF. /willington turns them on again.";
/// Live can't unload Follow Actions' bindings once it has them.
pub const TURNED_OFF_FOLLOW_STAYS: &str = "Willington bindings are OFF. Follow Actions' bindings, once Live has loaded them, stay until it restarts, with their edits off. /willington turns them on again.";

/// How long after the bindings are turned on the bridge may still be loading them: it looks at the switch
/// once a second, then checks each binding against Live's own executable.
const LOADING: Duration = Duration::from_secs(10);

/// Willington in the bridge in Live, when the bridge carries its files (or they're installed beside it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Willington {
    bridge: PathBuf,
}
impl Willington {
    /// In the bridge Kumi finds in Live's Remote Scripts folder.
    pub fn find(env: &Env) -> Option<Self> {
        Self::in_remote_scripts(Path::new(&remote_scripts_dir(env)))
    }
    pub fn in_remote_scripts(scripts: &Path) -> Option<Self> {
        let bridge = scripts.join(REMOTE_SCRIPT_PACKAGE);
        let runtime = |folder: &Path| folder.join("WillingtonRuntime").join("__init__.py").is_file();
        (bridge.join("__init__.py").is_file() && (runtime(&bridge.join(WILLINGTON_FOLDER)) || runtime(scripts))).then_some(Self { bridge })
    }
    fn scripts(&self) -> &Path {
        self.bridge.parent().unwrap_or(Path::new("."))
    }
    fn switch_file(&self) -> PathBuf {
        self.bridge.join(WILLINGTON_CONFIG)
    }
    /// On while the switch asks for edits from at least one binding; anything else, a missing switch too, is off.
    pub fn on(&self) -> bool {
        read_json(&self.switch_file()).is_some_and(|value| {
            value["enableWrites"] == true && ["followActions", "deviceTools", "rackZones"].iter().any(|key| value[*key] == true)
        })
    }
    /// The switch as the model is told of it: turned on moments ago, the bindings may not be loaded yet.
    pub fn switch(&self) -> WillingtonSwitch {
        if !self.on() {
            return WillingtonSwitch::Off;
        }
        // A time ahead of the clock counts as just now.
        let modified = std::fs::metadata(self.switch_file()).and_then(|entry| entry.modified());
        if modified.is_ok_and(|at| at.elapsed().map_or(true, |age| age < LOADING)) {
            WillingtonSwitch::JustOn
        } else {
            WillingtonSwitch::On
        }
    }
    /// The switch file as it stands (its time and size), None when there's none: what tells that it changed.
    fn stamp(&self) -> Option<(SystemTime, u64)> {
        let entry = std::fs::metadata(self.switch_file()).ok()?;
        Some((entry.modified().ok()?, entry.len()))
    }
    /// Whether a passing Follow Action self-test sits beside the WillingtonBindings the bridge loads (one
    /// installed beside the bridge comes first on Python's path, then the bridge's own copy), for a library in
    /// that folder. The bridge turns Follow Action edits on only when the receipt names the very library it
    /// selects for the Live that's open, which Kumi can't know; a receipt for none of them, kept from a library
    /// since replaced, would load Follow Actions for nothing.
    fn follow_receipt(&self) -> bool {
        let beside = self.scripts().join("WillingtonBindings");
        let bindings =
            if beside.join("__init__.py").is_file() { beside } else { self.bridge.join(WILLINGTON_FOLDER).join("WillingtonBindings") };
        let Some(receipt) = read_json(&bindings.join("self-test.json")) else { return false };
        // The bridge compares the digest exactly, as Python's lowercase hex.
        let digest = receipt["library_sha256"]
            .as_str()
            .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        receipt["status"] == "passed"
            && digest.is_some_and(|digest| {
                libraries(&bindings, 3)
                    .iter()
                    .any(|library| std::fs::read(library).is_ok_and(|bytes| hex::encode(Sha256::digest(bytes)) == digest))
            })
    }
    /// On writes the switch, owner-only (the bridge reads no other kind); off removes it. Follow Actions come
    /// on only with a self-test receipt that can turn their edits on, since Live can't unload their bindings.
    /// Done, it says what to tell the producer.
    pub fn set(&self, on: bool) -> Result<&'static str, String> {
        let switch = self.switch_file();
        if on {
            let follow = self.follow_receipt();
            let bytes = format!(
                "{{\"version\": 1, \"followActions\": {follow}, \"deviceTools\": true, \"rackZones\": true, \"enableWrites\": true}}\n"
            );
            // Staged beside the bridge, so a write cut short leaves nothing among its installed files.
            write_owner_file(&switch, self.scripts(), bytes.as_bytes()).map_err(|error| error.message().to_string())?;
            return Ok(if follow { TURNED_ON_WITH_FOLLOW } else { TURNED_ON });
        }
        let follow = read_json(&switch).is_some_and(|value| value["followActions"] == true);
        match std::fs::remove_file(&switch) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(if follow { TURNED_OFF_FOLLOW_STAYS } else { TURNED_OFF }),
        }
    }
}
fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
/// Willington's native libraries in a folder and up to `depth` folders below it (`build/<profile-id>/…`).
fn libraries(folder: &Path, depth: usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return vec![] };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                if depth > 0 {
                    libraries(&path, depth - 1)
                } else {
                    vec![]
                }
            } else if path.is_file() && path.extension().is_some_and(|extension| extension == "dylib" || extension == "pyd") {
                vec![path]
            } else {
                vec![]
            }
        })
        .collect()
}

/// /willington for the app: whether the bindings are on (None while the bridge in Live doesn't carry
/// Willington), and switching them, which ends with what to tell the producer.
#[derive(Clone)]
pub struct WillingtonControl {
    pub on: Rc<dyn Fn() -> Option<bool>>,
    pub set: Rc<dyn Fn(bool) -> LocalBoxFuture<'static, Result<&'static str, String>>>,
}
/// How often Kumi looks again for a bridge that doesn't carry Willington yet: first-run setup can put one in place.
const LOOK_AGAIN_MS: f64 = 2000.;
impl WillingtonControl {
    pub fn new(env: Env) -> Self {
        // The command menu asks on every frame it's drawn: the bridge, once found, stays where it is, and the
        // switch is read again only when its file changes.
        let found = RefCell::new((None::<Willington>, f64::NEG_INFINITY));
        let find: Rc<dyn Fn() -> Option<Willington>> = Rc::new(move || {
            let mut found = found.borrow_mut();
            let now = now_ms_f64();
            if found.0.is_none() && now - found.1 >= LOOK_AGAIN_MS {
                *found = (Willington::find(&env), now);
            }
            found.0.clone()
        });
        let read = Rc::new(RefCell::new(None::<(Option<(SystemTime, u64)>, bool)>));
        Self {
            on: {
                let (find, read) = (find.clone(), read.clone());
                Rc::new(move || {
                    let willington = find()?;
                    let stamp = willington.stamp();
                    let mut read = read.borrow_mut();
                    if let Some((seen, on)) = *read {
                        if seen == stamp {
                            return Some(on);
                        }
                    }
                    let on = willington.on();
                    *read = Some((stamp, on));
                    Some(on)
                })
            },
            set: Rc::new(move |on| {
                let (found, read) = (find(), read.clone());
                async move {
                    let willington = found.ok_or_else(|| "the bridge in Live doesn't carry Willington".to_string())?;
                    // On Windows an owner-only file takes PowerShell: off the app's thread.
                    let said = tokio::task::spawn_blocking(move || willington.set(on)).await.map_err(|error| error.to_string())?;
                    // Written within the same tick as the last read, the switch could keep its time: read it again.
                    *read.borrow_mut() = None;
                    said
                }
                .boxed_local()
            }),
        }
    }
}
