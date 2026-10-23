//! Whether the producer heard one of Kumi's changes: Live played after it, Kumi's own listens included (auditions,
//! listening, goal and match runs play through Main, as the producer has it). A render ends however it ends.
use kumi_runtime::{
    core::contracts::Integration,
    integrations::ableton::{integration::Ableton, options::AbletonOptions},
};
use std::rc::Rc;

#[tokio::test(flavor = "current_thread")]
async fn live_playing_during_kumis_own_listen_is_heard() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let integration = Ableton::new(AbletonOptions::new(Rc::new(|_, _| {})));
            let connection = integration.connection.clone();
            let since = connection.now().timestamp_millis();
            let render = integration.rendering.rendering_now();
            // One taken while a render runs changes nothing, and letting it go doesn't end the render.
            drop(integration.rendering.rendering_now());
            assert!(integration.rendering.is_rendering(), "a render already running");
            connection.note_transport(false);
            assert_eq!(integration.first_heard(since), None);
            connection.note_transport(true);
            assert!(integration.first_heard(since).is_some(), "Kumi's listen plays through Main: the producer hears it");
            drop(render);
            assert!(!integration.rendering.is_rendering());
            // A render that panics partway still ends.
            let rendering = &integration.rendering;
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _render = rendering.rendering_now();
                panic!("partway through a render");
            }));
            assert!(unwound.is_err() && !rendering.is_rendering());
        })
        .await;
}
