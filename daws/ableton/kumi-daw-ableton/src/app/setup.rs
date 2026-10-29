//! Live's side of first-run setup, for the app: Live the app (open, quit, start, its version, what's installed),
//! Kumi's bridge put in it, and the session connecting through the bridge once it's in place.
use super::folders::find_bridge_config;
use super::{
    bridge_setup::{self, BridgeSetupIo},
    live_app,
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_daw::{
    process::Run,
    provider::Installed,
    setup::{DawSetup, SetupSession},
};
use kumi_types::system::Env;
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

/// Live's side of first-run setup, for the app: Live the app, Kumi's bridge in it, and the session
/// connecting through the bridge once it's in place.
pub(crate) struct LiveSide {
    pub(crate) env: Env,
    pub(crate) run: Run,
    /// The bridge configuration the session's integration starts from, and whether it was given
    /// (`--bridge-config`), so the one put in place doesn't replace it.
    pub(crate) bridge: Rc<RefCell<Option<String>>>,
    pub(crate) given: bool,
    pub(crate) session: Rc<RefCell<Option<Weak<dyn SetupSession>>>>,
}
#[async_trait(?Send)]
impl DawSetup for LiveSide {
    async fn open_live(&self) -> Option<String> {
        live_app::open_live(&self.run, kumi_types::system::platform(), &self.env).await.map(|live| live.app)
    }
    async fn ask_to_quit(&self) -> bool {
        live_app::ask_to_quit(&self.run, kumi_types::system::platform(), &self.env).await
    }
    async fn closed(&self, stop: &Signal) -> bool {
        live_app::closed(&self.run, kumi_types::system::platform(), &self.env, stop).await
    }
    async fn install(&self) -> Result<String, String> {
        let mut options = BridgeSetupIo::quiet(self.env.clone());
        options.run = Some(self.run.clone());
        bridge_setup::install_quietly(options).await
    }
    async fn start(&self, app: Option<String>, set: Option<String>) -> bool {
        live_app::start(&self.run, kumi_types::system::platform(), &self.env, app.as_deref(), set.as_deref()).await
    }
    async fn connect(&self) {
        if !self.given {
            if let Some(found) = find_bridge_config(&self.env) {
                *self.bridge.borrow_mut() = Some(found);
            }
        }
        // The session may still be starting: a busy one is asked again shortly.
        for _ in 0..40 {
            let session = self.session.borrow().as_ref().and_then(Weak::upgrade);
            let Some(session) = session else { return };
            if session.reconnect().await.is_ok() {
                return;
            }
            drop(session);
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    async fn version(&self, app: Option<String>) -> Option<String> {
        live_app::version(&self.run, kumi_types::system::platform(), &self.env, app.as_deref()).await
    }
    async fn installed(&self) -> Installed {
        live_app::installed(&self.env, kumi_types::system::platform())
    }
    fn open_set(&self) -> Option<String> {
        self.session.borrow().as_ref().and_then(Weak::upgrade).and_then(|session| session.open_set())
    }
    fn here(&self) -> bool {
        live_here()
    }
    fn asks_permission(&self) -> Option<String> {
        (kumi_types::system::platform() == "darwin").then(|| kumi_media::voice::microphone::terminal_app(&self.env).to_string())
    }
    fn settings_menu(&self) -> Option<String> {
        live_app::settings_menu(kumi_types::system::platform())
    }
}
/// Live runs on macOS and Windows only: elsewhere Kumi chats without it, and setup only signs in.
pub(crate) fn live_here() -> bool {
    live_app::runs_on(kumi_types::system::platform())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn a_given_bridge_config_stays_once_setup_puts_the_bridge_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("Remote Scripts/AbletonMcpBridge");
        std::fs::create_dir_all(&reference).unwrap();
        let found = dir.path().join("bridge-config.json");
        std::fs::write(&found, "{}").unwrap();
        std::fs::write(reference.join("bridge-reference.json"), serde_json::json!({ "config": found }).to_string()).unwrap();
        let env = Env::from([("KUMI_REMOTE_SCRIPTS_DIR".to_string(), dir.path().join("Remote Scripts").display().to_string())]);
        let side = |given| LiveSide {
            env: env.clone(),
            run: kumi_daw::process::default_run(),
            bridge: Rc::new(RefCell::new(Some("/given/bridge-config.json".into()))),
            given,
            session: Rc::new(RefCell::new(None)),
        };
        let given = side(true);
        given.connect().await;
        assert_eq!(given.bridge.borrow().as_deref(), Some("/given/bridge-config.json"), "--bridge-config stays");
        let looked_up = side(false);
        looked_up.connect().await;
        assert_eq!(looked_up.bridge.borrow().as_deref(), found.to_str(), "the one put in place");
    }
}
