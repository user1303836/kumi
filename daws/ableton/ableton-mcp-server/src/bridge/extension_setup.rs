//! Connect the extension in the background while a real Live is connected.
use super::{
    extension_channel::{ExtensionChannel, ExtensionChannelOptions, ExtensionLaunch},
    extension_launcher::{launch_extension, LaunchOptions, LaunchOutcome},
    router::{routed_adapter, RoutedLiveAdapter},
};
use crate::live::*;
use futures::future::LocalBoxFuture;
use kumi_common::{abort::Signal, time::now_ms};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};
pub type ExtensionLauncher = Rc<dyn Fn(LaunchOptions) -> LocalBoxFuture<'static, Result<LaunchOutcome, LiveError>>>;
#[derive(Clone)]
pub struct ExtensionSetup {
    pub storage_directory: PathBuf,
    pub installed_storage: Option<PathBuf>,
    pub launch: Option<bool>,
    pub live_app: Option<PathBuf>,
    pub log: Option<Rc<dyn Fn(&str)>>,
    pub retry_ms: Option<f64>,
    pub launcher: Option<ExtensionLauncher>,
}
impl ExtensionSetup {
    pub fn new(storage_directory: impl Into<PathBuf>) -> Self {
        Self {
            storage_directory: storage_directory.into(),
            installed_storage: None,
            launch: None,
            live_app: None,
            log: None,
            retry_ms: None,
            launcher: None,
        }
    }
}
const RELAUNCH_AFTER_MS: f64 = 5.0 * 60_000.0;
pub fn with_extension(remote: Rc<dyn AsyncLiveAdapter>, setup: ExtensionSetup) -> RoutedLiveAdapter {
    let last_launch = Rc::new(Cell::new(0.0));
    let failed_epoch = Rc::new(Cell::new(None::<Option<i64>>));
    let share: Rc<RefCell<Option<Rc<dyn Fn(&Path)>>>> = Rc::new(RefCell::new(None));
    let launch: Option<ExtensionLaunch> = if setup.launch == Some(false) {
        None
    } else {
        let remote = remote.clone();
        let setup = setup.clone();
        let shared = share.clone();
        Some(Rc::new(move || {
            let remote = remote.clone();
            let setup = setup.clone();
            let last_launch = last_launch.clone();
            let failed_epoch = failed_epoch.clone();
            let shared = shared.clone();
            Box::pin(async move {
                let epoch = remote.status()?.epoch;
                if failed_epoch.get().is_some_and(|failed| failed == epoch) {
                    return Ok(());
                }
                if (now_ms() as f64) - last_launch.get() < RELAUNCH_AFTER_MS {
                    return Ok(());
                }
                last_launch.set(now_ms() as f64);
                let mut options = LaunchOptions::new(setup.storage_directory);
                options.live_app = setup.live_app;
                options.log = setup.log;
                options.on_shared = Some(Rc::new(move |folder| {
                    if let Some(share) = shared.borrow().as_ref() {
                        share(folder);
                    }
                }));
                let outcome =
                    if let Some(launcher) = setup.launcher { launcher(options).await? } else { launch_extension(options).await? };
                if outcome == LaunchOutcome::Failed {
                    failed_epoch.set(Some(epoch));
                    last_launch.set(0.0);
                } else {
                    failed_epoch.set(None);
                }
                Ok(())
            })
        }))
    };
    let mut options = ExtensionChannelOptions::new(setup.storage_directory);
    options.installed_storage = setup.installed_storage;
    options.launch = launch;
    options.log = setup.log;
    let status_remote = remote.clone();
    options.enabled = Some(Rc::new(move || {
        status_remote.status().is_ok_and(|status| status.connected && status.provenance == Some(LiveProvenance::RealLive))
    }));
    let channel = ExtensionChannel::new(options);
    *share.borrow_mut() = Some(channel.share_callback());
    let stop = Signal::new();
    let cancel = stop.clone();
    let period = setup.retry_ms.unwrap_or(10000.0);
    let period = if period.is_finite() && (1.0..=i32::MAX as f64).contains(&period) { period.floor() } else { 1.0 };
    let period = Duration::from_secs_f64(period / 1000.0);
    let retry_channel = channel.clone();
    tokio::task::spawn_local(async move {
        let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        loop {
            tokio::select! {biased;_=cancel.cancelled()=>break,_=timer.tick()=>{if retry_channel.status().is_none(){let channel=retry_channel.clone();tokio::task::spawn_local(async move{let _=channel.connect().await;});}}}
        }
    });
    let adapter = routed_adapter(remote, channel.clone(), Some(Rc::new(move || stop.cancel())));
    tokio::task::spawn_local(async move {
        if channel.status().is_none() {
            let _ = channel.connect().await;
        }
    });
    adapter
}
