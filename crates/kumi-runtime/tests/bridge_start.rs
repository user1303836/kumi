//! When Kumi's bridge can't start: its own words when it gave them, the usual way back after an update otherwise.
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::errors::RuntimeError,
    integrations::ableton::connection::{ConnectionOptions, LiveConnection},
};
use std::rc::Rc;

async fn start_with(said: &'static str) -> String {
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| async move { Err(RuntimeError::plain(said)) }.boxed_local()));
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.err().unwrap().to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn a_bridge_that_cant_start_is_said_in_its_own_words_or_the_way_back_after_an_update() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Live still running the bridge it loaded before `kumi bridge` put the new one in place: restart Live,
            // never another update.
            let restart = "Kumi's bridge didn't start: Live is running another version of Kumi's bridge than the one installed (Live loads it when it starts): restart Live";
            let said = start_with(restart).await;
            assert!(said.contains(restart) && !said.contains("run kumi bridge"), "{said}");
            // Anything else: the usual way back.
            let said = start_with("MCP connection failed; check the built bridge, config and Live setup").await;
            assert!(said.contains("After updating Kumi, Live's part needs updating too: quit Live, then run"), "{said}");
        })
        .await;
}
