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

/// The FX with the focus (`GetTouchedOrFocusedFX` mode 1): its ref and name, and its take's ref when it is a take's (the
/// chain FOCUS's tree shows then).
fn focused_fx(reads: &Reads, reaper: &Reaper, proj: Project) -> Option<(String, String, Option<String>)> {
    let hit = reads.touched_fx(reaper, 1)?;
    // Parm 1 says the window is still open but something else has the focus.
    if hit.param & 1 != 0 {
        return None;
    }
    let fx = fx_of_hit(reaper, proj, &hit)?;
    let guid = reaper.fx_guid(fx);
    if guid.is_empty() {
        return None;
    }
    reads.registry().note_fx(proj, &guid, fx);
    let chain = match fx.owner {
        FxOwner::Take(take) => Some(refs::take_ref(&reaper.take_guid(take))),
        FxOwner::Track(_) => None,
    };
    Some((refs::fx_ref(&guid), rows::clip(&reaper.fx_name(fx)), chain))
}

/// The track a floating routing and I/O window is about, when that window has the keyboard focus: REAPER titles it
/// `Routing for track 2: Lead` (View > Routing and I/O for a track). None when it isn't the window in front, or the
/// platform can't say. (REAPER in another language titles it in that language, and then the view stays `arrange`.)
fn routing_window(reaper: &Reaper, proj: Project) -> Option<Option<String>> {
    let title = crate::window::front_title()?;
    let rest = title.strip_prefix("Routing for track ")?;
    let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let track = number.parse::<i32>().ok().filter(|&n| n >= 1).and_then(|n| reaper.track_at(proj, n - 1));
    Some(track.map(|track| refs::track_ref(&rows::track_id(reaper, track))))
}

/// Whether REAPER's mixer window has the keyboard focus: undocked, REAPER titles it "Mixer". A mixer docked in the main
/// window has no window of its own to tell by, and the view stays `arrange` then.
fn mixer_window() -> bool {
    crate::window::front_title().is_some_and(|title| title == "Mixer" || title.starts_with("Mixer "))
}

/// The parameter touched last, named for the header: `{name, value, formatted, fxRef}`.
fn touched_param(reads: &Reads, reaper: &Reaper, proj: Project) -> Option<Value> {
    let hit = reads.touched_fx(reaper, 0)?;
    let fx = fx_of_hit(reaper, proj, &hit)?;
    let guid = reaper.fx_guid(fx);
    if guid.is_empty() || hit.param < 0 || hit.param >= reaper.fx_param_count(fx) {
        return None;
    }
    reads.registry().note_fx(proj, &guid, fx);
    Some(json!({
        "name": rows::clip(&reaper.param_name(fx, hit.param)),
        "value": reaper.param_normalized(fx, hit.param),
        "formatted": rows::clip(&reaper.param_formatted(fx, hit.param)),
        "fxRef": refs::fx_ref(&guid),
    }))
}

/// `fxRef`, `fxName` and `param`. The focused FX is named only when `focused` holds (the FX window is the view).
fn add_fx(reads: &Reads, reaper: &Reaper, proj: Project, selection: &mut Value, focused: bool) {
    selection["fxRef"] = Value::Null;
    if focused {
        if let Some((reference, name, chain)) = focused_fx(reads, reaper, proj) {
            selection["fxRef"] = reference.into();
            selection["fxName"] = name.into();
            if let Some(chain) = chain {
                selection["fxChainRef"] = chain.into();
            }
        }
    }
    if let Some(param) = touched_param(reads, reaper, proj) {
        selection["param"] = param;
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
}

impl Default for Focus {
    fn default() -> Self {
        Focus { objects: RefCell::new(None), interval: Cell::new(MIN_SELECTION_INTERVAL) }
    }
}

impl Focus {
    /// Forgets the cached selection (a tab switch).
    pub fn reset(&self) {
        *self.objects.borrow_mut() = None;
    }

    /// `{epoch, view, selection, transport, changed}`. `force` reads the selection now, whatever its throttle says.
    pub fn poll(&self, reads: &Reads, reaper: &Reaper, proj: Project, force: bool) -> Value {
        let midi = reaper.focused_midi_take();
        let midi_focused = midi.is_some() || reaper.midi_editor_focused();
        let focused_fx = if midi_focused { None } else { focused_fx(reads, reaper, proj) };
        let routing = if midi_focused || focused_fx.is_some() { None } else { routing_window(reaper, proj) };
        let view = if midi_focused {
            "midi"
        } else if focused_fx.is_some() {
            "fx"
        } else if routing.is_some() {
            "routing"
        } else if mixer_window() {
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
        // The FX window is named when it is the view; the touched parameter is always.
        selection["fxRef"] = Value::Null;
        if let Some((reference, name, chain)) = focused_fx {
            selection["fxRef"] = reference.into();
            selection["fxName"] = name.into();
            if let Some(chain) = chain {
                selection["fxChainRef"] = chain.into();
            }
        }
        if let Some(param) = touched_param(reads, reaper, proj) {
            selection["param"] = param;
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
            "changed": reaper.state_change_count(proj).unwrap_or(0),
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
