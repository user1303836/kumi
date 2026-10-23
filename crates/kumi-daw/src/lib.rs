//! What a DAW's crate (`daws/<daw>/kumi-daw-<daw>`) gives Kumi's app, and what it gets from it. A DAW describes itself
//! and checks and changes its own bridge; the app runs the flows (first-run setup, `kumi bridge`, updates, rollbacks,
//! removing Kumi, the doctor, the report), draws and asks in the terminal, and keeps the table of the DAWs it
//! integrates.
//! - [`provider`]: the DAW's provider, and what its methods say back (words, setup's step, a rollback's step).
//! - [`check`]: the doctor's model, and its look through a DAW's bridge.
//! - [`setup`]: the DAW's side of first-run setup.
//! - [`io`]: what a DAW's steps get from the app: where they say things, how they ask, a test's answers.
//! - [`process`]: running programs, where Kumi's executables are, copying a folder.
//! - [`files`]: a file's text, a folder's names, when a file changed, a long line clipped, removing.
pub mod check;
pub mod files;
pub mod io;
pub mod process;
pub mod provider;
pub mod setup;
