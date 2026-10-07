use super::super::{
    style::{palette, Style},
    tree::{tree_rows, TreeFocus, TreeNode},
};
use super::*;
pub(super) struct Beat {
    pub beat: f64,
    pub beats_per_bar: Option<f64>,
    pub tempo: f64,
}
impl TuiApp {
    pub(super) fn read_tree(&self, again: bool) {
        if !self.0.options.controller.has_device_tree() {
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
            let Some(reference) = focus.track_ref.clone().filter(|s| !s.is_empty()) else {
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
            let result = app.0.options.controller.device_tree(&reference).await;
            let again = {
                let mut state = app.0.state.borrow_mut();
                if let Ok(Some(tree)) = result {
                    if state.focus.as_ref().and_then(|f| f.track_ref.as_ref()) == Some(&tree.track_ref) {
                        state.tree = Some(tree);
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
        if !self.0.options.controller.has_clip_view() {
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
            if focus.detail != Some(LiveDetail::Clip) || focus.view != Some(LiveView::Session) {
                return;
            }
            let Some(reference) = focus.slot_ref.clone().filter(|s| !s.is_empty()) else {
                return;
            };
            let key = format!("{reference}\0{}\0{}", focus.clip.as_deref().unwrap_or(""), focus.selected_notes.unwrap_or(0));
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
            let result = app.0.options.controller.clip_view(&reference).await;
            let again = {
                let mut state = app.0.state.borrow_mut();
                if let Ok(clip) = result {
                    if clip.is_none() || clip.as_ref().map(|c| &c.slot_ref) == state.focus.as_ref().and_then(|f| f.slot_ref.as_ref()) {
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
    pub(super) fn strip_kind(&self) -> Option<&'static str> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref()?;
        if state.connection != ConnectionState::Connected || focus.track.is_none() {
            return None;
        }
        if state.touched == Some(Touched::Arrangement) && focus.view == Some(LiveView::Arrangement) {
            return Some("arrangement");
        }
        if state.touched == Some(Touched::Session)
            && focus.view == Some(LiveView::Session)
            && focus.track_ref.as_ref().is_some_and(|s| !s.is_empty())
            && focus.scene_index.is_some()
        {
            Some("session")
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
            let scene = focus.scene_index.unwrap_or(0.);
            let key = if kind == "session" { format!("s\0{reference}\0{}", number::to_string(scene)) } else { "a".into() };
            if (!again && state.strip.key.as_ref() == Some(&key)) || state.strip.reading {
                return;
            }
            state.strip.key = Some(key);
            state.strip.reading = true;
            (reference, scene)
        };
        if (kind == "session" && !self.0.options.controller.has_session_strip())
            || (kind == "arrangement" && !self.0.options.controller.has_arrangement_strip())
        {
            self.0.state.borrow_mut().strip.reading = false;
            return;
        }
        self.task(move |app| async move {
            if kind == "session" {
                if let Ok(strip) = app.0.options.controller.session_strip(&reference, scene).await {
                    app.0.state.borrow_mut().strip.session = strip;
                }
            } else if let Ok(strip) = app.0.options.controller.arrangement_strip().await {
                app.0.state.borrow_mut().strip.arrangement = strip;
            }
            app.0.state.borrow_mut().strip.reading = false;
            if !app.0.state.borrow().closing {
                app.0.scheduler.request();
            }
            Ok(())
        });
    }
    pub(super) fn keep_tree_fresh(&self, shown: Option<&'static str>) {
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
                    let period = Duration::from_millis(if shown == "arrangement" { 1500 } else { 4000 });
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
                            "tree" => app.read_tree(true),
                            "clip" => app.read_clip(true),
                            _ => app.read_strip(true),
                        }
                    }
                }));
            }
        } else if let Some(timer) = state.tree_refresh.take() {
            timer.abort();
        }
    }
    pub(super) fn clip_shown(&self) -> Option<ClipView> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref()?;
        let clip = state.clip.as_ref()?;
        (state.connection == ConnectionState::Connected
            && focus.track.is_some()
            && focus.detail == Some(LiveDetail::Clip)
            && state.touched == Some(Touched::Clip)
            && Some(&clip.slot_ref) == focus.slot_ref.as_ref())
        .then(|| clip.clone())
    }
    pub(super) fn tree_shown(&self) -> Option<Vec<TreeRow>> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref()?;
        let tree = state.tree.as_ref()?;
        if state.connection != ConnectionState::Connected || focus.track.is_none() || Some(&tree.track_ref) != focus.track_ref.as_ref() {
            return None;
        }
        if (focus.detail != Some(LiveDetail::Device) || state.touched != Some(Touched::Device)) && state.tree_cursor.is_none() {
            return None;
        }
        let rows = tree_rows(
            tree,
            &TreeFocus {
                device: focus.device.clone().filter(|s| !s.is_empty()),
                chain: focus.chain.clone().filter(|s| !s.is_empty()),
                device_ref: focus.device_ref.clone().filter(|s| !s.is_empty()),
            },
        );
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
        state.pinned = Some(Pin {
            kind: row.kind,
            pin: PinnedNode {
                track_ref,
                r#ref: row.r#ref.clone(),
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
        })
    }
    pub(super) fn next_beat(&self) {
        if let Some(timer) = self.0.state.borrow_mut().beat_timer.take() {
            timer.abort();
        }
        if let Some(at) = self.beat_now(perf_now()).filter(|_| !self.0.state.borrow().closing) {
            let phase = at.beat - at.beat.floor();
            let to_edge = (if phase < 0.25 { 0.25 - phase } else { 1. - phase }) * 60000. / at.tempo;
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
        let down = at.beats_per_bar.filter(|bar| bar.fract() == 0.).is_some_and(|bar| (at.beat + 1e-6).floor() % bar == 0.);
        let color = if phase < 0.25 {
            if down {
                palette::BEAT
            } else {
                helpers::mix_rgb(palette::BEAT, palette::OFFBEAT, 0.25)
            }
        } else {
            palette::OFFBEAT
        };
        let tempo = if at.tempo.fract() == 0. { number::to_string(at.tempo) } else { number::to_fixed(at.tempo, 1) };
        Some((format!("{tempo} BPM"), Style::fg(color)))
    }
}
