//! Kumi's changes to a Logic Pro project (M3): a preview that writes nothing and fences what it saw, an apply that
//! checks the fence again, writes through one channel (a header's control, the Inspector's name field, a menu item,
//! the Marker List, the Event List, the control bar), reads the value back and answers `applied` only when Logic
//! shows it (else `uncertain`), and a restore by value where the value can be set, else by Logic's own undo when the
//! Edit menu's Undo title still names Kumi's last step. The wire is the REAPER extension's (the host's journal is
//! the arbiter): the same methods, params and result fields, with Logic's titles and words.
//!
//! Logic keeps no undo step for a mixer move, a tempo nudge or a bypass (its history holds edits: renames, tracks,
//! markers, events), so those answer `undo.entry: false` and are put back by value; and its titles carry no nonce,
//! so the producer's own undo is named back to Kumi by [`undo::Watch`], which keeps Kumi's steps in order.

mod automation;
pub(crate) mod bounce;
mod command;
pub(crate) mod cycle;
pub(crate) mod front;
mod fx;
mod import;
mod lists;
mod marks;
mod menus;
mod metronome;
mod notes;
pub(crate) mod nudge;
pub mod panel;
mod panes;
mod params;
mod patch;
mod plugins;
mod preset;
mod quantize;
mod regions;
mod sample;
mod save;
mod sends;
mod smart;
pub mod smf;
mod stack;
pub(crate) mod strips;
mod structure;
pub(crate) mod timeline;
pub(crate) mod tracks;
pub mod undo;
mod window;

use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::ax::Ax;
use crate::keys::KeyCommands;
use crate::project::Project;
use crate::queue::Answer;
use crate::refs::{bad, failed, Fault, Ref};

pub use undo::Watch;

pub type Done<T> = Result<T, Fault>;

/// Whether the changes wait for Logic to settle after a write (a click, a menu item, a list's press): a replay over a
/// recorded scene has nothing to wait for, and turns the waits off.
static SETTLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_settling(on: bool) {
    SETTLING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the changes wait for Logic (false over a recorded scene, where a read repeated would eat the next step's).
pub(crate) fn settling() -> bool {
    SETTLING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Waits for Logic to settle, unless the waits are off.
pub(crate) fn pause(duration: std::time::Duration) {
    if SETTLING.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(duration);
    }
}

/// What a kind needs of the world it changes: the project reader (its layout, strings and epoch) and the walker.
pub struct Ctx<'a, 'h> {
    pub project: &'a mut Project,
    pub ax: &'a mut Ax<'h>,
}

/// What a change would do. `prior` is what undo puts back; `fence` what apply checks again before it writes.
pub struct Preview {
    pub target: Value,
    pub prior: Value,
    pub fence: Value,
    pub display: Value,
}

/// What an apply did, and what the undo watch keeps of it.
#[derive(Default)]
pub struct Applied {
    pub applied: Value,
    /// Where the target is now, when the apply moved it (a region moved, resized or renamed moves the epoch): the
    /// restore is asked for this ref.
    pub target: Option<Value>,
    pub created: Option<Value>,
    pub material: Option<Value>,
    pub display: Value,
    /// Logic's name for the undo step the change made, when it makes one ("Renaming", "Delete Tracks").
    pub step: Option<&'static str>,
    /// How the tick can see the change undone by hand, when it can.
    pub probe: Option<undo::Probe>,
}

/// What a restore is told: the change as the journal kept it.
pub struct Restore<'a> {
    pub target: &'a Value,
    pub prior: &'a Value,
    pub applied: &'a Value,
    pub created: Option<&'a Value>,
    pub material: Option<&'a Value>,
    pub discard: bool,
    /// The restore's own description (the journal's `Kumi: undo … (k…)`).
    pub desc: &'a str,
    /// The change's kind and target, as the watch keys its steps: what a restore through Logic's own undo checks.
    pub key: String,
}

/// What the watch keys a change by: its kind and its target's ref (an add has none).
fn change_key(kind: &str, target: &Value) -> String {
    format!("{kind} {}", target.as_str().unwrap_or(""))
}

pub struct Restored {
    pub restored: Value,
    pub display: Value,
    /// Put back through Logic's own undo (the Edit menu's Undo), not by value.
    pub undone_in_logic: bool,
}

impl Restored {
    pub fn by_value(restored: Value, display: Value) -> Restored {
        Restored { restored, display, undone_in_logic: false }
    }
}

/// A kind of change.
#[derive(Clone, Copy)]
pub enum Kind {
    TrackProp(&'static tracks::Prop),
    TrackRename,
    TrackAdd,
    TrackDelete,
    MarkerAdd,
    MarkerMove,
    MarkerDelete,
    Tempo,
    Notes(notes::Mode),
    FxBypass,
    KeyCommand,
    RegionMove,
    RegionResize,
    RegionSplit,
    RegionLoop,
    RegionRename,
    RegionDelete,
    TrackStack,
    TrackStackFlatten,
    LoopRange,
    NotesQuantize,
    MidiImport,
    TempoMarkerSet,
    TempoMarkerDelete,
    SignatureSet,
    FxAdd,
    FxDelete,
    FxParam,
    FxParams,
    FxPreset,
    SendAdd,
    SendRemove,
    SendVolume,
    SmartControlSet,
    PatchLoad,
    Automation(automation::Mode),
    MetronomeSet,
    SampleLoad,
}

/// The kinds Logic's bridge can't make yet, and why, so the refusal says so.
const NOT_YET: [(&str, &str); 5] = [
    ("marker-rename", "a marker's name cell in the Marker List isn't a text field"),
    ("send-mode", "a send's mode is a mark in its menu, which Logic Pro draws itself and Accessibility can't read, so it could never be read back"),
    ("fx-move", "Logic Pro moves a plug-in from one slot to another only by dragging it, and no key command or menu item moves one"),
    ("track-move", "the track headers can't be dragged from outside, no key command reorders tracks, and the headers offer no action but their menu"),
    ("track-color", "Logic Pro's Color palette shows its swatches to Accessibility as bare positions, and the track headers show no colour: nothing to read back"),
];

impl std::fmt::Debug for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::TrackProp(prop) => write!(f, "TrackProp({})", prop.kind),
            Kind::TrackRename => f.write_str("TrackRename"),
            Kind::TrackAdd => f.write_str("TrackAdd"),
            Kind::TrackDelete => f.write_str("TrackDelete"),
            Kind::MarkerAdd => f.write_str("MarkerAdd"),
            Kind::MarkerMove => f.write_str("MarkerMove"),
            Kind::MarkerDelete => f.write_str("MarkerDelete"),
            Kind::Tempo => f.write_str("Tempo"),
            Kind::Notes(mode) => write!(f, "Notes({mode:?})"),
            Kind::FxBypass => f.write_str("FxBypass"),
            Kind::KeyCommand => f.write_str("KeyCommand"),
            Kind::RegionMove => f.write_str("RegionMove"),
            Kind::RegionResize => f.write_str("RegionResize"),
            Kind::RegionSplit => f.write_str("RegionSplit"),
            Kind::RegionLoop => f.write_str("RegionLoop"),
            Kind::RegionRename => f.write_str("RegionRename"),
            Kind::RegionDelete => f.write_str("RegionDelete"),
            Kind::TrackStack => f.write_str("TrackStack"),
            Kind::TrackStackFlatten => f.write_str("TrackStackFlatten"),
            Kind::LoopRange => f.write_str("LoopRange"),
            Kind::NotesQuantize => f.write_str("NotesQuantize"),
            Kind::MidiImport => f.write_str("MidiImport"),
            Kind::TempoMarkerSet => f.write_str("TempoMarkerSet"),
            Kind::TempoMarkerDelete => f.write_str("TempoMarkerDelete"),
            Kind::SignatureSet => f.write_str("SignatureSet"),
            Kind::FxAdd => f.write_str("FxAdd"),
            Kind::FxDelete => f.write_str("FxDelete"),
            Kind::FxParam => f.write_str("FxParam"),
            Kind::FxParams => f.write_str("FxParams"),
            Kind::FxPreset => f.write_str("FxPreset"),
            Kind::SendAdd => f.write_str("SendAdd"),
            Kind::SendRemove => f.write_str("SendRemove"),
            Kind::SendVolume => f.write_str("SendVolume"),
            Kind::SmartControlSet => f.write_str("SmartControlSet"),
            Kind::PatchLoad => f.write_str("PatchLoad"),
            Kind::Automation(mode) => write!(f, "Automation({mode:?})"),
            Kind::MetronomeSet => f.write_str("MetronomeSet"),
            Kind::SampleLoad => f.write_str("SampleLoad"),
        }
    }
}

pub fn kind(name: &str) -> Option<Kind> {
    if let Some(prop) = tracks::PROPS.iter().find(|prop| prop.kind == name) {
        return Some(Kind::TrackProp(prop));
    }
    match name {
        "track-rename" => Some(Kind::TrackRename),
        "track-add" => Some(Kind::TrackAdd),
        "track-delete" => Some(Kind::TrackDelete),
        "marker-add" => Some(Kind::MarkerAdd),
        "marker-move" => Some(Kind::MarkerMove),
        "marker-delete" => Some(Kind::MarkerDelete),
        "tempo-set" => Some(Kind::Tempo),
        "notes-add" => Some(Kind::Notes(notes::Mode::Add)),
        "notes-change" => Some(Kind::Notes(notes::Mode::Change)),
        "notes-delete" => Some(Kind::Notes(notes::Mode::Delete)),
        "fx-bypass" => Some(Kind::FxBypass),
        "key-command" => Some(Kind::KeyCommand),
        "region-move" => Some(Kind::RegionMove),
        "region-resize" => Some(Kind::RegionResize),
        "region-split" => Some(Kind::RegionSplit),
        "region-loop" => Some(Kind::RegionLoop),
        "region-rename" => Some(Kind::RegionRename),
        "region-delete" => Some(Kind::RegionDelete),
        "track-stack" => Some(Kind::TrackStack),
        "track-stack-flatten" => Some(Kind::TrackStackFlatten),
        "loop-range" => Some(Kind::LoopRange),
        "notes-quantize" => Some(Kind::NotesQuantize),
        "midi-import" => Some(Kind::MidiImport),
        "tempo-marker-set" => Some(Kind::TempoMarkerSet),
        "tempo-marker-delete" => Some(Kind::TempoMarkerDelete),
        "signature-set" => Some(Kind::SignatureSet),
        "fx-add" => Some(Kind::FxAdd),
        "fx-delete" => Some(Kind::FxDelete),
        "fx-param" => Some(Kind::FxParam),
        "fx-params" => Some(Kind::FxParams),
        "fx-preset" => Some(Kind::FxPreset),
        "send-add" => Some(Kind::SendAdd),
        "send-remove" => Some(Kind::SendRemove),
        "send-volume" => Some(Kind::SendVolume),
        "smart-control-set" => Some(Kind::SmartControlSet),
        "patch-load" => Some(Kind::PatchLoad),
        "automation-add" => Some(Kind::Automation(automation::Mode::Add)),
        "automation-change" => Some(Kind::Automation(automation::Mode::Change)),
        "automation-delete" => Some(Kind::Automation(automation::Mode::Delete)),
        "metronome-set" => Some(Kind::MetronomeSet),
        "sample-load" => Some(Kind::SampleLoad),
        _ => None,
    }
}

/// Every kind the bridge makes, by its wire name.
pub const KINDS: [&str; 46] = [
    "track-volume",
    "track-pan",
    "track-mute",
    "track-solo",
    "track-arm",
    "track-rename",
    "track-add",
    "track-delete",
    "marker-add",
    "marker-move",
    "marker-delete",
    "tempo-set",
    "notes-add",
    "notes-change",
    "notes-delete",
    "fx-bypass",
    "key-command",
    "region-move",
    "region-resize",
    "region-split",
    "region-loop",
    "region-rename",
    "region-delete",
    "loop-range",
    "notes-quantize",
    "midi-import",
    "tempo-marker-set",
    "tempo-marker-delete",
    "signature-set",
    "track-stack",
    "track-stack-flatten",
    "fx-add",
    "fx-delete",
    "fx-param",
    "fx-params",
    "fx-preset",
    "patch-load",
    "send-add",
    "send-remove",
    "send-volume",
    "smart-control-set",
    "automation-add",
    "automation-change",
    "automation-delete",
    "metronome-set",
    "sample-load",
];

impl Kind {
    fn preview(self, ctx: &mut Ctx, keys: &KeyCommands, input: &Value) -> Done<Preview> {
        match self {
            Kind::TrackProp(prop) => tracks::prop_preview(ctx, prop, input),
            Kind::TrackRename => tracks::rename_preview(ctx, input),
            Kind::TrackAdd => structure::add_preview(ctx, input),
            Kind::TrackDelete => structure::delete_preview(ctx, input),
            Kind::MarkerAdd => marks::add_preview(ctx, input),
            Kind::MarkerMove => marks::move_preview(ctx, input),
            Kind::MarkerDelete => marks::delete_preview(ctx, input),
            Kind::Tempo => timeline::tempo_preview(ctx, input),
            Kind::Notes(mode) => notes::preview(ctx, input, mode),
            Kind::FxBypass => fx::bypass_preview(ctx, input),
            Kind::KeyCommand => unreachable!("previewed with the key command set"),
            Kind::RegionMove => regions::move_preview(ctx, input),
            Kind::RegionResize => regions::resize_preview(ctx, input),
            Kind::RegionSplit => regions::split_preview(ctx, input),
            Kind::RegionLoop => regions::loop_preview(ctx, input),
            Kind::RegionRename => regions::rename_preview(ctx, input),
            Kind::RegionDelete => regions::delete_preview(ctx, input),
            Kind::TrackStack => stack::stack_preview(ctx, input),
            Kind::TrackStackFlatten => stack::flatten_preview(ctx, input),
            Kind::LoopRange => cycle::preview(ctx, input),
            Kind::NotesQuantize => quantize::preview(ctx, input),
            Kind::MidiImport => import::preview(ctx, input),
            Kind::TempoMarkerSet => lists::tempo_set_preview(ctx, input),
            Kind::TempoMarkerDelete => lists::tempo_delete_preview(ctx, input),
            Kind::SignatureSet => lists::signature_preview(ctx, input),
            Kind::FxAdd => plugins::add_preview(ctx, input),
            Kind::FxDelete => plugins::delete_preview(ctx, input),
            Kind::FxParam => params::preview(ctx, input, false),
            Kind::FxParams => params::preview(ctx, input, true),
            Kind::FxPreset => preset::preview(ctx, input),
            Kind::SendAdd => sends::add_preview(ctx, input),
            Kind::SendRemove => sends::remove_preview(ctx, input),
            Kind::SendVolume => sends::volume_preview(ctx, input),
            Kind::SmartControlSet => smart::preview(ctx, input),
            Kind::PatchLoad => patch::preview(ctx, input),
            Kind::Automation(mode) => automation::preview(ctx, keys, input, mode),
            Kind::MetronomeSet => metronome::preview(ctx, input),
            Kind::SampleLoad => sample::preview(ctx, input),
        }
    }

    fn apply(self, ctx: &mut Ctx, keys: &KeyCommands, input: &Value, fence: &Value, desc: &str) -> Done<Applied> {
        match self {
            Kind::TrackProp(prop) => tracks::prop_apply(ctx, prop, input, fence),
            Kind::TrackRename => tracks::rename_apply(ctx, input, fence),
            Kind::TrackAdd => structure::add_apply(ctx, input, fence),
            Kind::TrackDelete => structure::delete_apply(ctx, input, fence, desc),
            Kind::MarkerAdd => marks::add_apply(ctx, input, fence),
            Kind::MarkerMove => marks::move_apply(ctx, input, fence),
            Kind::MarkerDelete => marks::delete_apply(ctx, input, fence),
            Kind::Tempo => timeline::tempo_apply(ctx, input, fence),
            Kind::Notes(mode) => notes::apply(ctx, input, fence, mode),
            Kind::FxBypass => fx::bypass_apply(ctx, input, fence),
            Kind::KeyCommand => unreachable!("applied with the key command set"),
            Kind::RegionMove => regions::move_apply(ctx, input, fence),
            Kind::RegionResize => regions::resize_apply(ctx, keys, input, fence),
            Kind::RegionSplit => regions::split_apply(ctx, keys, input, fence),
            Kind::RegionLoop => regions::loop_apply(ctx, input, fence),
            Kind::RegionRename => regions::rename_apply(ctx, input, fence),
            Kind::RegionDelete => regions::delete_apply(ctx, input, fence, desc),
            Kind::TrackStack => stack::stack_apply(ctx, keys, input, fence),
            Kind::TrackStackFlatten => stack::flatten_apply(ctx, keys, input, fence),
            Kind::LoopRange => cycle::apply(ctx, input, fence),
            Kind::NotesQuantize => quantize::apply(ctx, input, fence),
            Kind::MidiImport => import::apply(ctx, input, fence),
            Kind::TempoMarkerSet => lists::tempo_set_apply(ctx, input, fence),
            Kind::TempoMarkerDelete => lists::tempo_delete_apply(ctx, input, fence),
            Kind::SignatureSet => lists::signature_apply(ctx, input, fence),
            Kind::FxAdd => plugins::add_apply(ctx, input, fence),
            Kind::FxDelete => plugins::delete_apply(ctx, input, fence, desc),
            Kind::FxParam => params::apply(ctx, input, fence, false),
            Kind::FxParams => params::apply(ctx, input, fence, true),
            Kind::FxPreset => preset::apply(ctx, input, fence),
            Kind::SendAdd => sends::add_apply(ctx, input, fence),
            Kind::SendRemove => sends::remove_apply(ctx, input, fence, desc),
            Kind::SendVolume => sends::volume_apply(ctx, input, fence),
            Kind::SmartControlSet => smart::apply(ctx, input, fence),
            Kind::PatchLoad => patch::apply(ctx, input, fence, desc),
            Kind::Automation(mode) => automation::apply(ctx, keys, input, fence, mode),
            Kind::MetronomeSet => metronome::apply(ctx, input, fence),
            Kind::SampleLoad => sample::apply(ctx, input, fence),
        }
    }

    fn restore(self, ctx: &mut Ctx, keys: &KeyCommands, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
        match self {
            Kind::TrackProp(prop) => tracks::prop_restore(ctx, prop, restore),
            Kind::TrackRename => tracks::rename_restore(ctx, restore),
            Kind::TrackAdd => structure::add_restore(ctx, restore),
            Kind::TrackDelete => structure::delete_restore(ctx, watch, restore),
            Kind::MarkerAdd => marks::add_restore(ctx, restore),
            Kind::MarkerMove => marks::move_restore(ctx, restore),
            Kind::MarkerDelete => marks::delete_restore(ctx, watch, restore),
            Kind::Tempo => timeline::tempo_restore(ctx, restore),
            Kind::Notes(_) => notes::restore(ctx, restore),
            Kind::FxBypass => fx::bypass_restore(ctx, restore),
            Kind::KeyCommand => Err((
                "daw-only".into(),
                "Kumi can't take back what a key command did: Logic Pro's own undo can (Cmd-Z in Logic Pro)".into(),
            )),
            Kind::RegionMove => regions::move_restore(ctx, restore),
            Kind::RegionResize => regions::resize_restore(ctx, keys, restore),
            Kind::RegionSplit => regions::split_restore(ctx, watch, restore),
            Kind::RegionLoop => regions::loop_restore(ctx, restore),
            Kind::RegionRename => regions::rename_restore(ctx, restore),
            Kind::RegionDelete => regions::delete_restore(ctx, watch, restore),
            Kind::TrackStack => stack::stack_restore(ctx, keys, watch, restore),
            Kind::TrackStackFlatten => stack::flatten_restore(ctx, keys, watch, restore),
            Kind::LoopRange => cycle::restore(ctx, restore),
            Kind::NotesQuantize => quantize::restore(ctx, watch, restore),
            Kind::MidiImport => import::restore(ctx, watch, restore),
            Kind::TempoMarkerSet => lists::tempo_set_restore(ctx, restore),
            Kind::TempoMarkerDelete => lists::tempo_delete_restore(ctx, watch, restore),
            Kind::SignatureSet => lists::signature_restore(ctx, watch, restore),
            Kind::FxAdd => plugins::add_restore(ctx, restore),
            Kind::FxDelete => plugins::delete_restore(ctx, watch, restore),
            Kind::FxParam | Kind::FxParams => params::restore(ctx, restore),
            Kind::FxPreset => preset::restore(ctx, restore),
            Kind::SendAdd => sends::add_restore(ctx, watch, restore),
            Kind::SendRemove => sends::remove_restore(ctx, watch, restore),
            Kind::SendVolume => sends::volume_restore(ctx, restore),
            Kind::SmartControlSet => smart::restore(ctx, restore),
            Kind::PatchLoad => patch::restore(ctx, watch, restore),
            Kind::Automation(_) => automation::restore(ctx, keys, restore),
            Kind::MetronomeSet => metronome::restore(ctx, restore),
            Kind::SampleLoad => sample::restore(ctx, watch, restore),
        }
    }
}

/// The change side's state across requests: the undo watch, the counted plan steps and the key command set.
pub struct Changes {
    pub watch: Watch,
    steps: u64,
    /// A plan step left open by `undo_step_begin`: its label, when it began, and the changes applied in it so far.
    open_step: Option<(String, Instant, usize)>,
    /// The key command set the bridge posts with: the producer's (the bridge loads it on attach), else the built-in
    /// U.S. factory set (which a replay of a recorded scene uses too).
    pub keys: KeyCommands,
}

impl Default for Changes {
    fn default() -> Self {
        Changes { watch: Watch::default(), steps: 0, open_step: None, keys: KeyCommands::embedded() }
    }
}

/// A fault as the wire gives it, for a change: `changed` (the fence moved), `gone`, `uncertain`.
pub(crate) fn changed(message: impl Into<String>) -> Fault {
    (daw_bridge::wire::code::CHANGED.into(), message.into())
}

pub(crate) fn gone(message: impl Into<String>) -> Fault {
    ("gone".into(), message.into())
}

pub(crate) fn uncertain(message: impl Into<String>) -> Fault {
    (daw_bridge::wire::code::UNCERTAIN.into(), message.into())
}

pub(crate) fn busy(message: impl Into<String>) -> Fault {
    (daw_bridge::wire::code::BUSY.into(), message.into())
}

/// A helper or Accessibility error inside a change, in the wire's words: the missing grant is `failed` in words,
/// never busy; a timeout after a write is `uncertain`.
pub(crate) fn ax_fault(error: &crate::ax::AxError, wrote: bool) -> Fault {
    if crate::ax::untrusted(error) {
        return failed(crate::server::UNTRUSTED);
    }
    if wrote {
        return uncertain(format!("Logic Pro didn't answer after the change was sent: {error}"));
    }
    failed(format!("couldn't read Logic Pro: {error}"))
}

fn text<'a>(params: &'a Value, name: &str) -> Done<&'a str> {
    params.get(name).and_then(Value::as_str).filter(|text| !text.is_empty()).ok_or_else(|| bad(format!("{name} must be a string")))
}

fn object<'a>(params: &'a Value, name: &str) -> Done<&'a Value> {
    params.get(name).filter(|value| value.is_object()).ok_or_else(|| bad(format!("{name} must be an object")))
}

/// A name inside a title's sentence, in curly quotes as the REAPER extension's titles quote theirs.
pub(crate) fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// The kind a request names.
fn kind_of(params: &Value) -> Done<Kind> {
    let name = text(params, "kind")?;
    if let Some((_, why)) = NOT_YET.iter().find(|(not_yet, _)| *not_yet == name) {
        return Err(bad(format!("Logic Pro's bridge can't make a {name} change yet: {why}")));
    }
    kind(name).ok_or_else(|| bad(format!("Logic Pro has no change kind {name:?}")))
}

/// The request's epoch against the project's: a request made before the tracks or regions changed names refs that
/// mean something else now.
fn check_epoch(ctx: &Ctx, params: &Value) -> Done<()> {
    let epoch = text(params, "epoch").map_err(|_| bad("epoch: the epoch the refs were read in"))?;
    let now = ctx.project.epoch.to_string();
    if epoch != now {
        return Err((
            daw_bridge::wire::code::STALE.into(),
            format!("those refs are from an earlier epoch ({epoch}; now {now}): the tracks or regions changed, read again"),
        ));
    }
    Ok(())
}

/// Whose epoch a ref must carry: the project's (a request's input), or any (a restore's target and created refs,
/// which the journal kept from the preview and the apply and sends with the apply's epoch, checked first).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Epoch {
    Current,
    Any,
}

/// A ref of a kind, taken apart: its numbers (a track's place; a note's region and place; a plug-in's track and slot).
pub(crate) fn parse_ref(ctx: &Ctx, input: &Value, key: &str, kind: &str, epoch: Epoch, numbers: usize) -> Done<Vec<usize>> {
    let text = input.get(key).and_then(Value::as_str).ok_or_else(|| bad(format!("{key} must be a {kind} ref")))?;
    let example = match numbers {
        1 => format!("{}:{kind}:1", ctx.project.epoch),
        _ => format!("{}:{kind}:1.1", ctx.project.epoch),
    };
    let reference = Ref::parse(text).ok_or_else(|| bad(format!("{key} must be a {kind} ref, such as {example}")))?;
    if epoch == Epoch::Current && reference.epoch != ctx.project.epoch {
        return Err(crate::refs::stale("ref", reference.epoch, ctx.project.epoch));
    }
    if reference.kind != kind || reference.numbers.len() != numbers || reference.numbers.iter().any(|n| *n == 0) {
        return Err(bad(format!("{key} must be a {kind} ref ({example}), not {text}")));
    }
    Ok(reference.numbers)
}

/// A track ref: its place (from 1). A restore's ref from an earlier epoch is carried to where the track is now.
pub(crate) fn track_place(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<usize> {
    let numbers = parse_ref(ctx, input, key, "track", epoch, 1)?;
    carried(ctx, input, key, "track", numbers[0], |project, from, place| project.carry_track(from, place))
}

/// A region ref: its place (from 1), carried likewise.
pub(crate) fn region_place(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<usize> {
    let numbers = parse_ref(ctx, input, key, "region", epoch, 1)?;
    carried(ctx, input, key, "region", numbers[0], |project, from, place| project.carry_region(from, place))
}

/// A place from a ref's own epoch carried to the epoch now.
fn carried(
    ctx: &Ctx,
    input: &Value,
    key: &str,
    kind: &str,
    place: usize,
    carry: impl Fn(&Project, u32, usize) -> Option<usize>,
) -> Done<usize> {
    let text = input.get(key).and_then(Value::as_str).unwrap_or_default();
    let from = Ref::parse(text).map(|r| r.epoch).unwrap_or(ctx.project.epoch);
    if from == ctx.project.epoch {
        return Ok(place);
    }
    if from > ctx.project.epoch {
        return Err(crate::refs::stale("ref", from, ctx.project.epoch));
    }
    carry(ctx.project, from, place)
        .ok_or_else(|| gone(format!("the {kind} {text} named isn't in the project any more (or that epoch is too far back)")))
}

/// The ref of a thing at a place in the current epoch.
pub(crate) fn ref_text(ctx: &Ctx, kind: &str, numbers: &[usize]) -> String {
    Ref::new(ctx.project.epoch, kind, numbers).text()
}

/// The reads that open a window or a pane of Logic's to answer, through the change side's channels: a plug-in's
/// parameters (`param`, its window's Controls View), a track's Smart Controls (`smart`) and automation (`automation`).
/// None for the other kinds, which the catalog answers.
pub fn read_wide(project: &mut Project, ax: &mut Ax, keys: &KeyCommands, want: &crate::read::Want, params: &Value) -> Option<Answer> {
    use crate::read::Want;
    let (kind, one) = match want {
        Want::One(reference) => (Ref::parse(reference)?.kind, Some(reference.clone())),
        Want::Kind { kind, .. } => (kind.clone(), None),
    };
    let mut ctx = Ctx { project, ax };
    let rows: Done<Vec<Value>> = match kind.as_str() {
        "param" => (|| {
            let (owner, slot) = match &one {
                Some(reference) => {
                    let (owner, slot, _) = strips::param(&ctx, &json!({"ref": reference}), "ref", Epoch::Current)?;
                    (owner, slot)
                }
                None => strips::plugin(&ctx, params, "parent", Epoch::Current)?,
            };
            params::rows(&mut ctx, owner, slot)
        })(),
        "smart" => (|| {
            let place = match &one {
                Some(reference) => Ref::parse(reference).map(|r| r.first()).unwrap_or(0),
                None => track_place(&ctx, params, "parent", Epoch::Current)?,
            };
            smart::rows(&mut ctx, place)
        })(),
        "automation" => (|| {
            let place = match &one {
                Some(reference) => Ref::parse(reference).map(|r| r.first()).unwrap_or(0),
                None => track_place(&ctx, params, "parent", Epoch::Current)?,
            };
            automation::rows(&mut ctx, keys, place)
        })(),
        _ => return None,
    };
    let epoch = ctx.project.epoch;
    Some(rows.and_then(|rows| match one {
        Some(reference) => match rows.into_iter().find(|row| row["ref"] == reference.as_str()) {
            Some(row) => Ok(json!({"items": [row], "epoch": epoch.to_string()})),
            None => Err(crate::refs::not_in_project(&reference)),
        },
        None => crate::read::page(&rows, params, epoch),
    }))
}

impl Changes {
    /// `change_preview {kind, input, epoch}`: what the change would do, writing nothing.
    pub fn preview(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        check_epoch(ctx, params)?;
        let kind = kind_of(params)?;
        let preview = match kind {
            Kind::KeyCommand => command::preview(self, ctx, object(params, "input")?)?,
            other => other.preview(ctx, &self.keys, object(params, "input")?)?,
        };
        Ok(json!({
            "epoch": ctx.project.epoch.to_string(),
            "target": preview.target,
            "prior": preview.prior,
            "fence": preview.fence,
            "display": preview.display,
        }))
    }

    /// `change_apply {kind, input, fence, desc, epoch}`: the change made and read back; `undo` says whether Logic's
    /// history took a step for it, and what its Undo title says.
    pub fn apply(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        check_epoch(ctx, params)?;
        let kind = kind_of(params)?;
        if let Kind::KeyCommand = kind {
            return command::apply(self, ctx, params);
        }
        let (input, fence, desc) = (object(params, "input")?, object(params, "fence")?, text(params, "desc")?);
        let applied = kind.apply(ctx, &self.keys, input, fence, desc)?;
        if let Some((_, _, count)) = self.open_step.as_mut() {
            *count += 1;
        }
        // The watch keys the step by the target where the apply left it (a moved region's new ref).
        let key = change_key(text(params, "kind")?, applied.target.as_ref().map_or(&input["ref"], |target| &target["ref"]));
        let undo = self.watch.applied(ctx, desc, &key, applied.step, applied.probe);
        let mut answer = Map::new();
        answer.insert("epoch".into(), ctx.project.epoch.to_string().into());
        answer.insert("applied".into(), applied.applied);
        if let Some(target) = applied.target {
            answer.insert("target".into(), target);
        }
        if let Some(created) = applied.created {
            answer.insert("created".into(), created);
        }
        if let Some(material) = applied.material {
            answer.insert("material".into(), material);
        }
        answer.insert("display".into(), applied.display);
        answer.insert("undo".into(), undo);
        Ok(Value::Object(answer))
    }

    /// `change_restore {kind, target, prior, applied, created?, material?, discard?, desc, epoch}`: the change put
    /// back, by value, or by Logic's own undo when that is the only way and its Undo title is still this step. Its
    /// refs are from the change's own epoch, carried to where their tracks and regions stand now.
    pub fn restore(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        let kind = kind_of(params)?;
        if let Kind::KeyCommand = kind {
            return command::restore();
        }
        let desc = text(params, "desc")?;
        let target = object(params, "target")?;
        let key = change_key(text(params, "kind")?, &target["ref"]);
        let restore = Restore {
            target,
            prior: params.get("prior").unwrap_or(&Value::Null),
            applied: params.get("applied").unwrap_or(&Value::Null),
            created: params.get("created").filter(|v| !v.is_null()),
            material: params.get("material").filter(|v| !v.is_null()),
            discard: params.get("discard") == Some(&Value::Bool(true)),
            desc,
            key: key.clone(),
        };
        let restored = kind.restore(ctx, &self.keys, &mut self.watch, &restore)?;
        let undo = self.watch.restored(ctx, desc, &key, restored.undone_in_logic);
        Ok(json!({"epoch": ctx.project.epoch.to_string(), "restored": restored.restored, "display": restored.display, "undo": undo}))
    }

    /// `project_save {epoch, path?, version?}`: the project saved in place or as `path`; no change, no undo step.
    pub fn project_save(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        check_epoch(ctx, params)?;
        save::project_save(ctx, &self.keys, params)
    }

    /// `bounce {epoch, startBeats, endBeats, path, format?}`: the mix of a range written to an audio file through
    /// Logic's Bounce dialog; no change, no undo step.
    pub fn bounce(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        check_epoch(ctx, params)?;
        bounce::bounce(ctx, params)
    }

    /// `export_tracks {epoch, path}`: every track written as an audio file into a folder; no change, no undo step.
    pub fn export_tracks(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        check_epoch(ctx, params)?;
        bounce::export_tracks(ctx, params)
    }

    /// `transport_stop`: Logic stops playing and recording (the emergency stop).
    pub fn transport_stop(&mut self, ctx: &mut Ctx) -> Answer {
        timeline::stop(ctx)
    }

    /// `transport_play`: Logic plays from the playhead (no undo step).
    pub fn transport_play(&mut self, ctx: &mut Ctx) -> Answer {
        timeline::play(ctx)
    }

    /// `cursor_set {positionBeats}`: the playhead moves (no undo step).
    pub fn cursor_set(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        timeline::cursor_set(ctx, params)
    }

    /// `undo_step_begin {label}`: counted, since Logic has no undo block: each change stays its own step of Logic's
    /// history, and a plan is several of them.
    pub fn step_begin(&mut self, params: &Value) -> Answer {
        let label = text(params, "label")?;
        if self.open_step.is_some() {
            return Err(failed("a step is open already: end it before beginning another"));
        }
        self.steps += 1;
        let id = format!("s{}", self.steps);
        self.open_step = Some((label.to_string(), Instant::now(), 0));
        Ok(json!({"stepId": id}))
    }

    /// `undo_step_end {stepId, label?}`: the counted step closed, saying how many changes it held; Logic's history
    /// shows each by its own name.
    pub fn step_end(&mut self, params: &Value) -> Answer {
        let id = text(params, "stepId")?;
        let label = match params.get("label") {
            None | Some(Value::Null) => None,
            Some(_) => Some(text(params, "label")?.to_string()),
        };
        let Some((begun, began, changes)) = self.open_step.take() else {
            return Err(failed("that step isn't open (it ended already)"));
        };
        if id != format!("s{}", self.steps) {
            self.open_step = Some((begun, began, changes));
            return Err(failed("that step isn't open (it ended already)"));
        }
        Ok(json!({"desc": label.unwrap_or(begun), "entry": false, "changes": changes}))
    }

    /// The bridge is leaving.
    pub fn stop(&mut self) {
        self.open_step = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_named_and_the_ones_not_made_yet_say_why() {
        for name in KINDS {
            assert!(kind(name).is_some(), "{name}");
        }
        assert!(kind("track-color").is_none());
        assert!(kind("key-command").is_some());
        let refused = kind_of(&json!({"kind": "fx-move"})).unwrap_err();
        assert_eq!(refused.0, "bad-params");
        assert!(refused.1.contains("dragging"), "{}", refused.1);
        assert!(kind_of(&json!({"kind": "fx-add"})).is_ok());
        assert_eq!(kind_of(&json!({"kind": "bogus"})).unwrap_err().1, "Logic Pro has no change kind \"bogus\"");
        assert_eq!(quoted("Vocal"), "“Vocal”");
    }

    #[test]
    fn plan_steps_are_counted_and_closed_by_their_id() {
        let mut changes = Changes::default();
        let begun = changes.step_begin(&json!({"label": "Kumi: Tighten (k1234)"})).unwrap();
        assert_eq!(begun, json!({"stepId": "s1"}));
        assert_eq!(changes.step_begin(&json!({"label": "x"})).unwrap_err().0, "failed");
        assert_eq!(changes.step_end(&json!({"stepId": "s9"})).unwrap_err().0, "failed");
        let ended = changes.step_end(&json!({"stepId": "s1", "label": "Kumi: Vocal −6 dB and 1 more (k1234)"})).unwrap();
        assert_eq!(ended, json!({"desc": "Kumi: Vocal −6 dB and 1 more (k1234)", "entry": false, "changes": 0}));
        assert_eq!(changes.step_end(&json!({"stepId": "s1"})).unwrap_err().0, "failed");
        assert_eq!(changes.step_begin(&json!({"label": "again"})).unwrap()["stepId"], "s2");
    }
}
