//! A DAW's side of routing: only its own facts. Its routing read as a graph, and the steps that build a sidechain, a parallel
//! bus or stems in its own change kinds (run as one plan: one step of its undo), which core runs and checks.
use super::graph::Graph;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// What a parent route is in the DAW ("its folder, or the master at the top").
    pub parent: &'static str,
    /// How a sidechain's compressor is chosen, in words.
    pub compressor: &'static str,
}

/// What a builder makes: a sidechain (the trigger's sound into the target's compressor), a parallel bus (sends from tracks to
/// a new bus), or stems (a bus per group, every track of a group sent to it).
#[derive(Debug, Clone, PartialEq)]
pub enum Build {
    Sidechain { trigger: String, target: String, compressor: Option<String> },
    Parallel { tracks: Vec<String>, name: String, fx: Option<String>, pre_fader: bool },
    Stems { groups: Vec<(String, Vec<String>)> },
}

/// A build's steps (the DAW's change tools and their inputs) and what to say of them (a compressor whose own sidechain switch
/// the producer turns on, a track leaving its folder).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Steps {
    pub steps: Vec<Value>,
    pub said: Vec<String>,
}

#[async_trait(?Send)]
pub trait RoutingBackend {
    fn words(&self) -> &'static Words;

    /// The project's routing, read now.
    async fn graph(&self, signal: &Signal) -> Result<Graph, String>;

    /// The plan's steps that build it on the graph as it is.
    async fn steps(&self, graph: &Graph, build: &Build, signal: &Signal) -> Result<Steps, String>;

    /// Runs the steps as one plan: what the DAW answered.
    async fn run(&self, steps: Vec<Value>, signal: &Signal) -> Result<Value, String>;
}
