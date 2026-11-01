//! A DAW's side of the variations tool (M7): only its own facts. What its material is (and so how far apart two takes are),
//! its operators and how a recipe's lines become material, how it reads a source, writes, finds, rewrites, marks, places, drops
//! and renders takes, and its words for the tool. Core does the rest, the same in every DAW.
use super::heard::Heard;
use super::{Material, Rng, TakeSet};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_types::contracts::JsonObject;
use serde_json::Value;
use std::collections::BTreeMap;

/// What a set varies: the DAW's words for it (`label`), what its takes are named after (`short`: "V3 · <short>"), what tells the
/// same source again (`key`), what it holds (None when the takes start from lines the model writes) and the DAW's own facts.
#[derive(Debug, Clone)]
pub struct Source<M, D> {
    pub label: String,
    pub short: String,
    pub key: String,
    pub material: Option<M>,
    pub data: D,
}

/// A backend's set of takes: its source, each take's place (the DAW's, opaque to core) and material.
pub type Set<B> =
    TakeSet<Source<<B as VariationsBackend>::Material, <B as VariationsBackend>::Data>, Value, <B as VariationsBackend>::Material>;

/// One take's recipe: the DAW's operators as the model gave them, lines the model wrote itself, another take to start from, and
/// the direction's words. `picked`: Kumi chose the direction (the model left it), so another may take its place.
#[derive(Debug, Clone, Default)]
pub struct Recipe {
    pub direction: String,
    pub ops: Vec<Value>,
    pub lines: Option<Vec<Value>>,
    pub from: Option<usize>,
    pub picked: bool,
}

/// The DAW's words for the tool's description and schema.
pub struct Words {
    pub app: &'static str,
    /// What a source is, for make ("a pattern track's lines (…), a phrase (…)…").
    pub sources: &'static str,
    /// What anchors are in the DAW's terms.
    pub anchors: &'static str,
    /// The operators and what each does.
    pub ops: &'static str,
    /// What `lines` are (the model's own material).
    pub lines: &'static str,
    /// Where takes go (`place`), and the names `place` takes.
    pub places: &'static str,
    pub place_names: &'static [&'static str],
    /// What `at` places a kept take at.
    pub at: &'static str,
    /// What `options` are (the DAW's settings for new takes).
    pub options: &'static str,
    /// The extract action's words, when the DAW has it.
    pub extract: Option<&'static str>,
    /// How the takes are heard when the DAW doesn't render them for Kumi (None: rendered and heard before they're shown).
    pub hears: Option<&'static str>,
    /// How a rewrite is made when it isn't one change, and a drop when it isn't one change for all.
    pub rewrites: Option<&'static str>,
    pub drops: Option<&'static str>,
    /// The DAW's notation the model writes in (effect commands, numbering).
    pub idioms: &'static str,
    /// Directions Kumi picks for takes the model leaves to it, each one no other take of a request has (a backend's
    /// `directions` may give a source others: a REAPER MIDI item's are its notes').
    pub directions: &'static [&'static str],
    /// JSON schemas: a source, an operator, the options and `at`.
    pub source_schema: &'static str,
    pub op_schema: &'static str,
    pub options_schema: &'static str,
    pub at_schema: &'static str,
}

/// A take's row as the backend found it: `ref` (its long ref) when it's there, `gone` when it was deleted, and the DAW's own
/// fields (its name, index, slots…). Core reads only those two.
pub fn found(row: &Value) -> bool {
    row.get("ref").and_then(Value::as_str).is_some()
}

pub fn gone(row: &Value) -> bool {
    row.get("gone") == Some(&Value::Bool(true))
}

/// Why a found take stays out of a drop: an edit Kumi's digest doesn't see (a take the producer names goes all the same), or
/// its use elsewhere (it never goes).
#[derive(Debug, Clone, PartialEq)]
pub enum Undroppable {
    Edited(String),
    InUse(String),
}

/// A rewrite made: its change, and what the take holds instead of what was asked when only part went in (and why).
pub type Rewritten<M> = (String, Option<(M, String)>);

/// What each take sounded like (or why it wasn't rendered), and the source when it was asked for.
pub type Rendered = (Option<Heard>, BTreeMap<usize, Result<Heard, String>>);

#[async_trait(?Send)]
pub trait VariationsBackend: Sized {
    type Material: Material + Clone + std::fmt::Debug + PartialEq;
    type Data: Clone + std::fmt::Debug;

    fn words(&self) -> &'static Words;

    /// What the open project's sets are kept by (its own id), or None when it has none.
    async fn project_key(&self, signal: &Signal) -> Option<String>;

    /// The source a make names, read: its material and where its takes go.
    async fn source(&self, input: &JsonObject, signal: &Signal) -> Result<Source<Self::Material, Self::Data>, String>;

    /// A kept set's key as its source's places give it now, where its key holds a position the producer may have moved since
    /// (Renoise's pattern, track and instrument numbers): a make on that source goes on with the set. None: its key as kept.
    async fn key_now(&self, _source: &Source<Self::Material, Self::Data>, _signal: &Signal) -> Option<String> {
        None
    }

    /// The directions Kumi picks from for takes of `source` the model leaves to it (the words' by default).
    fn directions(&self, _source: &Source<Self::Material, Self::Data>) -> &'static [&'static str] {
        self.words().directions
    }

    /// The operators of a direction Kumi picked for takes of `source`.
    fn direction_ops(&self, source: &Source<Self::Material, Self::Data>, direction: &str) -> Vec<Value>;

    /// Whether the model's operators are the DAW's, and fit where the source's takes go (before anything is drawn).
    fn check_ops(&self, source: &Source<Self::Material, Self::Data>, ops: &[Value]) -> Result<(), String>;

    /// Applies the operators, each `factor` times as much, to a take drawn from `source`: what each did, in a few words.
    fn apply_ops(
        &self,
        ops: &[Value],
        factor: f64,
        take: &mut Self::Material,
        source: &Self::Material,
        rng: &mut Rng,
    ) -> Result<Vec<String>, String>;

    /// The model's own lines as a take (the anchors as `base` has them).
    fn written(&self, set: &Set<Self>, lines: &[Value], base: &Self::Material) -> Result<Self::Material, String>;

    /// The empty material takes of a source with none start from.
    fn empty(&self, set: &Set<Self>) -> Self::Material;

    /// Whether a take keeps what every take keeps (the anchors): why not.
    fn anchors_kept(&self, source: &Self::Material, take: &Self::Material) -> Result<(), String>;

    /// What a take breaks that the source keeps (its key, its grid), with the operators asked for as they were applied (`factor`
    /// times as much; a transpose moves the key on purpose): words, none when nothing.
    fn judge(&self, source: Option<&Self::Material>, take: &Self::Material, ops: &[Value], factor: f64) -> Vec<String>;

    /// What a take holds, to tell later whether anyone changed it.
    fn digest(&self, material: &Self::Material) -> String;

    /// Why Kumi can't tell a found take that holds other than it was kept (`current`) from one edited since: the DAW put what
    /// Kumi wrote where Kumi can't follow it there (#606). None by default: it was edited.
    fn unsure(&self, _current: &Self::Material) -> Option<String> {
        None
    }

    /// Writes take `number` (one change): its change id, its place and its long ref.
    async fn write_new(
        &self,
        set: &Set<Self>,
        number: usize,
        material: &Self::Material,
        signal: &Signal,
    ) -> Result<(String, Value, String), (String, bool)>;

    /// Where each take is now, in order (`found`, `gone`); a take found again another way gets its place updated.
    async fn find(&self, set: &mut Set<Self>, numbers: &[usize], signal: &Signal) -> Result<Vec<Value>, String>;

    /// What a found take holds now.
    async fn current(&self, set: &Set<Self>, row: &Value, signal: &Signal) -> Result<Self::Material, String>;

    /// The take `db` quieter, the rest of it as it is (REAPER's take volume, Renoise's volume column): how core turns down a
    /// take whose render clipped where the source's didn't, written with `rewrite` (#407). Why not when the DAW can't: core
    /// draws the take again instead.
    fn quieter(&self, _take: &Self::Material, _db: f64) -> Result<Self::Material, String> {
        Err(format!("{} can't turn a take down", self.words().app))
    }

    /// Writes `after` over a found take that holds `before` (one change, or two for a setting the DAW changes apart): the id of
    /// the last change made, and, when only part of `after` went in, what the take holds instead and why.
    async fn rewrite(
        &self,
        set: &Set<Self>,
        row: &Value,
        before: &Self::Material,
        after: &Self::Material,
        signal: &Signal,
    ) -> Result<Rewritten<Self::Material>, (String, bool)>;

    /// Marks a found take kept in the DAW (renames it "… · kept"): the change, or None when the producer named it otherwise.
    async fn mark_kept(&self, set: &Set<Self>, number: usize, row: &Value, signal: &Signal) -> Result<Option<String>, String>;

    /// Puts a kept take where the producer says (one change): what it answered.
    async fn place(&self, set: &mut Set<Self>, number: usize, row: &Value, at: &JsonObject, signal: &Signal) -> Result<Value, String>;

    /// Why a found take stays out of a drop (it holds the producer's edit where its digest doesn't look, or it's in use
    /// elsewhere), or None.
    async fn undroppable(&self, set: &Set<Self>, number: usize, row: &Value, signal: &Signal) -> Option<Undroppable>;

    /// Drops found takes in one change: its id and what the DAW said.
    async fn drop(&self, set: &Set<Self>, rows: &[(usize, Value)], signal: &Signal) -> Result<(String, Value), String>;

    /// Why the DAW can't render for Kumi now, or None.
    fn cant_render(&self) -> Option<String>;

    /// Whether the DAW plays now (a render would stop it); None when it couldn't say, and then nothing renders.
    async fn playing(&self, signal: &Signal) -> Option<bool>;

    /// Renders the takes (and the source, when asked) offline and hears them.
    async fn render(&self, set: &mut Set<Self>, numbers: &[usize], with_source: bool, signal: &Signal) -> Result<Rendered, String>;

    /// A long ref as the model sees it.
    fn short(&self, long: &str) -> String;

    /// What to tell of where the takes play (pattern takes play at the song's end), or None.
    fn end_note(&self, set: &Set<Self>) -> Option<String>;

    /// The DAW's extract action (a phrase of a pattern's lines), when it has one.
    async fn extract(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String>;

    /// Material and the DAW's facts as kept between runs, and read back.
    fn material_json(&self, material: &Self::Material) -> Value;
    fn material_of(&self, value: &Value) -> Option<Self::Material>;
    fn data_json(&self, data: &Self::Data) -> Value;
    fn data_of(&self, value: &Value) -> Option<Self::Data>;
}
