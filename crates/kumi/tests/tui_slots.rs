//! /slots in the app: the slots shown, a swap made and taken back, and a model Kumi can't run yet refused with why.
#[path = "support/tui_app.rs"]
mod support;
use kumi_runtime::{
    auth::store::{open_credential_store, CredentialStore},
    slots::{self, Choice, Job, Slots, SlotsContext},
};
use std::rc::Rc;
use support::*;

#[tokio::test(flavor = "current_thread")]
async fn slots_are_shown_swapped_and_taken_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let file = slots::file_in(folder.path());
            let context = Rc::new(SlotsContext {
                file: file.clone(),
                store: Rc::new(open_credential_store(folder.path().join("auth.json"))) as Rc<dyn CredentialStore>,
                env: Default::default(),
            });
            let h = Harness::with(140, 40, Rc::new(Control::default()), |options| options.slots = Some(context));
            h.start().await;
            h.connect();
            h.type_text("/sl").await;
            h.has("Which model does each listening job");
            h.type_text("\x15/slots\r").await;
            h.has("Model slots");
            h.has("stems (a mix split into its parts): Live's own splitter");
            h.has("embeddings (how alike two sounds are): Kumi's own: LAION-CLAP's music model");
            h.type_text("/slots stems https://huggingface.co/someone/stems\r").await;
            h.has("Kumi can't run a stem model itself yet");
            assert_eq!(Slots::load(&file), Slots::default());
            h.type_text("/slots listening off\r").await;
            h.has("Listening now uses off: the meters alone.");
            assert_eq!(Slots::load(&file).now(Job::Listening), Choice::Off);
            h.type_text("/slots back listening\r").await;
            h.has("Listening is back on the lookup");
            assert_eq!(Slots::load(&file), Slots::default());
            h.close().await;
        })
        .await;
}
