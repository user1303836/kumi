use super::super::{
    style::{palette, Style},
    tree::{tree_rows, TreeFocus, TreeNode},
};
use super::*;
pub(super) struct Beat {
    pub beat: f64,
    pub beats_per_bar: Option<f64>,
    pub tempo: f64,
    /// The DAW's words for its time ("7/8", "LPB 4").
    pub meter: Option<String>,
}
impl TuiApp {
    pub(super) fn read_tree(&self, again: bool) {
        if !self.0.options.controller.can_show(BodyKind::Tree) {
            return;
        }
        let reference = {
            let mut state = self.0.state.borrow_mut();
            if state.connection != ConnectionState::Connected {
                return;
            }
            let Some(focus) = &state.focus else {
                return;
            };
            // (the chain open, when it isn't the track's own: REAPER's take FX)
            let Some(reference) = focus.chain_ref.clone().or_else(|| focus.track_ref.clone()).filter(|s| !s.is_empty()) else {
                return;
            };
            let key = format!(
                "{reference}\0{}\0{}\0{}",
                focus.device.as_deref().unwrap_or(""),
                focus.chain.as_deref().unwrap_or(""),
                focus.device_ref.as_deref().unwrap_or("")
            );
            if !again && state.tree_key.as_ref() == Some(&key) {
                return;
            }
            state.tree_key = Some(key);
            if state.tree_reading {
                state.tree_again = true;
                return;
            }
            state.tree_reading = true;
            reference
        };
        self.task(move |app| async move {
            let result = app.0.options.controller.focus_body(&BodyRequest::Tree { track_ref: reference }).await;
            let again = {
                let mut state = app.0.state.borrow_mut();
                if let Ok(Some(FocusBody::Tree(tree))) = result {
                    if state.focus.as_ref().and_then(|f| f.chain_ref.as_ref().or(f.track_ref.as_ref())) == Some(&tree.track_ref) {
                        state.tree = Some(tree);
                        state.tree_revision += 1;
                    }
                }
                state.tree_reading = false;
                let again = state.tree_again;
                state.tree_again = false;
                again
            };
            if again {
                app.read_tree(true);
            }
            if !app.0.state.borrow().closing {
                app.0.scheduler.request();
            }
            Ok(())
        });
    }
    pub(super) fn read_clip(&self, again: bool) {
        if !self.0.options.controller.can_show(BodyKind::Notes) {
            return;
        }
        let reference = {
            let mut state = self.0.state.borrow_mut();
            if state.connection != ConnectionState::Connected {
                return;
            }
            let Some(focus) = &state.focus else {
                return;
            };
            // An item's notes are read in the note editor over the launcher grid, through the item's reference (or over
            // the timeline, for a DAW whose takes there show their notes).
            let over = main_kind(focus) == Some(ViewKind::Launcher)
                || (crate::daw::view().timeline_notes && main_kind(focus) == Some(ViewKind::Timeline));
            if editor_kind(focus) != Some(ViewKind::NoteEditor) || !over {
                return;
            }
            let Some(reference) = item_ref(focus).filter(|s| !s.is_empty()) else {
                return;
            };
            let name = focus.item.as_ref().and_then(|item| item.name.as_deref()).unwrap_or("");
            let key = format!("{reference}\0{name}\0{}", focus.selected_notes.unwrap_or(0));
            if !again && state.clip_key.as_ref() == Some(&key) {
                return;
            }
            state.clip_key = Some(key);
            if state.clip_reading {
                state.clip_again = true;
                return;
            }
            state.clip_reading = true;
            reference
        };
        self.task(move |app| async move {
            let result = app.0.options.controller.focus_body(&BodyRequest::Notes { item_ref: reference }).await;
            let again = {
                let mut state = app.0.state.borrow_mut();
                // A busy DAW answers nothing: what was shown stays.
                let busy = state.focus.as_ref().is_some_and(|focus| focus.busy.is_some());
                if let Ok(body) = result {
                    let clip = match body {
                        Some(FocusBody::Notes(notes)) => Some(notes),
                        _ => None,
                    };
                    let kept = clip.is_none() && busy;
                    if !kept
                        && (clip.is_none() || clip.as_ref().map(|c| c.item_ref().to_string()) == state.focus.as_ref().and_then(item_ref))
                    {
                        state.clip = clip;
                    }
                }
                state.clip_reading = false;
                let again = state.clip_again;
                state.clip_again = false;
                again
            };
            if again {
                app.read_clip(true);
            }
            if !app.0.state.borrow().closing {
                app.0.scheduler.request();
            }
            Ok(())
        });
    }
    pub(super) fn strip_kind(&self) -> Option<BodyKind> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref()?;
        if state.connection != ConnectionState::Connected || focus.track.is_none() {
            return None;
        }
        if state.touched == Some(Touched::Timeline) && main_kind(focus) == Some(ViewKind::Timeline) {
            return Some(BodyKind::Timeline);
        }
        if state.touched == Some(Touched::Grid)
            && main_kind(focus) == Some(ViewKind::Launcher)
            && focus.track_ref.as_ref().is_some_and(|s| !s.is_empty())
            && section_index(focus).is_some()
        {
            Some(BodyKind::Grid)
        } else {
            None
        }
    }
    pub(super) fn read_strip(&self, again: bool) {
        let Some(kind) = self.strip_kind() else {
            return;
        };
        let (reference, scene) = {
            let mut state = self.0.state.borrow_mut();
            let Some(focus) = &state.focus else {
                return;
            };
            let reference = focus.track_ref.clone().unwrap_or_default();
            let scene = section_index(focus).unwrap_or(0.);
            // Live's Arrangement strip is the same whatever is selected; another DAW's lanes centre on the selected track,
            // so another selection reads them again at once.
            let key = if kind == BodyKind::Grid {
                format!("s\0{reference}\0{}", number::to_string(scene))
            } else if crate::daw::current().id() == kumi_types::daw::DawId::Ableton {
                "a".into()
            } else {
                format!("a\0{reference}")
            };
            if (!again && state.strip.key.as_ref() == Some(&key)) || state.strip.reading {
                return;
            }
            state.strip.key = Some(key);
            state.strip.reading = true;
            (reference, scene)
        };
        if !self.0.options.controller.can_show(kind) {
            self.0.state.borrow_mut().strip.reading = false;
            return;
        }
        self.task(move |app| async move {
            let request =
                if kind == BodyKind::Grid { BodyRequest::Grid { track_ref: reference, index: scene } } else { BodyRequest::Timeline };
            if let Ok(body) = app.0.options.controller.focus_body(&request).await {
                let mut state = app.0.state.borrow_mut();
                // A busy DAW answers nothing: what was shown stays.
                let busy = state.focus.as_ref().is_some_and(|focus| focus.busy.is_some());
                match (kind, body) {
                    (BodyKind::Grid, Some(FocusBody::Grid(strip))) => state.strip.grid = Some(strip),
                    (_, Some(FocusBody::Timeline(strip))) => state.strip.timeline = Some(strip),
                    _ if busy => {}
                    (BodyKind::Grid, _) => state.strip.grid = None,
                    (_, _) => state.strip.timeline = None,
                }
            }
            app.0.state.borrow_mut().strip.reading = false;
            if !app.0.state.borrow().closing {
                app.0.scheduler.request();
            }
            Ok(())
        });
    }
    pub(super) fn keep_tree_fresh(&self, shown: Option<BodyKind>) {
        let mut state = self.0.state.borrow_mut();
        if shown != state.refreshing {
            if let Some(timer) = state.tree_refresh.take() {
                timer.abort();
            }
        }
        state.refreshing = shown;
        if let Some(shown) = shown {
            if state.tree_refresh.is_none() {
                let weak = Rc::downgrade(&self.0);
                state.tree_refresh = Some(tokio::task::spawn_local(async move {
                    // What moves as the DAW plays (the timeline, a tracker's lines and sequence) is read more often.
                    let period = Duration::from_millis(match shown {
                        BodyKind::Timeline | BodyKind::Tracker | BodyKind::Matrix => 1500,
                        _ => 4000,
                    });
                    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
                    loop {
                        interval.tick().await;
                        let Some(inner) = weak.upgrade() else {
                            return;
                        };
                        let app = TuiApp(inner);
                        if app.0.state.borrow().closing {
                            return;
                        }
                        // What the pane shows is what's read again (the detail view can hold a clip or a device
                        // the pane doesn't show).
                        match shown {
                            BodyKind::Tree => app.read_tree(true),
                            BodyKind::Notes => app.read_clip(true),
                            BodyKind::Grid | BodyKind::Timeline => app.read_strip(true),
                            _ => app.read_own(true),
                        }
                    }
                }));
            }
        } else if let Some(timer) = state.tree_refresh.take() {
            timer.abort();
        }
    }
    /// The body of a DAW's own view FOCUS shows now: what the producer touched last, where they are, when the DAW
    /// can draw it; with the request that reads it.
    pub(super) fn own_request(&self) -> Option<BodyRequest> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref()?;
        if state.connection != ConnectionState::Connected {
            return None;
        }
        let request = match state.touched? {
            Touched::Tracker if main_kind(focus) == Some(ViewKind::PatternEditor) => BodyRequest::Tracker { item_ref: item_ref(focus) },
            Touched::Matrix
                if main_kind(focus) == Some(ViewKind::Sequencer)
                    || focus.panes.iter().flatten().any(|pane| pane.kind == ViewKind::Sequencer) =>
            {
                BodyRequest::Matrix
            }
            Touched::Keyzones if main_kind(focus) == Some(ViewKind::Instrument) => BodyRequest::Keyzones { item_ref: item_ref(focus)? },
            Touched::Routing if editor_kind(focus) == Some(ViewKind::Routing) => {
                BodyRequest::Routing { track_ref: focus.track_ref.clone().filter(|s| !s.is_empty())? }
            }
            Touched::Mixer if main_kind(focus) == Some(ViewKind::Mixer) || editor_kind(focus) == Some(ViewKind::Mixer) => {
                BodyRequest::Mixer { track_ref: focus.track_ref.clone().filter(|s| !s.is_empty()) }
            }
            _ => return None,
        };
        drop(state);
        self.0.options.controller.can_show(request.kind()).then_some(request)
    }
    /// Reads that body again when what it's for changed (or `again`).
    pub(super) fn read_own(&self, again: bool) {
        let Some(request) = self.own_request() else {
            self.0.state.borrow_mut().own.body = None;
            return;
        };
        let key = format!("{request:?}");
        {
            let mut state = self.0.state.borrow_mut();
            if !again && state.own.key.as_ref() == Some(&key) {
                return;
            }
            if state.own.key.as_ref() != Some(&key) {
                state.own.body = None;
            }
            state.own.key = Some(key.clone());
            if state.own.reading {
                state.own.again = true;
                return;
            }
            state.own.reading = true;
        }
        self.task(move |app| async move {
            let result = app.0.options.controller.focus_body(&request).await;
            let again = {
                let mut state = app.0.state.borrow_mut();
                // A read that brings nothing (refused: a busy DAW, Renoise while Kumi renders) leaves what was shown;
                // with nothing shown yet, the request is forgotten, so the next focus asks again.
                if state.own.key.as_ref() == Some(&key) {
                    match result {
                        Ok(Some(body)) => state.own.body = Some(body),
                        _ if state.own.body.is_none() => state.own.key = None,
                        _ => {}
                    }
                }
                state.own.reading = false;
                std::mem::take(&mut state.own.again)
            };
            if again {
                app.read_own(true);
            }
            if !app.0.state.borrow().closing {
                app.0.scheduler.request();
            }
            Ok(())
        });
    }
    /// Moves a tracker's cursor and playing line to where the focus and the transport say they are now, between the
    /// reads (every 1.5 s): a line past those read, or playback back at an earlier line (the next pattern, a loop),
    /// is read again.
    pub(super) fn follow_lines(&self) {
        let again = {
            let mut state = self.0.state.borrow_mut();
            let item = state.focus.as_ref().and_then(item_ref);
            let cursor = state.focus.as_ref().and_then(|focus| focus.line);
            let (playing, line) = state.transport.as_ref().map_or((false, None), |transport| (transport.playing, transport.line));
            let Some(FocusBody::Tracker(grid)) = state.own.body.as_mut() else { return };
            let read = grid.from..grid.from + grid.tracks.iter().map(|track| track.cells.len()).max().unwrap_or(0);
            // (another pattern's lines, or none: its own read is coming)
            if item.as_deref() != Some(grid.item_ref.as_str()) || read.is_empty() {
                return;
            }
            let mut again = false;
            if let Some(cursor) = cursor.filter(|cursor| grid.cursor != Some(*cursor)) {
                if read.contains(&cursor) {
                    grid.cursor = Some(cursor);
                } else {
                    again = true;
                }
            }
            if let Some(was) = grid.playing {
                match line {
                    Some(line) if line < was || (read.contains(&was) && !read.contains(&line)) => again = true,
                    Some(line) => grid.playing = Some(line),
                    None if !playing => grid.playing = None,
                    None => {}
                }
            }
            again
        };
        if again {
            self.read_own(true);
        }
    }
    /// The DAW's own body FOCUS draws now, read for where the producer is.
    pub(super) fn own_shown(&self) -> Option<FocusBody> {
        let request = self.own_request()?;
        let state = self.0.state.borrow();
        (state.own.key.as_deref() == Some(format!("{request:?}").as_str())).then(|| state.own.body.clone()).flatten()
    }
    pub(super) fn clip_shown(&self) -> Option<NoteView> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref()?;
        let clip = state.clip.as_ref()?;
        (state.connection == ConnectionState::Connected
            && focus.track.is_some()
            && editor_kind(focus) == Some(ViewKind::NoteEditor)
            && state.touched == Some(Touched::Notes)
            && Some(clip.item_ref()) == item_ref(focus).as_deref())
        .then(|| clip.clone())
    }
    pub(super) fn tree_shown(&self) -> Option<Rc<[TreeRow]>> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref()?;
        let tree = state.tree.as_ref()?;
        if state.connection != ConnectionState::Connected
            || focus.track.is_none()
            || Some(&tree.track_ref) != focus.chain_ref.as_ref().or(focus.track_ref.as_ref())
        {
            return None;
        }
        if (editor_kind(focus) != Some(ViewKind::DeviceChain) || state.touched != Some(Touched::Tree)) && state.tree_cursor.is_none() {
            return None;
        }
        // Laid out again only for a tree read again or another focus in it, not every frame (each row lists its siblings).
        let key = [&focus.device, &focus.chain, &focus.device_ref].map(|s| s.clone().filter(|s| !s.is_empty()));
        let revision = state.tree_revision;
        let cached = state.tree_rows.as_ref().filter(|(made, at, _)| *made == revision && *at == key).map(|(_, _, rows)| rows.clone());
        let rows = match cached {
            Some(rows) => rows,
            None => {
                let [device, chain, device_ref] = key.clone();
                let open_containers = crate::daw::view().open_containers;
                let rows: Rc<[TreeRow]> = tree_rows(tree, &TreeFocus { device, chain, device_ref, open_containers }).into();
                drop(state);
                self.0.state.borrow_mut().tree_rows = Some((revision, key, rows.clone()));
                rows
            }
        };
        (!rows.is_empty()).then_some(rows)
    }
    pub(super) fn pin(&self, row: &TreeRow) {
        let mut state = self.0.state.borrow_mut();
        let Some(focus) = &state.focus else {
            return;
        };
        let Some(track_ref) = focus.track_ref.clone().filter(|s| !s.is_empty()) else {
            return;
        };
        // A node the DAW names no ref for (a device in Renoise's Doofer or in a Splitter's lane, REAPER's input FX as a
        // chain) has one FOCUS made up after a '#': the pin names it with its container's ref, which the tools read.
        let reference = row.r#ref.split_once('#').map_or(row.r#ref.as_str(), |(container, _)| container).to_owned();
        state.pinned = Some(Pin {
            kind: row.kind,
            row: row.r#ref.clone(),
            pin: PinnedNode {
                track_ref,
                r#ref: reference,
                node: match row.node {
                    TreeNode::Device => PinKind::Device,
                    TreeNode::Chain => PinKind::Chain,
                },
                name: row.name.clone(),
                trail: row.trail.clone(),
                siblings: row.siblings.clone(),
                track: focus.track.as_ref().map(|t| t.name.clone()),
                live: None,
                time: None,
                // (where the tree's refs were read: Renoise's places move with its tracks before Kumi sees the pin)
                layout: state.tree.as_ref().and_then(|tree| tree.layout),
            },
        });
        drop(state);
        self.0.scheduler.request();
    }
    pub(super) fn unpin(&self) {
        self.0.state.borrow_mut().pinned = None;
        self.0.scheduler.request();
    }
    pub(super) fn wake_at(&self, at: f64, now: f64) {
        let mut state = self.0.state.borrow_mut();
        if state.closing {
            return;
        }
        if at <= now + 1. {
            drop(state);
            self.0.scheduler.request();
            return;
        }
        if state.wake_timer.is_some() && state.wake_time.is_some_and(|time| time <= at) {
            return;
        }
        if let Some(timer) = state.wake_timer.take() {
            timer.abort();
        }
        state.wake_time = Some(at);
        let weak = Rc::downgrade(&self.0);
        state.wake_timer = Some(tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_secs_f64((at - now).ceil() / 1000.)).await;
            if let Some(a) = weak.upgrade() {
                let mut state = a.state.borrow_mut();
                state.wake_timer = None;
                state.wake_time = None;
                if !state.closing {
                    drop(state);
                    a.scheduler.request();
                }
            }
        }));
    }
    pub(super) fn beat_now(&self, now: f64) -> Option<Beat> {
        let state = self.0.state.borrow();
        let transport = state.transport.as_ref()?;
        let tempo = transport.tempo?;
        let beat = transport.beat?;
        if !transport.playing || tempo <= 0. || state.connection != ConnectionState::Connected {
            return None;
        }
        Some(Beat {
            beat: beat + (now - transport.at).max(0.) * tempo / 60000.,
            tempo,
            beats_per_bar: transport.beats_per_bar.filter(|n| *n != 0.),
            meter: transport.meter.clone(),
        })
    }
    pub(super) fn next_beat(&self) {
        if let Some(timer) = self.0.state.borrow_mut().beat_timer.take() {
            timer.abort();
        }
        if let Some(at) = self.beat_now(perf_now()).filter(|_| !self.0.state.borrow().closing) {
            let phase = at.beat - at.beat.floor();
            let beat_edge = if phase < 0.25 { 0.25 - phase } else { 1. - phase };
            // (a bar that starts between beats, 7/8's, lights and dims on its own edges too; Live's light is as it always
            // was, on whole beats alone)
            let bar_edge = at.beats_per_bar.filter(|bar| *bar > 0. && bar.fract() != 0. && !lives_light()).map_or(f64::INFINITY, |bar| {
                let into = at.beat.rem_euclid(bar);
                if into < 0.25 {
                    0.25 - into
                } else {
                    bar - into
                }
            });
            let to_edge = beat_edge.min(bar_edge) * 60000. / at.tempo;
            let weak = Rc::downgrade(&self.0);
            self.0.state.borrow_mut().beat_timer = Some(tokio::task::spawn_local(async move {
                tokio::time::sleep(Duration::from_millis((to_edge.ceil() + 1.).max(4.) as u64)).await;
                if let Some(a) = weak.upgrade() {
                    a.state.borrow_mut().beat_timer = None;
                    let app = TuiApp(a);
                    app.0.scheduler.request();
                    app.next_beat();
                }
            }));
        }
        self.0.scheduler.request();
    }
    pub(super) fn beat_light(&self) -> Option<(String, Style)> {
        let at = self.beat_now(perf_now())?;
        let phase = at.beat - at.beat.floor();
        // The bar's first beat, wherever it falls: a bar of 3.5 beats (7/8) starts between quarter notes. Live's light is as
        // it always was: only a bar of whole beats has a bright first beat.
        let down = if lives_light() {
            phase < 0.25 && at.beats_per_bar.filter(|bar| bar.fract() == 0.).is_some_and(|bar| (at.beat + 1e-6).floor() % bar == 0.)
        } else {
            at.beats_per_bar.filter(|bar| *bar > 0.).is_some_and(|bar| (at.beat + 1e-6).rem_euclid(bar) < 0.25)
        };
        let color = if down {
            palette::BEAT
        } else if phase < 0.25 {
            helpers::mix_rgb(palette::BEAT, palette::OFFBEAT, 0.25)
        } else {
            palette::OFFBEAT
        };
        let tempo = if at.tempo.fract() == 0. { number::to_string(at.tempo) } else { number::to_fixed(at.tempo, 1) };
        let meter = at.meter.map(|meter| format!(" · {meter}")).unwrap_or_default();
        Some((format!("{tempo} BPM{meter}"), Style::fg(color)))
    }
}

/// Whether the beat light is Live's, which keeps its lights on whole beats (a 7/8 bar's start between beats isn't lit):
/// the other DAWs light a bar's first beat wherever it falls.
fn lives_light() -> bool {
    crate::daw::current().id() == kumi_types::daw::DawId::Ableton
}

/// The kind of view the producer works in.
fn main_kind(focus: &DawFocus) -> Option<ViewKind> {
    focus.main_view.as_ref().map(|view| view.kind)
}
/// The kind of editor shown with it.
fn editor_kind(focus: &DawFocus) -> Option<ViewKind> {
    focus.editor_view.as_ref().map(|view| view.kind)
}
/// The reference of the item in the editor.
fn item_ref(focus: &DawFocus) -> Option<String> {
    focus.item.as_ref().and_then(|item| item.item_ref.clone())
}
/// The position of the section selected.
fn section_index(focus: &DawFocus) -> Option<f64> {
    focus.section.as_ref().and_then(|section| section.index)
}
