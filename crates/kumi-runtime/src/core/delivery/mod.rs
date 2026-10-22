//! Delivery (#401): files rendered for someone else's pipeline, built once for every DAW. Core groups the timeline's items into
//! assets (`assets`), names them in the project's own convention and checks names a game or a client takes (`naming`), and the
//! tool (`tool`) makes their regions, measures the DAW's own dry run against the names planned, and renders exactly those files.
//! The DAW's backend (`backend`) reads items and regions, adds regions, lists what a render would write, and renders.
pub mod assets;
pub mod backend;
#[cfg(test)]
mod fake_backend_tests;
pub mod naming;
pub mod tool;
