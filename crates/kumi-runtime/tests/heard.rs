//! Whether the producer heard one of Kumi's changes: Live played after it, but not while Kumi played the Set for itself
//! (a silent render, Main down), as auditions, listening and goal and match runs do.
use kumi_runtime::{
    core::contracts::Integration,
    integrations::ableton::{integration::Ableton, options::AbletonOptions},
};
use std::rc::Rc;

#[tokio::test(flavor = "current_thread")]
async fn live_playing_during_kumis_own_render_isnt_heard() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let integration = Ableton::new(AbletonOptions::new(Rc::new(|_, _| {})));
            let connection = integration.connection.clone();
            let since = connection.now().timestamp_millis();
            assert!(!integration.rendering.begin_rendering());
            assert!(integration.rendering.begin_rendering(), "a render already running");
            connection.note_transport(true);
            assert_eq!(integration.first_heard(since), None);
            integration.rendering.end_rendering();
            connection.note_transport(true);
            assert!(integration.first_heard(since).is_some());
        })
        .await;
}
