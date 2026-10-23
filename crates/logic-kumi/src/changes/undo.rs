//! Noticing that the producer undid or redid a change of Kumi's with Logic's own undo. Logic's Undo History isn't
//! readable from outside, and its Edit menu's Undo and Redo titles name the top steps only once Logic has validated
//! that menu (a key command of its own validates nothing; see [`super::front`]), and carry no nonce. So the watch
//! keeps Kumi's steps in the order Logic's history holds them, each with the title Logic gave it at apply time and the
//! `desc` the host sent, and names them back when the titles move: the Redo title taking a step's name is that step
//! (and every one above it) undone, the Undo title taking it back is a redo. For what the focus tick reads every
//! 150 ms anyway (a header's name, mute, solo, arm, volume and pan, the tracks' names, the tempo) a probe tells at
//! once, whatever the titles say: a value back at what it was before Kumi's change is the change undone, back at
//! what Kumi set is a redo. The titles are refreshed (by a key, which opens nothing) after each of Kumi's own writes
//! while Logic is in front, and read as they stand otherwise: the tick puts no input into Logic on its own, and the
//! bridge presses no menu bar item (see [`super::front`] for the wedge that taught it), so a step without a probe (a
//! marker's, a note's) undone by hand while Logic is behind is named when Logic next validates its Edit menu.

use serde_json::{json, Value};

use crate::ax::strings::Strings;
use crate::ax::Ax;
use crate::changes::{front, Ctx, Done};
use crate::focus::Look;
use crate::log;
use crate::rows::Header;

/// The most steps the watch keeps: the oldest go first.
const MOST: usize = 64;

/// The Edit menu's Undo and Redo titles, raw.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Titles {
    pub undo: String,
    pub redo: String,
}

/// A title's action: what follows "Undo " or "Redo " in Logic's language; nothing for a bare "Undo" or "Can’t Redo".
pub fn action_of(title: &str, verb: &str, strings: &Strings) -> Option<String> {
    let mut words = strings.candidates(verb);
    words.push(verb.to_string());
    words.sort_by_key(|word| std::cmp::Reverse(word.len()));
    for word in words {
        if let Some(rest) = title.strip_prefix(word.as_str()) {
            let rest = rest.trim();
            if rest.is_empty() {
                return None;
            }
            // "Undo Renaming", never "Undoing": the word ends where the title's next word begins.
            if title.len() > word.len() && !title[word.len()..].starts_with(char::is_whitespace) {
                continue;
            }
            return Some(rest.to_string());
        }
    }
    None
}

impl Titles {
    pub fn undo_action(&self, strings: &Strings) -> Option<String> {
        action_of(&self.undo, "Undo", strings)
    }

    pub fn redo_action(&self, strings: &Strings) -> Option<String> {
        action_of(&self.redo, "Redo", strings)
    }
}

/// How the focus tick can see a change undone by hand.
#[derive(Debug, Clone)]
pub enum Probe {
    /// A header's attribute at a place: `read` takes it from the header, `same` compares.
    Header { place: usize, read: fn(&Header) -> Value, same: fn(&Value, &Value) -> bool, prior: Value, applied: Value },
    /// The tracks' names in order, before and after.
    Headers { prior: Vec<String>, applied: Vec<String> },
    /// The tempo the control bar shows.
    Tempo { prior: f64, applied: f64 },
    /// A track's strip, as the Inspector shows the selected track's: its plug-ins' names, or its sends' busses.
    Strip { place: usize, part: StripPart, prior: Vec<String>, applied: Vec<String> },
}

/// What of a strip a probe watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripPart {
    Plugins,
    Sends,
}

/// The selected track's strip as the Inspector showed it at a tick (read only when a probe watches that track).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub place: usize,
    pub plugins: Vec<String>,
    pub sends: Vec<String>,
}

impl Probe {
    /// Whether the change reads as applied (Some(true)), as before it (Some(false)), or as neither.
    fn state(&self, headers: &[Header], look: &Look, seen: Option<&Seen>) -> Option<bool> {
        match self {
            Probe::Strip { place, part, prior, applied } => {
                let seen = seen.filter(|seen| seen.place == *place)?;
                let now = match part {
                    StripPart::Plugins => &seen.plugins,
                    StripPart::Sends => &seen.sends,
                };
                if now == applied {
                    Some(true)
                } else if now == prior {
                    Some(false)
                } else {
                    None
                }
            }
            Probe::Header { place, read, same, prior, applied } => {
                let header = headers.get(place.checked_sub(1)?)?;
                let now = read(header);
                if same(&now, applied) {
                    Some(true)
                } else if same(&now, prior) {
                    Some(false)
                } else {
                    None
                }
            }
            Probe::Headers { prior, applied } => {
                let names: Vec<String> = headers.iter().map(|header| header.name.clone()).collect();
                if names == *applied {
                    Some(true)
                } else if names == *prior {
                    Some(false)
                } else {
                    None
                }
            }
            Probe::Tempo { prior, applied } => {
                let now = look.transport.tempo;
                if (now - applied).abs() < 0.01 {
                    Some(true)
                } else if (now - prior).abs() < 0.01 {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }

    /// A probe tied to a track's place, carried to where the track is after an epoch move; None when it went.
    fn carried(&self, carry: &dyn Fn(usize) -> Option<usize>) -> Option<Probe> {
        match self {
            Probe::Header { place, read, same, prior, applied } => {
                carry(*place).map(|place| Probe::Header { place, read: *read, same: *same, prior: prior.clone(), applied: applied.clone() })
            }
            Probe::Strip { place, part, prior, applied } => {
                carry(*place).map(|place| Probe::Strip { place, part: *part, prior: prior.clone(), applied: applied.clone() })
            }
            other => Some(other.clone()),
        }
    }
}

/// One of Kumi's steps as Logic's history holds it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub desc: String,
    /// The change's kind and target, which a restore names again (its `desc` is the restore's own).
    pub key: String,
    /// Logic's name for the step ("Renaming"), as its Undo title said after the apply; none when Logic made no step.
    pub step: Option<String>,
    /// The Undo title read after the apply.
    pub title: String,
    pub undone: bool,
    pub probe: Option<Probe>,
    /// The epoch the probe's place is from.
    pub epoch: u32,
}

#[derive(Default)]
pub struct Watch {
    pub entries: Vec<Entry>,
    seen: Option<Titles>,
    /// Whether the log was told, once, that the titles are being read unverified while Logic is behind.
    unverified_said: bool,
    edit: Option<String>,
    epoch: Option<u32>,
    /// The selected track's strip as last read for a strip probe.
    strip_seen: Option<Seen>,
}

impl Watch {
    fn edit_menu(&mut self, ax: &mut Ax, strings: &Strings) -> Done<String> {
        if let Some(edit) = &self.edit {
            return Ok(edit.clone());
        }
        let edit = super::nudge::read(front::edit_menu(ax, strings))?;
        self.edit = Some(edit.clone());
        Ok(edit)
    }

    /// The titles, validated by a key when Logic is in front (`true`), else as they stand (`false`: a key doesn't
    /// land behind, and the bridge presses no menu); kept as the baseline either way.
    pub fn fresh_titles(&mut self, ctx: &mut Ctx) -> Done<(Titles, bool)> {
        let edit = self.edit_menu(ctx.ax, &ctx.project.strings)?;
        let verified = super::nudge::read(ctx.ax.in_front())?;
        if verified {
            super::nudge::read(front::refresh(ctx.ax))?;
        } else if !self.unverified_said {
            self.unverified_said = true;
            log::line(
                "Logic Pro is behind: the Edit menu's Undo titles are read unverified (nothing is pressed or posted into a Logic \
                 behind); a marker or note step undone by hand is named when Logic next validates its Edit menu",
            );
        }
        let (undo, redo) = super::nudge::read(front::titles(ctx.ax, &edit))?;
        let titles = Titles { undo, redo };
        self.seen = Some(titles.clone());
        Ok((titles, verified))
    }

    /// Whether Kumi's latest step that Logic's history holds is the change `key` names (its kind and target): the one
    /// Logic's undo would take back first.
    pub fn top_is(&self, key: &str) -> bool {
        self.entries.iter().rev().find(|entry| entry.step.is_some() && !entry.undone).is_some_and(|entry| entry.key == key)
    }

    /// Kumi applied a change: the titles are refreshed and the step kept; the `undo` field of the apply's answer.
    pub fn applied(&mut self, ctx: &mut Ctx, desc: &str, key: &str, expected: Option<&'static str>, probe: Option<Probe>) -> Value {
        let (titles, verified) = self.fresh_titles(ctx).unwrap_or_default();
        // A title Logic hasn't validated may name an older step: unverified, the step is the kind's own Logic name.
        let step = match expected {
            Some(expected) if verified => titles.undo_action(&ctx.project.strings).or_else(|| Some(expected.to_string())),
            Some(expected) => Some(expected.to_string()),
            None => None,
        };
        let entry = step.is_some();
        if self.entries.len() >= MOST {
            self.entries.remove(0);
        }
        let epoch = ctx.project.epoch;
        self.entries.push(Entry {
            desc: desc.to_string(),
            key: key.to_string(),
            step,
            title: if verified { titles.undo.clone() } else { String::new() },
            undone: false,
            probe,
            epoch,
        });
        self.epoch = Some(epoch);
        let mut undo = json!({"desc": desc, "entry": entry});
        if entry && verified && !titles.undo.is_empty() {
            undo["title"] = titles.undo.into();
        }
        undo
    }

    /// Kumi ran a key command: the titles read again and compared with `before`. The Undo title moved, so Logic
    /// made a step of it, which the watch keeps under the title's action; else no step and nothing kept. The `undo`
    /// field of the apply's answer, and the step's name when there is one.
    pub fn applied_if_title_moved(&mut self, ctx: &mut Ctx, desc: &str, key: &str, before: &Titles) -> (Value, Option<String>) {
        let (titles, verified) = self.fresh_titles(ctx).unwrap_or_default();
        self.applied_with_titles(ctx, desc, key, before, titles, verified)
    }

    /// As `applied_if_title_moved`, over titles the caller read (a key command reads them until they move).
    pub fn applied_with_titles(
        &mut self,
        ctx: &mut Ctx,
        desc: &str,
        key: &str,
        before: &Titles,
        titles: Titles,
        verified: bool,
    ) -> (Value, Option<String>) {
        let moved = verified && titles.undo != before.undo;
        let step = if moved { titles.undo_action(&ctx.project.strings) } else { None };
        let mut undo = json!({"desc": desc, "entry": step.is_some()});
        if let Some(step) = &step {
            if self.entries.len() >= MOST {
                self.entries.remove(0);
            }
            let epoch = ctx.project.epoch;
            self.entries.push(Entry {
                desc: desc.to_string(),
                key: key.to_string(),
                step: Some(step.clone()),
                title: titles.undo.clone(),
                undone: false,
                probe: None,
                epoch,
            });
            self.epoch = Some(epoch);
            undo["title"] = titles.undo.clone().into();
        }
        (undo, step)
    }

    /// Kumi put a change back: its step is Kumi's no more (by value it is a new step of Logic's; by Logic's own undo
    /// it is gone), so the latest step the key names leaves the watch; the `undo` field of the restore's answer,
    /// with `how: "undo"` when Logic's own undo did it.
    pub fn restored(&mut self, ctx: &mut Ctx, desc: &str, key: &str, undone_in_logic: bool) -> Value {
        self.taken_back(key);
        let _ = self.fresh_titles(ctx);
        let mut undo = json!({"desc": desc, "entry": false});
        if undone_in_logic {
            undo["how"] = "undo".into();
        }
        undo
    }

    /// Whether a strip probe watches the track at `place` (the selected one, whose strip the Inspector shows).
    pub fn strip_wanted(&self, place: usize) -> bool {
        self.entries.iter().any(|entry| matches!(&entry.probe, Some(Probe::Strip { place: p, .. }) if *p == place))
    }

    /// The selected track's strip as the Inspector showed it, for the strip probes of the next poll.
    pub fn saw_strip(&mut self, seen: Seen) {
        self.strip_seen = Some(seen);
    }

    /// The `desc` of Kumi's latest step that Logic's history holds, if any.
    pub fn top_desc(&self) -> Option<String> {
        self.entries.iter().rev().find(|entry| entry.step.is_some() && !entry.undone).map(|entry| entry.desc.clone())
    }

    /// Takes the latest step a key names off the watch: Kumi put it back, by value or by Logic's own undo.
    pub fn taken_back(&mut self, key: &str) {
        if let Some(at) = self.entries.iter().rposition(|entry| entry.key == key) {
            self.entries.remove(at);
        }
    }

    /// One tick: the probes against what the focus read, and the titles as they stand against what they said; the
    /// tick posts nothing into Logic. `out` takes each `undone_in_daw` event's data; `carry` says where a track of an
    /// earlier epoch stands now, for the probes.
    pub fn poll(
        &mut self,
        ax: &mut Ax,
        strings: &Strings,
        look: &Look,
        headers: &[Header],
        out: &mut dyn FnMut(Value),
        carry: &dyn Fn(u32, usize) -> Option<usize>,
    ) {
        for entry in self.entries.iter_mut().filter(|entry| entry.epoch != look.epoch) {
            let from = entry.epoch;
            entry.probe = entry.probe.take().and_then(|probe| probe.carried(&|place| carry(from, place)));
            entry.epoch = look.epoch;
        }
        self.epoch = Some(look.epoch);
        let epoch = look.epoch.to_string();
        // The probes: a value back where it was is the change undone by hand; back where Kumi set it, a redo.
        let (mut undone, mut redone) = (Vec::new(), Vec::new());
        let seen = self.strip_seen.take();
        for entry in self.entries.iter_mut() {
            let Some(state) = entry.probe.as_ref().and_then(|probe| probe.state(headers, look, seen.as_ref())) else { continue };
            if !state && !entry.undone {
                entry.undone = true;
                undone.push(entry.desc.clone());
            } else if state && entry.undone {
                entry.undone = false;
                redone.push(entry.desc.clone());
            }
        }
        if self.entries.is_empty() {
            return;
        }
        // The titles as they stand: the tick reads them, and puts no input into Logic (nothing validates them here).
        let Ok(edit) = self.edit_menu(ax, strings) else { return };
        let Ok((undo, redo)) = front::titles(ax, &edit) else { return };
        let titles = Titles { undo, redo };
        if self.seen.as_ref() != Some(&titles) {
            let (by_titles_undone, by_titles_redone) = self.moved(&titles, strings, headers, look);
            undone.extend(by_titles_undone);
            redone.extend(by_titles_redone);
            self.seen = Some(titles);
        }
        if !undone.is_empty() {
            out(json!({"descs": undone, "how": "undo", "epoch": epoch}));
        }
        if !redone.is_empty() {
            out(json!({"descs": redone, "how": "redo", "epoch": epoch}));
        }
    }

    /// What the titles say moved: the steps undone (the Redo title names one of Kumi's, and Logic's history undoes
    /// in order, so every step above it went too) and the steps redone (the Undo title names an undone one of
    /// Kumi's, so it and every undone step below it are back). A step whose probe still shows it applied isn't
    /// taken as undone by a title alone: Logic's titles carry no nonce, and the producer's own step may share the name.
    fn moved(&mut self, titles: &Titles, strings: &Strings, headers: &[Header], look: &Look) -> (Vec<String>, Vec<String>) {
        let (mut undone, mut redone) = (Vec::new(), Vec::new());
        if let Some(action) = titles.redo_action(strings) {
            let found = self.entries.iter().rposition(|entry| !entry.undone && entry.step.as_deref() == Some(action.as_str()));
            if let Some(at) = found {
                for entry in self.entries[at..].iter_mut().filter(|entry| !entry.undone && entry.step.is_some()) {
                    if entry.probe.as_ref().and_then(|probe| probe.state(headers, look, None)) == Some(true) {
                        continue;
                    }
                    entry.undone = true;
                    undone.push(entry.desc.clone());
                }
            }
        }
        if let Some(action) = titles.undo_action(strings) {
            let found = self.entries.iter().rposition(|entry| entry.undone && entry.step.as_deref() == Some(action.as_str()));
            if let Some(at) = found {
                for entry in self.entries[..=at].iter_mut().filter(|entry| entry.undone && entry.step.is_some()) {
                    if entry.probe.as_ref().and_then(|probe| probe.state(headers, look, None)) == Some(false) {
                        continue;
                    }
                    entry.undone = false;
                    redone.push(entry.desc.clone());
                }
            }
        }
        (undone, redone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::focus::Transport;
    use crate::helper::{Launch, MockHands};

    fn header(name: &str, volume: f64) -> Header {
        Header {
            index: 1,
            name: name.into(),
            mute: false,
            solo: false,
            armed: false,
            monitoring: false,
            volume_db: Some(volume),
            pan: 0,
            selected: false,
            stack: false,
        }
    }

    fn look(epoch: u32, tempo: f64) -> Look {
        Look { epoch, view: "tracks", transport: Transport { tempo, ..Transport::default() }, ..Look::default() }
    }

    fn volume_probe(place: usize, prior: f64, applied: f64) -> Probe {
        Probe::Header {
            place,
            read: |header| json!(header.volume_db),
            same: |a, b| (a.as_f64().unwrap_or(f64::NAN) - b.as_f64().unwrap_or(f64::NAN)).abs() < 0.05,
            prior: json!(prior),
            applied: json!(applied),
        }
    }

    fn entry(desc: &str, step: Option<&str>, probe: Option<Probe>) -> Entry {
        Entry { desc: desc.into(), key: desc.into(), step: step.map(str::to_string), title: String::new(), undone: false, probe, epoch: 1 }
    }

    /// A carry for the tests: every track stays where it is.
    fn stays(_: u32, place: usize) -> Option<usize> {
        Some(place)
    }

    fn menus(hands: &MockHands, undo: &str, redo: &str) {
        hands.set_tree(json!({"role": "AXMenu", "c": [
            {"i": "3.0.0", "role": "AXMenuItem", "title": undo}, {"i": "3.0.1", "role": "AXMenuItem", "title": redo}
        ]}));
    }

    #[test]
    fn a_titles_action_is_what_follows_the_verb_in_logics_language() {
        let english = Strings::english();
        assert_eq!(action_of("Undo Renaming", "Undo", &english).as_deref(), Some("Renaming"));
        assert_eq!(action_of("Undo Delete Tracks", "Undo", &english).as_deref(), Some("Delete Tracks"));
        assert_eq!(action_of("Undo", "Undo", &english), None);
        assert_eq!(action_of("Can’t Redo", "Redo", &english), None);
        assert_eq!(action_of("Redo Renaming", "Redo", &english).as_deref(), Some("Renaming"));
        assert_eq!(action_of("Undoing", "Undo", &english), None);
        let german = Strings::with("de", &[("Undo", "Widerrufen"), ("Redo", "Wiederholen")]);
        assert_eq!(action_of("Widerrufen Umbenennen", "Undo", &german).as_deref(), Some("Umbenennen"));
        let titles = Titles { undo: "Undo Create Marker".into(), redo: "Redo Delete".into() };
        assert_eq!(
            (titles.undo_action(&english).as_deref(), titles.redo_action(&english).as_deref()),
            (Some("Create Marker"), Some("Delete"))
        );
    }

    #[test]
    fn a_probe_names_an_undo_by_hand_at_once_and_its_redo_and_goes_when_the_epoch_moves() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        menus(&hands, "Undo", "Can’t Redo");
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let strings = Strings::english();
        let mut watch = Watch::default();
        watch.entries.push(entry("Kumi: Vocal volume −6 dB (k0001)", None, Some(volume_probe(1, 0.0, -6.0))));
        watch.entries.push(entry("Kumi: Tempo 120 BPM (k0002)", None, Some(Probe::Tempo { prior: 160.0, applied: 120.0 })));
        let events = std::cell::RefCell::new(Vec::new());
        let mut out = |data: Value| events.borrow_mut().push(data);
        watch.poll(&mut ax, &strings, &look(1, 120.0), &[header("Vocal", -6.0)], &mut out, &stays);
        assert!(events.borrow().is_empty(), "as applied: {:?}", events.borrow());
        // The fader back at 0 dB: undone; the tempo back at 160 too.
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vocal", 0.0)], &mut out, &stays);
        assert_eq!(events.borrow().len(), 1);
        assert_eq!(events.borrow()[0]["how"], "undo");
        assert_eq!(events.borrow()[0]["descs"], json!(["Kumi: Vocal volume −6 dB (k0001)", "Kumi: Tempo 120 BPM (k0002)"]));
        assert_eq!(events.borrow()[0]["epoch"], "1");
        // Back at −6 dB: a redo of the volume alone.
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vocal", -6.0)], &mut out, &stays);
        let seen = events.borrow();
        assert_eq!((seen.len(), seen[1]["how"].as_str(), seen[1]["descs"].as_array().unwrap().len()), (2, Some("redo"), 1));
        drop(seen);
        // Another epoch: a track's place is carried (here it went), so its probe goes; the tempo's stays.
        watch.poll(&mut ax, &strings, &look(2, 160.0), &[header("Other", 0.0), header("Vocal", -6.0)], &mut out, &|_, _| None);
        assert_eq!(events.borrow().len(), 2);
        assert!(watch.entries[0].probe.is_none() && watch.entries[1].probe.is_some());
        // Nothing on the watch: the titles aren't even read.
        let before = hands.calls_of("tree");
        watch.entries.clear();
        watch.poll(&mut ax, &strings, &look(2, 160.0), &[], &mut |_| {}, &stays);
        assert_eq!(hands.calls_of("tree"), before);
    }

    #[test]
    fn a_strip_probe_names_a_plug_in_taken_off_by_hand_when_the_inspector_shows_its_track() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        menus(&hands, "Undo Insert Plug-in in Channel Strip", "Can’t Redo");
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let strings = Strings::english();
        let mut watch = Watch::default();
        let probe = Probe::Strip {
            place: 1,
            part: StripPart::Plugins,
            prior: vec!["Channel EQ".into()],
            applied: vec!["Channel EQ".into(), "Gain".into()],
        };
        watch.entries.push(entry("Kumi: Added “Gain” to “Vocal” (k0001)", Some("Insert Plug-in in Channel Strip"), Some(probe)));
        assert!(watch.strip_wanted(1) && !watch.strip_wanted(2));
        let events = std::cell::RefCell::new(Vec::new());
        let mut out = |data: Value| events.borrow_mut().push(data);
        // No strip seen at this tick (another track selected): the probe says nothing.
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vocal", 0.0)], &mut out, &stays);
        assert!(events.borrow().is_empty());
        watch.saw_strip(Seen { place: 1, plugins: vec!["Channel EQ".into(), "Gain".into()], sends: vec![] });
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vocal", 0.0)], &mut out, &stays);
        assert!(events.borrow().is_empty(), "as applied");
        // The Gain gone from the strip: undone at once, whatever the titles say.
        watch.saw_strip(Seen { place: 1, plugins: vec!["Channel EQ".into()], sends: vec![] });
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vocal", 0.0)], &mut out, &stays);
        assert_eq!(events.borrow().len(), 1);
        assert_eq!(events.borrow()[0]["descs"], json!(["Kumi: Added “Gain” to “Vocal” (k0001)"]));
        // The strip read for a tick serves that tick only.
        assert_eq!(watch.strip_seen, None);
    }

    #[test]
    fn the_titles_name_kumis_steps_undone_in_order_and_redone_and_the_producers_own_are_not() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        menus(&hands, "Undo Create Marker", "Can’t Redo");
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let strings = Strings::english();
        let mut watch = Watch::default();
        watch.entries.push(entry("Kumi: Renamed track “Vocal” → “Vox” (k0001)", Some("Renaming"), None));
        watch.entries.push(entry("Kumi: Marker at bar 17 (k0002)", Some("Create Marker"), None));
        let events = std::cell::RefCell::new(Vec::new());
        let mut out = |data: Value| events.borrow_mut().push(data);
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
        // The producer pressed Cmd-Z twice and Logic validated its menu: the Redo title names the rename, so the marker
        // above it went too.
        menus(&hands, "Undo", "Redo Renaming");
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        assert_eq!(events.borrow().len(), 1);
        assert_eq!(events.borrow()[0]["descs"], json!(["Kumi: Renamed track “Vocal” → “Vox” (k0001)", "Kumi: Marker at bar 17 (k0002)"]));
        assert_eq!(events.borrow()[0]["how"], "undo");
        // Redone up to the marker: the Undo title names it, so both are back.
        menus(&hands, "Undo Create Marker", "Can’t Redo");
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        assert_eq!(events.borrow().len(), 2);
        assert_eq!(events.borrow()[1]["how"], "redo");
        assert_eq!(events.borrow()[1]["descs"].as_array().unwrap().len(), 2);
        // The producer's own step with another name, undone: not Kumi's.
        menus(&hands, "Undo Create Marker", "Redo Value Change");
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        assert_eq!(events.borrow().len(), 2);
        assert!(watch.top_is("Kumi: Marker at bar 17 (k0002)"));
        assert!(!watch.top_is("Kumi: Renamed track “Vocal” → “Vox” (k0001)"));
        watch.taken_back("Kumi: Marker at bar 17 (k0002)");
        assert!(watch.top_is("Kumi: Renamed track “Vocal” → “Vox” (k0001)"));
    }

    #[test]
    fn a_step_whose_probe_still_shows_it_applied_is_not_undone_by_a_title_that_shares_its_name() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        menus(&hands, "Undo Renaming", "Can’t Redo");
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let strings = Strings::english();
        let mut watch = Watch::default();
        let probe = Probe::Header {
            place: 1,
            read: |header| json!(header.name),
            same: |a, b| a == b,
            prior: json!("Vocal"),
            applied: json!("Vox"),
        };
        watch.entries.push(entry("Kumi: Renamed track “Vocal” → “Vox” (k0001)", Some("Renaming"), Some(probe)));
        let events = std::cell::RefCell::new(Vec::new());
        let mut out = |data: Value| events.borrow_mut().push(data);
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vox", 0.0)], &mut out, &stays);
        // The producer renamed another track by hand and undid that: the Redo title says Renaming, the header still Vox.
        menus(&hands, "Undo Renaming", "Redo Renaming");
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vox", 0.0)], &mut out, &stays);
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
        // Then Kumi's own rename undone: the header says so.
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[header("Vocal", 0.0)], &mut out, &stays);
        assert_eq!(events.borrow().len(), 1);
    }

    #[test]
    fn the_tick_puts_no_input_into_logic_and_reads_the_titles_as_they_stand() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": false})]);
        menus(&hands, "Undo Create Marker", "Can’t Redo");
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let strings = Strings::english();
        let mut watch = Watch::default();
        // A marker step has no probe: only the titles can name its undo.
        watch.entries.push(entry("Kumi: Marker at bar 9 (k0001)", Some("Create Marker"), None));
        let events = std::cell::RefCell::new(Vec::new());
        let mut out = |data: Value| events.borrow_mut().push(data);
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        assert_eq!(hands.calls_of("action") + hands.calls_of("keys"), 0, "behind: nothing pressed, nothing posted");
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        assert_eq!(hands.calls_of("action") + hands.calls_of("keys"), 0, "in front: the tick still posts nothing");
        assert!(hands.calls().iter().all(|(op, _)| op == "tree"), "the tick only reads: {:?}", hands.calls());
        assert!(events.borrow().is_empty());
        // The producer undid the marker and Logic validated its Edit menu (they opened it): the Redo title names it.
        menus(&hands, "Undo", "Redo Create Marker");
        watch.poll(&mut ax, &strings, &look(1, 160.0), &[], &mut out, &stays);
        assert_eq!(events.borrow().len(), 1);
        assert_eq!(events.borrow()[0]["descs"], json!(["Kumi: Marker at bar 9 (k0001)"]));
        assert_eq!(events.borrow()[0]["how"], "undo");
    }
}
