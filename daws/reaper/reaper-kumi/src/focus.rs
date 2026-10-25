//! What the producer is looking at: the view, the selection, the transport and a counter that moves when anything in the
//! project does. Polled on the main thread each tick (the heavy part throttled by what it costs), kept for the socket
//! threads to answer `focus` from, so Kumi's ~150 ms polls never wait for the main thread.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::read::Reads;
use crate::reaper::{Fx, FxHit, FxOwner, Project, Reaper, Take, Track};
use crate::refs::{self, TrackId};
use crate::rows;

/// The most selected tracks and items named in a selection; the counts say how many there are.
const SELECTED_TRACKS: usize = 50;
const SELECTED_ITEMS: usize = 20;
/// The selection is read at least this often, and no oftener than its cost allows (about 3% of the main thread).
const MIN_SELECTION_INTERVAL: Duration = Duration::from_millis(50);
const MAX_SELECTION_INTERVAL: Duration = Duration::from_millis(500);
const SELECTION_DUTY: u32 = 30;
/// Items looked at to tell a MIDI track from an audio one.
const TYPE_SAMPLE: i32 = 5;
/// How long what `GetTouchedOrFocusedFX` named stays resolved (the FX's GUID and name, the parameter's name) while the
/// hit, the FX and REAPER's state change count stay: read again after this whatever they say.
const NAMED_FOR: Duration = Duration::from_secs(1);

#[cfg(test)]
thread_local! {
    /// A test's own bound in place of `NAMED_FOR`.
    pub static TEST_NAMED_FOR: Cell<Option<Duration>> = const { Cell::new(None) };
}

fn named_for() -> Duration {
    #[cfg(test)]
    if let Some(bound) = TEST_NAMED_FOR.with(|bound| bound.get()) {
        return bound;
    }
    NAMED_FOR
}

/// What the selection holds, with what FOCUS's header shows of it: `focus`'s `selection`, and the snapshot's.
pub fn selection(reads: &Reads, reaper: &Reaper, proj: Project) -> Value {
    let mut selection = selected_objects(reads, reaper, proj);
    add_cursor(reaper, proj, &mut selection);
    add_fx(reads, reaper, proj, &mut selection, true);
    selection
}

fn add_cursor(reaper: &Reaper, proj: Project, selection: &mut Value) {
    selection["cursor"] = rows::seconds(reaper.cursor(proj)).into();
    selection["timeSelection"] = rows::time_selection(reaper, proj);
}

/// What kind of track: `master`, `folder`, `midi` (an instrument, a MIDI input, or only MIDI items) or `audio`.
pub fn track_type(reaper: &Reaper, track: Track) -> &'static str {
    if rows::track_id(reaper, track) == TrackId::Master {
        return "master";
    }
    if reaper.track_value(track, "I_FOLDERDEPTH") as i32 == 1 {
        return "folder";
    }
    let input = reaper.track_value(track, "I_RECINPUT") as i32;
    if reaper.track_has_instrument(track) || (input >= 0 && input & 4096 != 0) {
        return "midi";
    }
    let items = reaper.track_item_count(track).min(TYPE_SAMPLE);
    let midi = (0..items)
        .filter_map(|i| reaper.track_item(track, i))
        .filter(|&item| reaper.active_take(item).is_some_and(|t| reaper.take_is_midi(t)))
        .count();
    if items > 0 && midi as i32 == items {
        "midi"
    } else {
        "audio"
    }
}

/// The selected tracks and items, which cost the most to read.
fn selected_objects(reads: &Reads, reaper: &Reaper, proj: Project) -> Value {
    let (track_total, tracks) = reaper.selected_tracks_up_to(proj, SELECTED_TRACKS);
    let track_refs: Vec<String> = tracks.iter().map(|&t| refs::track_ref(&reads.registry().track_id(reaper, proj, t))).collect();
    let header = tracks.first().map(|&t| {
        let color = reaper.track_color(t).and_then(|c| reaper.color_hex(c));
        json!({"name": rows::clip(&reaper.track_name(t)), "color": color, "type": track_type(reaper, t)})
    });
    let (item_total, items) = reaper.selected_items_up_to(proj, SELECTED_ITEMS);
    let item_guids: Vec<String> = items
        .iter()
        .map(|&item| {
            let guid = reaper.item_guid(item);
            reads.registry().note_item(proj, &guid, item);
            guid
        })
        .collect();
    let take = items.first().and_then(|&item| reaper.active_take(item));
    json!({
        "trackRef": track_refs.first(),
        "trackRefs": track_refs,
        "trackCount": track_total,
        "track": header,
        "itemRef": item_guids.first().map(|g| refs::item_ref(g)),
        "itemRefs": item_guids.iter().map(|g| refs::item_ref(g)).collect::<Vec<_>>(),
        "itemCount": item_total,
        "itemName": take.map(|t| rows::clip(&reaper.take_name(t))),
        "takeRef": take.map(|t| refs::take_ref(&reaper.take_guid(t))),
    })
}

// (r-tracks: the NIT-7 watch tells the producer's touched parameter from a meter by it)
pub(crate) fn fx_of_hit(reaper: &Reaper, proj: Project, hit: &FxHit) -> Option<Fx> {
    let track = if hit.track < 0 { reaper.master_track(proj)? } else { reaper.track_at(proj, hit.track)? };
    let owner = if hit.item >= 0 {
        let item = reaper.track_item(track, hit.item)?;
        FxOwner::Take(reaper.take_at(item, hit.take)?)
    } else {
        FxOwner::Track(track)
    };
    Some(Fx { owner, index: hit.fx })
}

/// What a hit of `GetTouchedOrFocusedFX` names, resolved once: the FX's GUID, and its name (the focused FX) or the
/// parameter's (the touched parameter), with the take's ref when the FX is a take's. Good while the hit, the FX it
/// resolves to (REAPER's own pointers, asked each poll) and REAPER's state change count stay (a rename, an FX moved or
/// deleted, a track reordered are undo points and move the count), and for `NAMED_FOR` at most: a bare API rename of
/// another extension's moves nothing, and shows within it. The poll ran twenty times a second and read these afresh
/// each time: three string reads and a GUID per hit, all the same as the time before.
struct Named {
    hit: FxHit,
    fx: Fx,
    changed: Option<i32>,
    at: Instant,
    guid: String,
    name: String,
    chain: Option<String>,
}

/// What a hit names when resolved: its GUID, its name and its chain's ref.
type Resolved = (String, String, Option<String>);

/// The FX with the focus (`GetTouchedOrFocusedFX` mode 1), as the hit names it: nothing when the window is open but
/// something else has the focus (parm 1).
fn focused_hit(reads: &Reads, reaper: &Reaper, proj: Project) -> Option<(FxHit, Fx)> {
    let hit = reads.touched_fx(reaper, 1)?;
    // Parm 1 says the window is still open but something else has the focus.
    if hit.param & 1 != 0 {
        return None;
    }
    let fx = fx_of_hit(reaper, proj, &hit)?;
    Some((hit, fx))
}

/// The focused FX's GUID, name and chain, read from REAPER.
fn resolve_focused(reads: &Reads, reaper: &Reaper, proj: Project, fx: Fx) -> Option<Resolved> {
    let guid = reaper.fx_guid(fx);
    if guid.is_empty() {
        return None;
    }
    reads.registry().note_fx(proj, &guid, fx);
    let chain = match fx.owner {
        FxOwner::Take(take) => Some(refs::take_ref(&reaper.take_guid(take))),
        FxOwner::Track(_) => None,
    };
    Some((guid, rows::clip(&reaper.fx_name(fx)), chain))
}

/// The touched parameter's FX GUID and the parameter's name, read from REAPER; nothing for a parameter the FX hasn't.
fn resolve_touched(reads: &Reads, reaper: &Reaper, proj: Project, hit: &FxHit, fx: Fx) -> Option<Resolved> {
    let guid = reaper.fx_guid(fx);
    if guid.is_empty() || hit.param < 0 || hit.param >= reaper.fx_param_count(fx) {
        return None;
    }
    reads.registry().note_fx(proj, &guid, fx);
    Some((guid, rows::clip(&reaper.param_name(fx, hit.param)), None))
}

/// The track a floating routing and I/O window is about, when that window (titled `title`) has the keyboard focus:
/// REAPER titles it `Routing for track 2: Lead` (View > Routing and I/O for a track). None when it isn't the window in
/// front. (REAPER in another language titles it in that language, and then the view stays `arrange`.)
fn routing_window(reaper: &Reaper, proj: Project, title: &str) -> Option<Option<String>> {
    let rest = title.strip_prefix("Routing for track ")?;
    let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let track = number.parse::<i32>().ok().filter(|&n| n >= 1).and_then(|n| reaper.track_at(proj, n - 1));
    Some(track.map(|track| refs::track_ref(&rows::track_id(reaper, track))))
}

/// Whether the window with the keyboard focus (titled `title`) is REAPER's mixer: undocked, REAPER titles it "Mixer". A
/// mixer docked in the main window has no window of its own to tell by, and the view stays `arrange` then.
fn mixer_window(title: &str) -> bool {
    title == "Mixer" || title.starts_with("Mixer ")
}

/// The touched parameter's header: `{name, value, formatted, fxRef}`, the value and its text read now.
fn param_json(reaper: &Reaper, hit: &FxHit, fx: Fx, named: &Resolved) -> Value {
    json!({
        "name": named.1,
        "value": reaper.param_normalized(fx, hit.param),
        "formatted": rows::clip(&reaper.param_formatted(fx, hit.param)),
        "fxRef": refs::fx_ref(&named.0),
    })
}

fn add_focused(selection: &mut Value, named: Option<&Resolved>) {
    selection["fxRef"] = Value::Null;
    if let Some((guid, name, chain)) = named {
        selection["fxRef"] = refs::fx_ref(guid).into();
        selection["fxName"] = name.clone().into();
        if let Some(chain) = chain {
            selection["fxChainRef"] = chain.clone().into();
        }
    }
}

/// `fxRef`, `fxName` and `param`, read from REAPER (the snapshot's selection: once a look, not kept). The focused FX is
/// named only when `focused` holds (the FX window is the view).
fn add_fx(reads: &Reads, reaper: &Reaper, proj: Project, selection: &mut Value, focused: bool) {
    let named = focused.then(|| focused_hit(reads, reaper, proj)).flatten().and_then(|(_, fx)| resolve_focused(reads, reaper, proj, fx));
    add_focused(selection, named.as_ref());
    if let Some((hit, fx)) = reads.touched_fx(reaper, 0).and_then(|hit| fx_of_hit(reaper, proj, &hit).map(|fx| (hit, fx))) {
        if let Some(named) = resolve_touched(reads, reaper, proj, &hit, fx) {
            selection["param"] = param_json(reaper, &hit, fx, &named);
        }
    }
}

/// The take and item a MIDI editor is on.
fn add_midi_take(reads: &Reads, reaper: &Reaper, proj: Project, selection: &mut Value, take: Take) {
    selection["takeRef"] = refs::take_ref(&reaper.take_guid(take)).into();
    selection["itemName"] = rows::clip(&reaper.take_name(take)).into();
    if let Some(item) = reaper.take_item(take) {
        let guid = reaper.item_guid(item);
        reads.registry().note_item(proj, &guid, item);
        selection["itemRef"] = refs::item_ref(&guid).into();
    }
}

/// Polls the project for `focus`.
pub struct Focus {
    objects: RefCell<Option<(Instant, Value)>>,
    interval: Cell<Duration>,
    /// What the focused FX's hit and the touched parameter's hit last resolved to.
    focused: RefCell<Option<Named>>,
    touched: RefCell<Option<Named>>,
}

impl Default for Focus {
    fn default() -> Self {
        Focus {
            objects: RefCell::new(None),
            interval: Cell::new(MIN_SELECTION_INTERVAL),
            focused: RefCell::new(None),
            touched: RefCell::new(None),
        }
    }
}

impl Focus {
    /// Forgets the cached selection and what the hits resolved to (a tab switch).
    pub fn reset(&self) {
        *self.objects.borrow_mut() = None;
        *self.focused.borrow_mut() = None;
        *self.touched.borrow_mut() = None;
    }

    /// What a hit names, from `slot` while the hit, the FX and the count stay and `NAMED_FOR` hasn't passed, else from
    /// `resolve` (kept for the next poll).
    fn named(
        slot: &RefCell<Option<Named>>,
        hit: FxHit,
        fx: Fx,
        changed: Option<i32>,
        resolve: impl FnOnce() -> Option<Resolved>,
    ) -> Option<Resolved> {
        if let Some(named) = slot.borrow().as_ref() {
            if named.hit == hit && named.fx == fx && named.changed == changed && named.at.elapsed() < named_for() {
                return Some((named.guid.clone(), named.name.clone(), named.chain.clone()));
            }
        }
        let resolved = resolve();
        *slot.borrow_mut() = resolved.as_ref().map(|(guid, name, chain)| Named {
            hit,
            fx,
            changed,
            at: Instant::now(),
            guid: guid.clone(),
            name: name.clone(),
            chain: chain.clone(),
        });
        resolved
    }

    /// `{epoch, view, selection, transport, changed}`. `force` reads the selection now, whatever its throttle says.
    pub fn poll(&self, reads: &Reads, reaper: &Reaper, proj: Project, force: bool) -> Value {
        let changed = reaper.state_change_count(proj);
        let midi = reaper.focused_midi_take();
        let midi_focused = midi.is_some() || reaper.midi_editor_focused();
        let focused_fx = if midi_focused {
            None
        } else {
            focused_hit(reads, reaper, proj)
                .and_then(|(hit, fx)| Self::named(&self.focused, hit, fx, changed, || resolve_focused(reads, reaper, proj, fx)))
        };
        // The window with the keyboard focus is asked for once a poll, and not at all when an FX window or a MIDI editor
        // has it (on macOS the title is four messages to AppKit and a copy).
        let title = if midi_focused || focused_fx.is_some() { None } else { crate::window::front_title() };
        let routing = title.as_deref().and_then(|title| routing_window(reaper, proj, title));
        let view = if midi_focused {
            "midi"
        } else if focused_fx.is_some() {
            "fx"
        } else if routing.is_some() {
            "routing"
        } else if title.as_deref().is_some_and(mixer_window) {
            "mixer"
        } else {
            "arrange"
        };
        let now = Instant::now();
        let due = force || self.objects.borrow().as_ref().is_none_or(|(at, _)| now.duration_since(*at) >= self.interval.get());
        if due {
            let objects = selected_objects(reads, reaper, proj);
            let cost = now.elapsed();
            self.interval.set((cost * SELECTION_DUTY).clamp(MIN_SELECTION_INTERVAL, MAX_SELECTION_INTERVAL));
            *self.objects.borrow_mut() = Some((now, objects));
        }
        let mut selection = self.objects.borrow().as_ref().map(|(_, v)| v.clone()).unwrap_or_else(|| json!({}));
        add_cursor(reaper, proj, &mut selection);
        // The FX window is named when it is the view; the touched parameter is always, its value and text read now, its
        // name and FX as they were resolved.
        add_focused(&mut selection, focused_fx.as_ref());
        if let Some((hit, fx)) = reads.touched_fx(reaper, 0).and_then(|hit| fx_of_hit(reaper, proj, &hit).map(|fx| (hit, fx))) {
            if let Some(named) = Self::named(&self.touched, hit, fx, changed, || resolve_touched(reads, reaper, proj, &hit, fx)) {
                selection["param"] = param_json(reaper, &hit, fx, &named);
            }
        }
        // The routing window's own track, which is not always the selected one.
        selection["routingTrackRef"] = routing.flatten().into();
        if let Some(take) = midi {
            add_midi_take(reads, reaper, proj, &mut selection, take);
        }
        json!({
            "epoch": reads.epoch(),
            "view": view,
            "selection": selection,
            "transport": rows::transport(reaper, proj),
            "changed": changed.unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake::{self, FakeFx, FakeItem, FakeTake, FakeTrack};

    fn project() -> (Reads, Project) {
        fake::reset();
        fake::with(|s| {
            s.tracks[0].fx.push(FakeFx::new("{FX-1}", "VST3: ReaEQ (Cockos)"));
            s.tracks[1].items.push(FakeItem::new("{ITEM-1}", 4.0, 2.0, vec![FakeTake::midi("{TAKE-1}", "bass.mid")]));
            s.tracks[1].items.push(FakeItem::new("{ITEM-2}", 8.0, 2.0, vec![FakeTake::wave("{TAKE-2}", "kick.wav")]));
            s.state_count = 7;
            s.cursor = 3.5;
        });
        let proj = Reaper::new(&fake::api()).current_project().unwrap().ptr;
        (Reads::default(), proj)
    }

    #[test]
    fn the_arrange_view_follows_the_selection_cursor_and_state_counter() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::select_tracks(&[1]);
        fake::with(|s| s.tracks[1].items[0].selected = true);
        let focus = Focus::default();
        let value = focus.poll(&reads, &reaper, proj, true);
        assert_eq!(value["view"], "arrange");
        assert_eq!(value["changed"], 7);
        assert_eq!(value["epoch"], reads.epoch());
        let selection = &value["selection"];
        assert_eq!(
            (&selection["trackRef"], &selection["trackRefs"], &selection["trackCount"]),
            (&json!("r:track:{BBBB}"), &json!(["r:track:{BBBB}"]), &json!(1))
        );
        assert_eq!(
            (&selection["itemRef"], &selection["itemName"], &selection["takeRef"]),
            (&json!("r:item:{ITEM-1}"), &json!("bass.mid"), &json!("r:take:{TAKE-1}"))
        );
        assert_eq!(selection["track"], json!({"name": "Bass", "color": Value::Null, "type": "audio"}));
        assert_eq!(selection["cursor"], 3.5);
        assert_eq!(value["selection"]["timeSelection"], Value::Null);
        assert_eq!(value["transport"]["state"], "stopped");
    }

    #[test]
    fn a_midi_editor_with_the_focus_is_the_view_and_names_its_take() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::with(|s| s.midi_editor = Some((1, 0, 0)));
        let value = Focus::default().poll(&reads, &reaper, proj, true);
        assert_eq!(value["view"], "midi");
        assert_eq!(value["selection"]["takeRef"], "r:take:{TAKE-1}");
        assert_eq!(value["selection"]["itemRef"], "r:item:{ITEM-1}");
        assert_eq!(value["selection"]["itemName"], "bass.mid");
    }

    #[test]
    fn reapers_undocked_mixer_with_the_focus_is_the_view() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::with(|s| s.front_title = Some("Mixer".into()));
        assert_eq!(Focus::default().poll(&reads, &reaper, proj, true)["view"], "mixer");
        fake::with(|s| s.front_title = Some("Mixer (no focus)".into()));
        assert_eq!(Focus::default().poll(&reads, &reaper, proj, true)["view"], "mixer");
        fake::with(|s| s.front_title = Some("Mixers of the world".into()));
        assert_eq!(Focus::default().poll(&reads, &reaper, proj, true)["view"], "arrange");
    }

    #[test]
    fn a_takes_focused_fx_names_the_takes_chain_for_focus_to_draw() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::with(|s| s.tracks[1].items[1].takes[0].fx.push(FakeFx::new("{T-FX}", "VST: ReaDelay (Cockos)")));
        fake::with(|s| s.focused_fx = Some((1, 1, 0, 0, 0)));
        let value = Focus::default().poll(&reads, &reaper, proj, true);
        assert_eq!(value["view"], "fx");
        assert_eq!((&value["selection"]["fxRef"], &value["selection"]["fxChainRef"]), (&json!("r:fx:{T-FX}"), &json!("r:take:{TAKE-2}")));
        // A track's FX names no chain: the track's is FOCUS's.
        fake::with(|s| s.focused_fx = Some((0, -1, -1, 0, 0)));
        assert_eq!(Focus::default().poll(&reads, &reaper, proj, true)["selection"].get("fxChainRef"), None);
    }

    #[test]
    fn a_focused_fx_window_is_the_view_unless_it_is_open_but_no_longer_focused() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::with(|s| s.focused_fx = Some((0, -1, -1, 0, 0)));
        let value = Focus::default().poll(&reads, &reaper, proj, true);
        assert_eq!(value["view"], "fx");
        assert_eq!((&value["selection"]["fxRef"], &value["selection"]["fxName"]), (&json!("r:fx:{FX-1}"), &json!("VST3: ReaEQ (Cockos)")));
        // Parm 1: the window is still open, but something else has the focus.
        fake::with(|s| s.focused_fx = Some((0, -1, -1, 0, 1)));
        let value = Focus::default().poll(&reads, &reaper, proj, true);
        assert_eq!(value["view"], "arrange");
        assert_eq!(value["selection"]["fxRef"], Value::Null);
    }

    #[test]
    fn a_tracks_routing_window_with_the_keyboard_focus_is_the_view_and_names_its_track() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::select_tracks(&[0]);
        // REAPER titles the window with the track's number and name.
        fake::with(|s| s.front_title = Some("Routing for track 2: Lead \"vox\" é".into()));
        let value = Focus::default().poll(&reads, &reaper, proj, true);
        assert_eq!(value["view"], "routing");
        assert_eq!(value["selection"]["routingTrackRef"], "r:track:{BBBB}");
        // The selection is still the selection.
        assert_eq!(value["selection"]["trackRef"], "r:track:{AAAA}");
        // No name, or a track that isn't there any more: still the routing view, with no track to name.
        fake::with(|s| s.front_title = Some("Routing for track 1".into()));
        assert_eq!(Focus::default().poll(&reads, &reaper, proj, true)["selection"]["routingTrackRef"], "r:track:{AAAA}");
        fake::with(|s| s.front_title = Some("Routing for track 40: Gone".into()));
        let value = Focus::default().poll(&reads, &reaper, proj, true);
        assert_eq!((&value["view"], &value["selection"]["routingTrackRef"]), (&json!("routing"), &Value::Null));
        // The main window in front, another window, or a platform that can't say: the arrange view.
        // (the mixer's own window is its own view: see reapers_undocked_mixer_with_the_focus_is_the_view)
        for title in [Some("[unsaved project] - REAPER v7.82"), Some("Routing Matrix"), Some("Track Manager"), None] {
            fake::with(|s| s.front_title = title.map(String::from));
            let value = Focus::default().poll(&reads, &reaper, proj, true);
            assert_eq!((&value["view"], &value["selection"]["routingTrackRef"]), (&json!("arrange"), &Value::Null), "{title:?}");
        }
        // A MIDI editor or an FX window with the focus wins: it is the one in front.
        fake::with(|s| {
            s.front_title = Some("Routing for track 1".into());
            s.focused_fx = Some((0, -1, -1, 0, 0));
        });
        assert_eq!(Focus::default().poll(&reads, &reaper, proj, true)["view"], "fx");
        fake::with(|s| {
            s.focused_fx = None;
            s.midi_editor = Some((1, 0, 0));
        });
        assert_eq!(Focus::default().poll(&reads, &reaper, proj, true)["view"], "midi");
    }

    // The optimization audit's R8 (2026-10-24): the poll ran twenty times a second and read the focused FX's GUID, name
    // and take, and the touched parameter's FX GUID, count and name, afresh each time, though they can't change while the
    // hit, the FX and REAPER's state change count stay. Resolved once now; the parameter's value and text are read every
    // poll.
    #[test]
    fn what_the_hits_name_is_resolved_once_while_the_hit_the_fx_and_the_count_stay() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::with(|s| {
            s.tracks[0].fx[0].params = vec![crate::reaper::fake::FakeParam::new("Gain", 0.75, "-3.0 dB")];
            s.touched_fx = Some((0, -1, -1, 0, 0));
            s.focused_fx = Some((0, -1, -1, 0, 0));
        });
        let focus = Focus::default();
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        let first = focus.poll(&reads, &reaper, proj, true);
        let fresh = calls() - before;
        assert_eq!(first["view"], "fx");
        assert_eq!(first["selection"]["fxName"], "VST3: ReaEQ (Cockos)");
        assert_eq!(first["selection"]["param"], json!({"name": "Gain", "value": 0.75, "formatted": "-3.0 dB", "fxRef": "r:fx:{FX-1}"}));
        // The same hits again: the GUID, the names and the chain stand; the parameter's value and text are read now.
        fake::with(|s| {
            s.tracks[0].fx[0].params[0].value = 0.5;
            s.tracks[0].fx[0].params[0].shown = "-6.0 dB".into();
        });
        let before = calls();
        let again = focus.poll(&reads, &reaper, proj, true);
        let kept = calls() - before;
        // (the two hits' GUIDs and names and the parameter count: five calls and three string reads a poll, twenty polls a second)
        assert_eq!(fresh - kept, 5, "{kept} REAPER calls with the hits resolved, {fresh} fresh");
        assert_eq!(again["selection"]["param"], json!({"name": "Gain", "value": 0.5, "formatted": "-6.0 dB", "fxRef": "r:fx:{FX-1}"}));
        assert_eq!((&again["selection"]["fxRef"], &again["selection"]["fxName"]), (&json!("r:fx:{FX-1}"), &json!("VST3: ReaEQ (Cockos)")));
        // The producer renames the FX (an undo point: the count moves): the name shows at once.
        fake::with(|s| s.tracks[0].fx[0].name = "VST3: ReaEQ (Cockos) warm".into());
        fake::producer_edit("Rename FX");
        assert_eq!(focus.poll(&reads, &reaper, proj, true)["selection"]["fxName"], "VST3: ReaEQ (Cockos) warm");
        // A bare rename (a script's: no undo point, the count where it was) shows once the bound passes.
        fake::with(|s| s.tracks[0].fx[0].name = "VST3: ReaEQ (Cockos) bare".into());
        assert_eq!(focus.poll(&reads, &reaper, proj, true)["selection"]["fxName"], "VST3: ReaEQ (Cockos) warm", "kept");
        TEST_NAMED_FOR.with(|bound| bound.set(Some(Duration::ZERO)));
        assert_eq!(focus.poll(&reads, &reaper, proj, true)["selection"]["fxName"], "VST3: ReaEQ (Cockos) bare");
        TEST_NAMED_FOR.with(|bound| bound.set(None));
        // Another parameter touched, or another FX focused: resolved anew.
        fake::with(|s| {
            s.tracks[0].fx[0].params.push(crate::reaper::fake::FakeParam::new("Freq", 0.25, "250 Hz"));
            s.touched_fx = Some((0, -1, -1, 0, 1));
            s.tracks[1].items[1].takes[0].fx.push(FakeFx::new("{T-FX}", "VST: ReaDelay (Cockos)"));
            s.focused_fx = Some((1, 1, 0, 0, 0));
        });
        let moved = focus.poll(&reads, &reaper, proj, true);
        assert_eq!(moved["selection"]["param"]["name"], "Freq");
        assert_eq!((&moved["selection"]["fxRef"], &moved["selection"]["fxChainRef"]), (&json!("r:fx:{T-FX}"), &json!("r:take:{TAKE-2}")));
        // A tab switch forgets what was resolved.
        focus.reset();
        assert!(focus.focused.borrow().is_none() && focus.touched.borrow().is_none());
    }

    #[test]
    fn the_front_window_is_asked_once_a_poll_and_not_when_an_fx_window_or_midi_editor_has_the_focus() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let focus = Focus::default();
        let asked = || fake::with(|s| s.front_title_asked);
        fake::with(|s| s.front_title = Some("[unsaved project] - REAPER v7.82".into()));
        let before = asked();
        assert_eq!(focus.poll(&reads, &reaper, proj, true)["view"], "arrange");
        assert_eq!(asked() - before, 1, "once, for the routing window and the mixer both");
        fake::with(|s| s.front_title = Some("Mixer".into()));
        let before = asked();
        assert_eq!(focus.poll(&reads, &reaper, proj, true)["view"], "mixer");
        assert_eq!(asked() - before, 1);
        fake::with(|s| s.front_title = Some("Routing for track 1".into()));
        let before = asked();
        let value = focus.poll(&reads, &reaper, proj, true);
        assert_eq!((&value["view"], &value["selection"]["routingTrackRef"]), (&json!("routing"), &json!("r:track:{AAAA}")));
        assert_eq!(asked() - before, 1);
        // An FX window or a MIDI editor with the focus is the view: the window title isn't asked for.
        fake::with(|s| s.focused_fx = Some((0, -1, -1, 0, 0)));
        let before = asked();
        assert_eq!(focus.poll(&reads, &reaper, proj, true)["view"], "fx");
        assert_eq!(asked() - before, 0);
        fake::with(|s| {
            s.focused_fx = None;
            s.midi_editor = Some((1, 0, 0));
        });
        let before = asked();
        assert_eq!(focus.poll(&reads, &reaper, proj, true)["view"], "midi");
        assert_eq!(asked() - before, 0);
    }

    #[test]
    fn the_selection_is_throttled_by_its_cost_but_forced_when_the_surface_says_so() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let focus = Focus::default();
        fake::select_tracks(&[0]);
        let first = focus.poll(&reads, &reaper, proj, false);
        assert_eq!(first["selection"]["trackRef"], "r:track:{AAAA}");
        // Within the interval the cached selection stands; the cursor is read fresh.
        fake::select_tracks(&[1]);
        fake::with(|s| s.cursor = 9.0);
        let second = focus.poll(&reads, &reaper, proj, false);
        assert_eq!(second["selection"]["trackRef"], "r:track:{AAAA}");
        assert_eq!(second["selection"]["cursor"], 9.0);
        let forced = focus.poll(&reads, &reaper, proj, true);
        assert_eq!(forced["selection"]["trackRef"], "r:track:{BBBB}");
    }

    #[test]
    fn a_big_selection_is_counted_and_only_the_first_are_named() {
        let (reads, proj) = project();
        fake::with(|s| {
            for i in 0..80 {
                let mut track = FakeTrack::new(&format!("{{T{i}}}"), "t");
                track.selected = true;
                s.tracks.push(track);
            }
        });
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let value = selection(&reads, &reaper, proj);
        assert_eq!(value["trackCount"], 80);
        assert_eq!(value["trackRefs"].as_array().unwrap().len(), SELECTED_TRACKS);
    }

    #[test]
    fn the_header_names_the_track_kind_and_the_touched_parameter() {
        let (reads, proj) = project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        fake::with(|s| {
            s.tracks[0].fx[0].params = vec![crate::reaper::fake::FakeParam::new("Gain", 0.75, "-3.0 dB")];
            s.touched_fx = Some((0, -1, -1, 0, 0));
            s.tracks[0].folder = 1;
        });
        let selection = selection(&reads, &reaper, proj);
        assert_eq!(selection["param"], json!({"name": "Gain", "value": 0.75, "formatted": "-3.0 dB", "fxRef": "r:fx:{FX-1}"}));
        let kinds: Vec<&str> = {
            fake::with(|s| {
                s.tracks[1].fx.push(FakeFx { instrument: true, ..FakeFx::new("{FX-INST}", "VSTi: Synth") });
                s.tracks.push(FakeTrack::new("{AUDIO}", "Vox"));
            });
            [
                reaper.master_track(proj).unwrap(),
                reaper.track_at(proj, 0).unwrap(),
                reaper.track_at(proj, 1).unwrap(),
                reaper.track_at(proj, 2).unwrap(),
            ]
            .iter()
            .map(|&t| track_type(&reaper, t))
            .collect()
        };
        assert_eq!(kinds, ["master", "folder", "midi", "audio"]);
    }
}
