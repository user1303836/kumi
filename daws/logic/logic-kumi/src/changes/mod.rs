//! Fenced changes to Logic Pro (M3): preview, apply through one native channel, readback, and restore by value.
//! Logic exposes no transaction identity for its native Undo steps. Deleted tracks cannot be rebuilt by value,
//! and deleted markers cannot regain an arbitrary name through these channels, so those restores refuse without
//! pressing native Undo. The observer names only value-proven reversions; generic titles and header lists do not
//! establish ownership of a producer's structural, marker or note step.

mod cycle;
mod front;
mod fx;
mod marks;
mod notes;
mod nudge;
mod region_values;
mod structure;
mod timeline;
mod tracks;
pub mod undo;

use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::ax::Ax;
use crate::project::Project;
use crate::queue::Answer;
use crate::refs::{bad, failed, Fault, Ref};

pub use undo::Watch;

pub type Done<T> = Result<T, Fault>;

/// Logic exposes no transaction identity for its native Undo step. Titles cannot establish ownership.
pub const NATIVE_UNDO_REFUSAL: &str = "Logic Pro's Undo title cannot identify Kumi's specific step. Kumi won't undo a possibly unrelated producer edit; use Logic Pro's own undo manually.";

/// Whether the changes wait for Logic to settle after a write (a click, a menu item, a list's press): a replay over a
/// recorded scene has nothing to wait for, and turns the waits off.
static SETTLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_settling(on: bool) {
    SETTLING.store(on, std::sync::atomic::Ordering::Relaxed);
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
    /// A property edit such as a region rename moves the epoch: the host journal adopts its confirmed new ref.
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
    LoopRange,
    RegionRename,
    RegionLoop,
}

/// The kinds Logic's bridge can't make yet, and why, so the refusal says so.
const NOT_YET: [(&str, &str); 8] = [
    ("marker-rename", "a marker's name cell in the Marker List isn't a text field"),
    ("fx-add", "the insert slot's pop-up menu can't be driven from outside"),
    ("fx-delete", "the insert slot's pop-up menu can't be driven from outside"),
    ("fx-param", "a plug-in's controls aren't reached yet"),
    ("fx-move", "the insert slots can't be rearranged from outside"),
    ("send-add", "the send slot's pop-up menu can't be driven from outside"),
    ("send-volume", "the send slot's pop-up menu can't be driven from outside"),
    ("track-move", "the track headers can't be dragged from outside"),
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
            Kind::LoopRange => f.write_str("LoopRange"),
            Kind::RegionRename => f.write_str("RegionRename"),
            Kind::RegionLoop => f.write_str("RegionLoop"),
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
        "loop-range" => Some(Kind::LoopRange),
        "region-rename" => Some(Kind::RegionRename),
        "region-loop" => Some(Kind::RegionLoop),
        _ => None,
    }
}

/// Every kind the bridge makes, by its wire name.
pub const KINDS: [&str; 19] = [
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
    "loop-range",
    "region-rename",
    "region-loop",
];

impl Kind {
    fn preview(self, ctx: &mut Ctx, input: &Value) -> Done<Preview> {
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
            Kind::LoopRange => cycle::preview(ctx, input),
            Kind::RegionRename => region_values::preview(ctx, input, region_values::Mode::Rename),
            Kind::RegionLoop => region_values::preview(ctx, input, region_values::Mode::Loop),
        }
    }

    fn apply(self, ctx: &mut Ctx, input: &Value, fence: &Value, desc: &str) -> Done<Applied> {
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
            Kind::LoopRange => cycle::apply(ctx, input, fence),
            Kind::RegionRename => region_values::apply(ctx, input, fence, region_values::Mode::Rename),
            Kind::RegionLoop => region_values::apply(ctx, input, fence, region_values::Mode::Loop),
        }
    }

    fn restore(self, ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
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
            Kind::LoopRange => cycle::restore(ctx, restore),
            Kind::RegionRename => region_values::restore(ctx, restore, region_values::Mode::Rename),
            Kind::RegionLoop => region_values::restore(ctx, restore, region_values::Mode::Loop),
        }
    }
}

/// The change side's state across requests: the undo watch and the counted plan steps.
#[derive(Default)]
pub struct Changes {
    pub watch: Watch,
    steps: u64,
    /// A plan step left open by `undo_step_begin`: its label, when it began, and the changes applied in it so far.
    open_step: Option<(String, Instant, usize)>,
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

/// Before dispatch, a missing grant is `failed`, never busy. Once a write may have been sent, a timeout, lost reply,
/// or revoked grant is `uncertain`: a partial change must never become a known refusal that allows a resend.
pub(crate) fn ax_fault(error: &crate::ax::AxError, wrote: bool) -> Fault {
    if wrote {
        return uncertain(format!("Logic Pro didn't answer after the change was sent: {error}"));
    }
    if crate::ax::untrusted(error) {
        return failed(crate::server::UNTRUSTED);
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

impl Changes {
    /// `change_preview {kind, input, epoch}`: what the change would do, writing nothing.
    pub fn preview(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        check_epoch(ctx, params)?;
        let kind = kind_of(params)?;
        let preview = kind.preview(ctx, object(params, "input")?)?;
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
        let (input, fence, desc) = (object(params, "input")?, object(params, "fence")?, text(params, "desc")?);
        let applied = kind.apply(ctx, input, fence, desc)?;
        if let Some((_, _, count)) = self.open_step.as_mut() {
            *count += 1;
        }
        let key = change_key(text(params, "kind")?, &input["ref"]);
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

    /// `change_restore {kind, target, prior, applied, created?, material?, discard?, desc, epoch}` restores by value.
    /// Refs are carried from the change's epoch where identity is unambiguous. Deleted tracks and markers refuse:
    /// Logic does not expose enough evidence to prove ownership of a native Undo step.
    pub fn restore(&mut self, ctx: &mut Ctx, params: &Value) -> Answer {
        let kind = kind_of(params)?;
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
        let restored = kind.restore(ctx, &mut self.watch, &restore)?;
        let undo = self.watch.restored(ctx, desc, &key, restored.undone_in_logic);
        Ok(json!({"epoch": ctx.project.epoch.to_string(), "restored": restored.restored, "display": restored.display, "undo": undo}))
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

    /// A different project has no ownership relation to this project's undo steps. Step ids stay monotonic.
    pub fn new_project(&mut self) {
        self.watch = Watch::default();
        self.open_step = None;
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
        let refused = kind_of(&json!({"kind": "fx-add"})).unwrap_err();
        assert_eq!(refused.0, "bad-params");
        assert!(refused.1.contains("pop-up menu"), "{}", refused.1);
        assert_eq!(kind_of(&json!({"kind": "bogus"})).unwrap_err().1, "Logic Pro has no change kind \"bogus\"");
        assert_eq!(quoted("Vocal"), "“Vocal”");
        let hands = crate::helper::MockHands::new();
        let mut launcher = hands.clone();
        let mut helper = crate::helper::Launch::launch(&mut launcher).unwrap();
        let mut project = Project::new(crate::ax::strings::Strings::english());
        let mut ax = Ax::new(helper.as_mut(), 7);
        let mut ctx = Ctx { project: &mut project, ax: &mut ax };
        let mut changes = Changes::default();
        for epoch in ["0", "2"] {
            let request = json!({"epoch": epoch, "kind": "track-mute", "input": {"ref": "1:track:1", "mute": true}, "fence": {"mute": false}, "desc": "Kumi: mute"});
            assert_eq!(changes.preview(&mut ctx, &request).unwrap_err().0, "stale");
            assert_eq!(changes.apply(&mut ctx, &request).unwrap_err().0, "stale");
        }
        assert!(hands.calls().is_empty(), "a stale change never reaches Accessibility");
        for error in [
            crate::helper::HelperError::Timeout,
            crate::helper::HelperError::Gone("lost reply".into()),
            crate::helper::HelperError::Refused { code: "untrusted".into(), reply: json!({}) },
        ] {
            let error = crate::ax::AxError::Helper(error);
            assert_eq!(ax_fault(&error, true).0, "uncertain", "a partial write is never downgraded to a known refusal");
            assert_eq!(ax_fault(&error, false).0, "failed");
        }
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
        changes.new_project();
        assert_eq!(changes.step_end(&json!({"stepId": "s2"})).unwrap_err().0, "failed");
        assert_eq!(changes.step_begin(&json!({"label": "the new project"})).unwrap()["stepId"], "s3");
    }

    #[test]
    fn a_producers_same_titled_native_step_is_never_pressed_attributed_or_removed_from_the_watch() {
        use crate::helper::{Launch, MockHands};
        let hands = MockHands::new();
        hands.set_tree(json!({"role": "AXMenu", "c": [
            {"i": "3.0.0", "role": "AXMenuItem", "title": "Undo Delete Tracks"},
            {"i": "3.0.1", "role": "AXMenuItem", "title": "Redo Delete Tracks"}
        ]}));
        // A same-named producer step could even leave the observed header list unchanged.
        hands.reply("menu", Ok(json!({"title": "Undo Delete Tracks"})));
        let mut helper = Launch::launch(&mut hands.clone()).unwrap();
        let mut project = Project::new(crate::ax::strings::Strings::english());
        let mut ax = Ax::new(helper.as_mut(), 7);
        let mut ctx = Ctx { project: &mut project, ax: &mut ax };
        for (kind, step, probe) in [
            ("track-delete", "Delete Tracks", Some(undo::Probe::Headers { prior: vec!["Vocal".into()], applied: vec![] })),
            ("marker-delete", "Delete", None),
        ] {
            let desc = format!("Kumi: {kind} (k0001)");
            let mut changes = Changes::default();
            changes.watch.entries.push(undo::Entry {
                desc: desc.clone(),
                key: format!("{kind} 1:track:1"),
                step: Some(step.into()),
                title: format!("Undo {step}"),
                undone: false,
                probe,
                epoch: 1,
            });
            let before = hands.calls().len();
            let refused = changes
                .restore(
                    &mut ctx,
                    &json!({"kind": kind, "target": {"ref": "1:track:1"}, "prior": {},
                "applied": {"deleted": true}, "material": {}, "desc": "Kumi: put back", "epoch": "1"}),
                )
                .unwrap_err();
            assert_eq!(refused, failed(NATIVE_UNDO_REFUSAL));
            assert_eq!(hands.calls().len(), before, "refusal consumes no native Undo/menu request");
            let mut events = vec![];
            changes.watch.poll(
                ctx.ax,
                &ctx.project.strings,
                &crate::focus::Look { epoch: 1, ..Default::default() },
                &[],
                &mut |event| events.push(event),
                &|_, place| Some(place),
            );
            assert!(events.is_empty(), "generic titles and header-only evidence do not attribute a native step");
            assert_eq!(changes.watch.entries.len(), 1);
            assert!(!changes.watch.entries[0].undone);
            assert_eq!(changes.watch.top_desc().as_deref(), Some(desc.as_str()));
        }
        assert!(hands.calls().iter().all(|(op, _)| op == "tree"), "no input is sent to the producer's step");
    }
}
