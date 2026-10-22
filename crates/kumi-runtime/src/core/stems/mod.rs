//! Stems (#401: REAPER and Renoise have no splitter), built once for every DAW without one: a track's stretch rendered,
//! split into drums, bass and other by Kumi's own splitter (`split`), written as sound files and put back beside the track
//! (`tool`); the DAW's backend (`backend`) renders and places them.
pub mod backend;
pub mod split;
pub mod tool;

#[cfg(test)]
mod fake_backend_tests;
