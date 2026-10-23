//! Where an FX is, in every chain REAPER has: a track's main chain, its input chain (the master's monitoring FX), a take's
//! chain, and a container's children (nested as deep as the producer nests them). An FX is always found by its GUID;
//! its address is worked out from where it is now, never kept.
//!
//! What the FX kinds share: the chain an FX is in and the GUIDs in it, the address of a place in a chain (REAPER's
//! container addresses are mixed radix: see `slot_address`), an FX's entry in its owner's chunk (to keep a deleted FX and
//! make it again, or to move it into a container with its identity), FX chain files, the guard that keeps a track in
//! touch, latch or write mode from recording Kumi's writes, and the writes that make REAPER keep an undo point for what
//! it doesn't notice by itself.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{Ctx, Done, Fault};
use crate::read::{bad, failed};
use crate::reaper::{Envelope, Fx, FxOwner, Reaper, Take, Track, CONTAINER_FX, INPUT_FX};
use crate::refs::{self, Ref};
use crate::rows;

pub(super) fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

pub(super) fn gone(message: impl Into<String>) -> Fault {
    ("gone".into(), message.into())
}

/// A lookup that found nothing, as an apply sees it: the object was there for the preview.
pub(super) fn went(fault: Fault) -> Fault {
    if fault.0 == "failed" {
        changed(format!("{}: it went since the preview", fault.1))
    } else {
        fault
    }
}

/// A lookup that found nothing, as a restore sees it.
pub(super) fn gone_if_missing(fault: Fault) -> Fault {
    if fault.0 == "failed" {
        gone(fault.1)
    } else {
        fault
    }
}

/// What holds a chain of FX.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Holder {
    /// A track's main chain, or its input chain.
    Track {
        track: Track,
        input: bool,
    },
    Take(Take),
    /// A container's children: the container as REAPER addresses it now.
    Container(Fx),
}

impl Holder {
    /// The track or take at the top of the chain.
    pub fn owner(self) -> FxOwner {
        match self {
            Holder::Track { track, .. } => FxOwner::Track(track),
            Holder::Take(take) => FxOwner::Take(take),
            Holder::Container(fx) => fx.owner,
        }
    }

    /// The track a write here can be recorded on (a take's item's track).
    pub fn track(self, reaper: &Reaper) -> Option<Track> {
        match self.owner() {
            FxOwner::Track(track) => Some(track),
            FxOwner::Take(take) => reaper.take_item(take).and_then(|item| reaper.item_track(item)),
        }
    }
}

/// The FX in a chain, in order.
pub(crate) fn members(reaper: &Reaper, holder: Holder) -> Vec<Fx> {
    match holder {
        Holder::Track { track, input: false } => {
            let owner = FxOwner::Track(track);
            (0..reaper.fx_count(owner)).map(|index| Fx { owner, index }).collect()
        }
        Holder::Track { track, input: true } => {
            let owner = FxOwner::Track(track);
            (0..reaper.input_fx_count(track)).map(|index| Fx { owner, index: INPUT_FX | index }).collect()
        }
        Holder::Take(take) => {
            let owner = FxOwner::Take(take);
            (0..reaper.fx_count(owner)).map(|index| Fx { owner, index }).collect()
        }
        Holder::Container(fx) => reaper.fx_children(fx),
    }
}

pub(crate) fn guids(reaper: &Reaper, holder: Holder) -> Vec<String> {
    members(reaper, holder).into_iter().map(|fx| reaper.fx_guid(fx)).collect()
}

pub(crate) fn position_of(chain: &[String], guid: &str) -> Option<usize> {
    chain.iter().position(|g| g.eq_ignore_ascii_case(guid))
}

/// Whether an FX is a container.
pub(crate) fn is_container(reaper: &Reaper, fx: Fx) -> bool {
    reaper.fx_config(fx, "container_count").is_some()
}

/// The chain an FX is in.
pub(crate) fn holder_of(reaper: &Reaper, fx: Fx) -> Holder {
    if fx.index & CONTAINER_FX != 0 {
        if let Some(parent) = reaper.fx_config(fx, "parent_container").and_then(|p| p.trim().parse::<i32>().ok()) {
            return Holder::Container(Fx { owner: fx.owner, index: parent });
        }
    }
    match fx.owner {
        FxOwner::Track(track) => Holder::Track { track, input: fx.index & INPUT_FX != 0 },
        FxOwner::Take(take) => Holder::Take(take),
    }
}

/// The places that lead from the top of a chain to a container's child: the container's own path, top level first.
fn path_of(reaper: &Reaper, fx: Fx) -> Option<Vec<usize>> {
    if fx.index & CONTAINER_FX == 0 {
        return Some(vec![(fx.index & !INPUT_FX) as usize]);
    }
    let parent = match holder_of(reaper, fx) {
        Holder::Container(parent) => parent,
        _ => return None,
    };
    let place = reaper.fx_children(parent).iter().position(|child| *child == fx)?;
    let mut path = path_of(reaper, parent)?;
    path.push(place);
    Some(path)
}

/// The address of `slot` (from 0) among a container's children: what an FX put there gets, so an insert and a move name
/// it. REAPER's addresses are mixed radix over the chain: a child at path `[i0, i1, …]` is
/// `0x2000000 + (i0+1) + (i1+1)·(c0+1) + (i2+1)·(c0+1)·(c1+1) …`, `c0` the top chain's count and `c1` the count of the
/// container at `i0` (measured to depth 2 in 7.82, `container_item.N` reading the same numbers). Not for a container in
/// an input chain, whose addresses aren't known.
pub(crate) fn slot_address(reaper: &Reaper, container: Fx, slot: usize) -> Option<i32> {
    if container.index & INPUT_FX != 0 {
        return None;
    }
    let path = path_of(reaper, container)?;
    // The count of the chain at each level the address passes: the top chain's, then each container's on the way down
    // to the one the slot is in (whose own count the address doesn't need).
    let mut counts = vec![reaper.fx_count(container.owner) as i64];
    let mut node = Fx { owner: container.owner, index: path[0] as i32 };
    for &place in &path[1..] {
        let children = reaper.fx_children(node);
        counts.push(children.len() as i64);
        node = *children.get(place)?;
    }
    if node != container {
        return None;
    }
    let (mut address, mut stride) = (0i64, 1i64);
    for (level, place) in path.iter().copied().chain([slot]).enumerate() {
        address += (place as i64 + 1) * stride;
        stride *= counts.get(level).copied().unwrap_or(0) + 1;
    }
    i32::try_from(CONTAINER_FX as i64 + address).ok()
}

/// Where an FX goes when it is added at `place` of a chain: the place REAPER's add takes (`-1000 - place`), and whether
/// it is the input chain's.
pub(crate) fn add_place(reaper: &Reaper, holder: Holder, place: usize) -> Done<(i32, bool)> {
    match holder {
        Holder::Track { input, .. } => Ok((place as i32, input)),
        Holder::Take(_) => Ok((place as i32, false)),
        Holder::Container(container) if container.index & INPUT_FX != 0 => {
            Err(bad("Kumi doesn't put FX into a container inside an input chain: REAPER's addresses there aren't known"))
        }
        Holder::Container(container) => slot_address(reaper, container, place)
            .map(|address| (address, false))
            .ok_or_else(|| failed("REAPER doesn't say where that container is")),
    }
}

/// The address an FX moved to `place` of its chain ends up at (the destination of REAPER's copy and move).
pub(crate) fn move_address(reaper: &Reaper, holder: Holder, place: usize) -> Done<i32> {
    match holder {
        Holder::Track { input: false, .. } | Holder::Take(_) => Ok(place as i32),
        Holder::Track { input: true, .. } => Ok(INPUT_FX | place as i32),
        Holder::Container(container) if container.index & INPUT_FX != 0 => {
            Err(bad("Kumi doesn't move FX inside a container in an input chain: REAPER's addresses there aren't known"))
        }
        Holder::Container(container) => {
            slot_address(reaper, container, place).ok_or_else(|| failed("REAPER doesn't say where that container is"))
        }
    }
}

/// An FX found by its GUID, wherever it is, with the chain it is in.
#[derive(Clone)]
pub(crate) struct FxAt {
    pub fx: Fx,
    pub guid: String,
    pub holder: Holder,
}

/// The FX with this GUID in a chain Kumi just wrote to, noted in the registry: REAPER's state count doesn't always move
/// on an add, and a lookup that missed a moment ago would miss it again.
pub(crate) fn adopt(ctx: &Ctx, holder: Holder, guid: &str) -> Done<FxAt> {
    let fx = members(ctx.reaper, holder)
        .into_iter()
        .find(|fx| ctx.reaper.fx_guid(*fx).eq_ignore_ascii_case(guid))
        .ok_or_else(|| failed("the FX Kumi just wrote isn't in its chain"))?;
    ctx.reads.registry().note_fx(ctx.proj, guid, fx);
    Ok(FxAt { fx, guid: guid.to_string(), holder })
}

pub(crate) fn locate_guid(ctx: &Ctx, guid: &str) -> Done<FxAt> {
    let fx = ctx.reads.fx_by_guid(ctx.reaper, ctx.proj, guid)?;
    Ok(FxAt { fx, guid: guid.to_string(), holder: holder_of(ctx.reaper, fx) })
}

/// The FX a ref names: an FX ref, or a param ref's FX.
pub(crate) fn locate(ctx: &Ctx, reference: &Value) -> Done<FxAt> {
    let text = reference.as_str().ok_or_else(|| bad("ref must be an FX ref"))?;
    match Ref::parse(text).map_err(bad)? {
        Ref::Fx(guid) => locate_guid(ctx, &guid),
        other => Err(bad(format!("ref must be an FX ref, not r:{}", other.kind()))),
    }
}

/// A chain a request names: a track (`input` for its input chain), a take or an item (its active take), or a container.
pub(crate) fn holder_named(ctx: &Ctx, reference: &Value, input: bool, what: &str) -> Done<Holder> {
    let text = reference.as_str().ok_or_else(|| bad(format!("{what}: a track, take, item or container ref")))?;
    let holder = match Ref::parse(text).map_err(bad)? {
        Ref::Track(id) => Holder::Track { track: ctx.reads.track_by_id(ctx.reaper, ctx.proj, &id)?, input },
        Ref::Take(guid) => Holder::Take(ctx.reads.take_by_ref(ctx.reaper, ctx.proj, &guid)?),
        Ref::Item(guid) => {
            let item = ctx.reads.item_by_ref(ctx.reaper, ctx.proj, &guid)?;
            Holder::Take(ctx.reaper.active_take(item).ok_or_else(|| failed("that item has no take to hold FX"))?)
        }
        Ref::Fx(guid) => {
            let at = locate_guid(ctx, &guid)?;
            if !is_container(ctx.reaper, at.fx) {
                return Err(bad(format!("{what}: that FX isn't a container")));
            }
            Holder::Container(at.fx)
        }
        other => return Err(bad(format!("{what}: a track, take, item or container ref, not r:{}", other.kind()))),
    };
    if input && !matches!(holder, Holder::Track { .. }) {
        return Err(bad("input: only a track has an input chain"));
    }
    Ok(holder)
}

/// A chain as a record keeps it: by refs, so the chain is found again wherever it moved.
pub(crate) fn holder_value(ctx: &Ctx, holder: Holder) -> Value {
    match holder {
        Holder::Track { track, input } => {
            json!({"trackRef": refs::track_ref(&rows::track_id(ctx.reaper, track)), "input": input})
        }
        Holder::Take(take) => json!({"takeRef": refs::take_ref(&ctx.reaper.take_guid(take))}),
        Holder::Container(fx) => json!({"containerRef": refs::fx_ref(&ctx.reaper.fx_guid(fx))}),
    }
}

/// The chain a record names, now; one that went is `gone`.
pub(crate) fn holder_from(ctx: &Ctx, value: &Value) -> Done<Holder> {
    if let Some(track) = value.get("trackRef").and_then(Value::as_str) {
        let input = value["input"] == true;
        return holder_named(ctx, &json!(track), input, "the chain's track").map_err(gone_if_missing);
    }
    if let Some(take) = value.get("takeRef").and_then(Value::as_str) {
        return holder_named(ctx, &json!(take), false, "the chain's take").map_err(gone_if_missing);
    }
    if let Some(container) = value.get("containerRef").and_then(Value::as_str) {
        return holder_named(ctx, &json!(container), false, "the chain's container").map_err(gone_if_missing);
    }
    Err(bad("the chain: trackRef, takeRef or containerRef"))
}

/// A chain as the producer says it, inside a title's sentence: “Bass”, the master, “Bass”'s input FX, take “vox.wav”,
/// “Parallel” on “Bass”.
pub(crate) fn holder_label(ctx: &Ctx, holder: Holder) -> String {
    match holder {
        Holder::Track { track, input } => {
            let master = rows::track_id(ctx.reaper, track) == crate::refs::TrackId::Master;
            match (master, input) {
                (true, false) => "the master".into(),
                (true, true) => "the master's monitoring FX".into(),
                (false, false) => super::quoted(&super::track_name(ctx.reaper, track)),
                (false, true) => format!("{}'s input FX", super::quoted(&super::track_name(ctx.reaper, track))),
            }
        }
        Holder::Take(take) => {
            let name = rows::clip(&ctx.reaper.take_name(take));
            if name.is_empty() {
                "a take".into()
            } else {
                format!("take {}", super::quoted(&name))
            }
        }
        Holder::Container(container) => {
            format!("{} on {}", super::quoted(&super::fx::fx_label(ctx, container)), holder_label(ctx, holder_of(ctx.reaper, container)))
        }
    }
}

/// A chain's ref as a target names it.
pub(crate) fn holder_target(ctx: &Ctx, holder: Holder) -> Value {
    match holder {
        Holder::Track { track, input } => {
            let mut target = json!({"ref": refs::track_ref(&rows::track_id(ctx.reaper, track)), "kind": "track"});
            if input {
                target["input"] = true.into();
            }
            target
        }
        Holder::Take(take) => json!({"ref": refs::take_ref(&ctx.reaper.take_guid(take)), "kind": "take"}),
        Holder::Container(fx) => json!({"ref": refs::fx_ref(&ctx.reaper.fx_guid(fx)), "kind": "fx"}),
    }
}

// ----- chunk entries -----

/// The chunk an FX's entry is in: its track's (main, input and containers) or its take's item's.
pub(crate) fn owner_chunk(reaper: &Reaper, owner: FxOwner) -> Done<String> {
    match owner {
        FxOwner::Track(track) => reaper.track_chunk(track).ok_or_else(|| failed("REAPER wouldn't give the track's state")),
        FxOwner::Take(take) => {
            let item = reaper.take_item(take).ok_or_else(|| failed("REAPER doesn't say which item the take is on"))?;
            reaper.item_chunk(item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))
        }
    }
}

/// The blocks of a chunk that hold FX entries: a track's main and input chains (not its items' take FX), or an item's
/// takes' chains.
fn chain_blocks(take: bool) -> &'static [&'static str] {
    if take {
        &["<TAKEFX"]
    } else {
        &["<FXCHAIN", "<FXCHAIN_REC"]
    }
}

/// An FX's entry in its owner's chunk: from its `BYPASS` line through its `WAK` line, its container's children, its
/// modulation and parameter envelopes inside, and the `PARALLEL` line REAPER writes after it when it runs in parallel.
pub(crate) fn fx_entry(chunk: &str, take: bool, guid: &str) -> Option<String> {
    let lines: Vec<&str> = chunk.lines().collect();
    let id = format!("FXID {guid}");
    let blocks = chain_blocks(take);
    // The chain blocks, as line ranges (a block opens at its name line and closes at its matching ">").
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if blocks.iter().any(|block| lines[at] == *block || lines[at].starts_with(&format!("{block} "))) {
            let mut depth = 0;
            let start = at;
            while at < lines.len() {
                if lines[at].starts_with('<') {
                    depth += 1;
                } else if lines[at] == ">" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                at += 1;
            }
            ranges.push((start, at));
        }
        at += 1;
    }
    for (start, end) in ranges {
        let Some(found) = (start..end).find(|&i| lines[i].trim().eq_ignore_ascii_case(&id)) else { continue };
        // Back to the entry's BYPASS line, over the blocks the entry holds.
        let mut depth = 0i32;
        let mut first = None;
        for i in (start + 1..found).rev() {
            let line = lines[i];
            if line == ">" {
                depth += 1;
            } else if line.starts_with('<') {
                depth -= 1;
            }
            if depth == 0 && line.starts_with("BYPASS ") {
                first = Some(i);
                break;
            }
        }
        // Forward to its WAK line, over the blocks after the id (modulation, envelopes).
        let mut depth = 0i32;
        let mut last = None;
        for (i, line) in lines.iter().enumerate().take(end).skip(found + 1) {
            if line.starts_with('<') {
                depth += 1;
            } else if *line == ">" {
                depth -= 1;
            }
            if depth == 0 && line.starts_with("WAK ") {
                last = Some(i);
                break;
            }
        }
        let (Some(first), Some(mut last)) = (first, last) else { return None };
        if lines.get(last + 1).is_some_and(|line| line.starts_with("PARALLEL ")) {
            last += 1;
        }
        return Some(lines[first..=last].join("\n"));
    }
    None
}

/// An entry (or a chain file's text) without its FX ids, so REAPER gives what it adds ids of its own.
pub(crate) fn without_ids(text: &str) -> String {
    text.lines().filter(|line| !line.trim_start().starts_with("FXID ")).collect::<Vec<_>>().join("\n")
}

/// The FX ids an entry or a chain file holds, at any depth.
#[cfg(test)]
pub(crate) fn ids_in(text: &str) -> Vec<String> {
    text.lines().filter_map(|line| line.trim_start().strip_prefix("FXID ")).map(|id| id.trim().to_string()).collect()
}

/// An FX chain file in REAPER's FXChains folder (REAPER adds a chain only by a name in that folder, never by a path),
/// removed again when it goes out of scope.
pub(crate) struct ChainFile {
    path: PathBuf,
    pub name: String,
}

impl ChainFile {
    pub fn write(reaper: &Reaper, text: &str, tag: &str) -> Done<ChainFile> {
        let folder = Path::new(&reaper.resource_path()).join("FXChains");
        let _ = fs::create_dir_all(&folder);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let clean: String = tag.chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect();
        let name = format!("kumi-{clean}-{stamp}.RfxChain");
        let path = folder.join(&name);
        fs::write(&path, format!("{}\n", text.trim_end()))
            .map_err(|e| failed(format!("couldn't write an FX chain file into REAPER's FXChains folder: {e}")))?;
        Ok(ChainFile { path, name })
    }
}

impl Drop for ChainFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ----- what REAPER doesn't notice, and what it records -----

/// Makes REAPER keep an undo point for an FX whose change it doesn't notice by itself (a state chunk, a modulation
/// value, a container's macro taken off): a nudge of the FX's wet control, there and back (measured: a block with only
/// those writes makes no point, and REAPER's undo of a neighbouring step then loses them). The nudge is a parameter
/// write: on a track in touch, latch or write mode REAPER would record it, so it is made inside `QuietAutomation`.
pub(crate) fn mark_fx(reaper: &Reaper, fx: Fx) {
    let Some(wet) = reaper.fx_ident(fx, ":wet") else { return };
    let track = match fx.owner {
        FxOwner::Track(track) => Some(track),
        FxOwner::Take(take) => reaper.take_item(take).and_then(|item| reaper.item_track(item)),
    };
    let _quiet = QuietAutomation::around(reaper, track);
    let now = reaper.param_normalized(fx, wet);
    let nudge = if now >= 0.5 { now - 1e-6 } else { now + 1e-6 };
    reaper.fx_set_param(fx, wet, nudge);
    reaper.fx_set_param(fx, wet, now);
}

/// The same for an envelope (made by `GetFXEnvelope`, or written whole): sorting it is an edit REAPER keeps.
pub(crate) fn mark_envelope(reaper: &Reaper, envelope: Envelope) {
    reaper.env_sort(envelope, -1);
}

/// While a track is in touch, latch or write mode (or the global override is), REAPER records Kumi's parameter and
/// bypass writes into envelopes, stopped or not (lab 7.82). Around such writes the global override is read (1), which
/// records nothing, and is put back as it was after: the producer's environment is left as found.
pub(crate) struct QuietAutomation<'a, 'r> {
    reaper: &'a Reaper<'r>,
    put_back: Option<i32>,
}

impl<'a, 'r> QuietAutomation<'a, 'r> {
    pub fn around(reaper: &'a Reaper<'r>, track: Option<Track>) -> Self {
        let writes = |mode: i32| (2..=5).contains(&mode);
        let put_back = reaper.automation_override().filter(|&global| {
            writes(global) || ((global == -1 || global == 6) && track.is_some_and(|t| writes(reaper.track_value(t, "I_AUTOMODE") as i32)))
        });
        if put_back.is_some() {
            reaper.set_automation_override(1);
        }
        QuietAutomation { reaper, put_back }
    }
}

impl Drop for QuietAutomation<'_, '_> {
    fn drop(&mut self) {
        if let Some(mode) = self.put_back {
            self.reaper.set_automation_override(mode);
        }
    }
}

/// Every envelope of the parameters of an FX and of the FX inside it (a container's children, at any depth).
pub(crate) fn fx_envelopes_deep(reaper: &Reaper, at: &FxAt) -> usize {
    let mut count = fx_envelopes(reaper, at).len();
    for child in reaper.fx_children(at.fx) {
        let child = FxAt { fx: child, guid: reaper.fx_guid(child), holder: Holder::Container(at.fx) };
        count += fx_envelopes_deep(reaper, &child);
    }
    count
}

/// Every envelope of an FX's parameters: its track's, or its take's.
pub(crate) fn fx_envelopes(reaper: &Reaper, at: &FxAt) -> Vec<Envelope> {
    match at.fx.owner {
        FxOwner::Track(track) => (0..reaper.track_envelope_count(track))
            .filter_map(|i| reaper.track_envelope(track, i))
            .filter(|&envelope| reaper.envelope_owner(envelope).is_some_and(|(_, fx, _)| fx == at.fx.index))
            .collect(),
        FxOwner::Take(_) => (0..reaper.fx_param_count(at.fx)).filter_map(|p| reaper.fx_envelope(at.fx, p, false)).collect(),
    }
}
