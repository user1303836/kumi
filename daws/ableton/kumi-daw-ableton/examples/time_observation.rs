//! Opt-in timing on real Live: what each turn's look at the open Set costs, read-only. By default the first turn
//! reads the Set's devices whole and the turns after reuse them while Live tells of no change; with `--whole` every
//! turn reads them whole, as Kumi did before. Each turn prints its time, its Live requests and the bytes of their
//! answers.
//!   cargo run --release -p kumi-runtime --example time_observation -- <bridge-config.json> [turns] [--whole]
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::{contracts::KernelTool, errors::RuntimeError, timing},
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        observation::{ObservationHost, ObservedChange, Observer},
        remember::Remember,
    },
};
use std::{rc::Rc, time::Instant};

struct Host;
#[async_trait(?Send)]
impl ObservationHost for Host {
    fn reset_turn(&self, _: bool) {}
    fn changes(&self) -> Vec<ObservedChange> {
        vec![]
    }
    fn definitions(&self) -> Vec<Rc<dyn KernelTool>> {
        vec![]
    }
    async fn restore_after_crash(&self, _: &str, _: Option<&str>, _: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(None)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = args.first().cloned().expect("the bridge configuration's path");
    let turns: usize = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(12);
    let whole = args.iter().any(|arg| arg == "--whole");
    tokio::task::LocalSet::new()
        .run_until(async move {
            let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
            options.bridge_config = Some(config);
            let connection = LiveConnection::new(options);
            connection.start(Signal::new()).await.expect("the bridge");
            let remember = Remember::new(connection.clone(), None, None);
            let observer = Observer::new(connection.clone(), remember.clone());
            for turn in 1..=turns {
                if whole {
                    observer.forget_devices();
                }
                let recorder = timing::begin();
                let started = Instant::now();
                let observed = observer.observe(&Host, Signal::new(), None).await;
                let ms = started.elapsed().as_secs_f64() * 1000.;
                let timing = recorder.finish();
                let tracks = observed.as_ref().ok().and_then(|o| o.tracks.as_ref().map(Vec::len));
                println!(
                    "turn {turn:2}: {ms:6.1} ms · {} Live requests · {:7.1} KB from Live · reused {:?} · drift {:?} · {} tracks{}",
                    timing.live_requests,
                    timing.live_bytes as f64 / 1024.,
                    timing.set_reused,
                    timing.set_drift,
                    tracks.map(|n| n.to_string()).unwrap_or("?".into()),
                    if observed.is_err() { " (failed)" } else { "" }
                );
                // Live's events, and the backstop beside the turns, land between them.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            remember.cancel_timer();
            let _ = connection.close().await;
        })
        .await;
}
