//! Routing (#401): the routing doctor and builder, built once for every DAW. Core reads the project's routing as a graph
//! (`graph`): every way each track reaches the master, and what's likely wrong; the tool (`tool`) says it in words with the
//! tracks' names and builds a sidechain, a parallel bus or stems as one plan, checking the graph after. The DAW's backend
//! (`backend`) reads the graph and writes the plan's steps in its own change kinds.
pub mod backend;
pub mod graph;
pub mod tool;

#[cfg(test)]
mod fake_backend_tests;
