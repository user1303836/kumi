//! When Kumi's bridge can't start: its own words when it gave them, the usual way back after an update otherwise.
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::errors::RuntimeError,
    integrations::ableton::connection::{ConnectionOptions, LiveConnection},
};
use std::rc::Rc;

async fn start_with(said: String) -> String {
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| {
        let said = said.clone();
        async move { Err(RuntimeError::plain(said)) }.boxed_local()
    }));
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.err().unwrap().to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn a_bridge_that_cant_start_is_said_in_its_own_words_or_the_way_back_after_an_update() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Live still running the bridge it loaded before `kumi bridge` put the new one in place: restart Live, in
            // the bridge's words, never the update advice.
            let restart = format!("Kumi's bridge didn't start: {}", kumi_common::bridge::ANOTHER_BRIDGE);
            let said = start_with(restart.clone()).await;
            assert!(said.contains(&restart) && !said.contains("After updating Kumi"), "{said}");
            // Anything else the bridge said is the cause, with the usual way back after it.
            let said = start_with("Kumi's bridge didn't start: Connection refused (os error 61)".into()).await;
            assert!(
                said.starts_with("Kumi couldn't start its bridge to Live (Connection refused (os error 61)). After updating Kumi, Live's part needs updating too: quit Live, then run"),
                "{said}"
            );
            let said = start_with("MCP connection failed; check the built bridge, config and Live setup".into()).await;
            assert!(said.starts_with("Kumi couldn't start its bridge to Live. After updating Kumi, Live's part needs updating too: quit Live, then run"), "{said}");
        })
        .await;
}
