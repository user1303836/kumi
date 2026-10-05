//! Willington, the native bindings Kumi's bridge carries into Live (`AbletonMcpBridge/willington`): off
//! until the producer turns them on with /willington. The switch is `willington.json` beside the bridge's
//! Remote Script, which the bridge reads again within a second of it changing, Live running.

use crate::config::remote_scripts_dir;
use ableton_mcp_server::delivery::{write_owner_file, REMOTE_SCRIPT_PACKAGE, WILLINGTON_CONFIG, WILLINGTON_FOLDER};
use futures::future::{FutureExt, LocalBoxFuture};
use kumi_common::time::now_ms_f64;
use kumi_runtime::system::Env;
use serde_json::Value;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

/// What Kumi says when it starts while the bindings are off.
pub const OFF_AT_START: &str = "Willington bindings are OFF currently, type /willington to toggle them on";
pub const TURNED_ON: &str = "Willington bindings are ON: Kumi can map rack macros with their ranges, name macros and variations, and set rack chain zones, on the Live versions Willington supports. /willington again turns them off.";
pub const TURNED_OFF: &str = "Willington bindings are OFF. /willington turns them on again.";

/// Every binding, with its edits: the switch /willington writes.
const ON: &[u8] = b"{\"version\": 1, \"followActions\": true, \"deviceTools\": true, \"rackZones\": true, \"enableWrites\": true}\n";

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
    /// On while the switch asks for edits from at least one binding; anything else, a missing switch too, is off.
    pub fn on(&self) -> bool {
        let Ok(bytes) = std::fs::read(self.bridge.join(WILLINGTON_CONFIG)) else { return false };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { return false };
        value["enableWrites"] == true && ["followActions", "deviceTools", "rackZones"].iter().any(|key| value[*key] == true)
    }
    /// On writes the switch, owner-only (the bridge reads no other kind); off removes it.
    pub fn set(&self, on: bool) -> Result<(), String> {
        let switch = self.bridge.join(WILLINGTON_CONFIG);
        if on {
            return write_owner_file(&switch, ON).map_err(|error| error.message().to_string());
        }
        match std::fs::remove_file(&switch) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        }
    }
}

/// /willington for the app: whether the bindings are on (None while the bridge in Live doesn't carry
/// Willington), and switching them.
#[derive(Clone)]
pub struct WillingtonControl {
    pub on: Rc<dyn Fn() -> Option<bool>>,
    pub set: Rc<dyn Fn(bool) -> LocalBoxFuture<'static, Result<(), String>>>,
}
/// How often Kumi looks again for a bridge that doesn't carry Willington yet: first-run setup can put one in place.
const LOOK_AGAIN_MS: f64 = 2000.;
impl WillingtonControl {
    pub fn new(env: Env) -> Self {
        // The command menu asks on every frame it's drawn: the bridge, once found, stays where it is.
        let found = RefCell::new((None::<Willington>, f64::NEG_INFINITY));
        let find: Rc<dyn Fn() -> Option<Willington>> = Rc::new(move || {
            let mut found = found.borrow_mut();
            let now = now_ms_f64();
            if found.0.is_none() && now - found.1 >= LOOK_AGAIN_MS {
                *found = (Willington::find(&env), now);
            }
            found.0.clone()
        });
        Self {
            on: {
                let find = find.clone();
                Rc::new(move || find().map(|willington| willington.on()))
            },
            set: Rc::new(move |on| {
                let found = find();
                async move {
                    let willington = found.ok_or_else(|| "the bridge in Live doesn't carry Willington".to_string())?;
                    // On Windows an owner-only file takes PowerShell: off the app's thread.
                    tokio::task::spawn_blocking(move || willington.set(on)).await.map_err(|error| error.to_string())?
                }
                .boxed_local()
            }),
        }
    }
}
