//! /map: the producer's sounds and songs as two maps, full screen in place of the conversation until it closes (the
//! view is tui/map.rs). Space plays a point through Kumi's player, and so does the mouse resting on one (the terminal
//! reports the mouse's moves while the map is open); an action puts what it asks in the input box.
//! /map open serves the same maps to the browser (map_server.rs), whose actions land in the input box too.
use super::super::{
    keys::{MouseAction, MouseButton, WheelDirection},
    map::{Asked, MapView},
    pictures::{kitty, Raster},
    render::Cursor,
};
use super::*;
use crate::map_server::Served;
use kumi_media::audio::player::Played;
use std::{path::PathBuf, sync::Arc};

/// How long the mouse rests on a point before it plays: a sweep across points plays none of them.
const HOVER_REST: Duration = Duration::from_millis(120);

impl TuiApp {
    pub(super) fn map_open(&self) -> bool {
        self.0.state.borrow().map.is_some()
    }

    /// The map is on the screen, taking the keys: open, with nothing Kumi asked over it. A panel that opens meanwhile
    /// (answers to pick at a turn's end, a question) shows in its place until it's done with, and the map then comes back.
    pub(super) fn map_shown(&self) -> bool {
        let state = self.0.state.borrow();
        state.map.is_some() && state.panel.is_none()
    }

    /// /map opens the map, and reads it; /map open shows it in the browser, and /map close closes that.
    pub(super) async fn map_command(&self, load: crate::map::LoadMap, words: &str) {
        match words.trim() {
            "" => {}
            "open" => return self.open_map_page(load).await,
            "close" => return self.close_map_page(),
            _ => {
                self.notice(
                    "/map opens the map of your sounds and songs; /map open shows it in your browser, and /map close closes that.",
                    NoticeTone::Warn,
                );
                return;
            }
        }
        // The map can't show the microphone's keys or what it hears: talking stops as it opens.
        if let Some(voice) = self.0.voice.as_ref().filter(|voice| voice.active()) {
            voice.cancel();
        }
        let hover = self.0.options.map_hover.as_ref().is_none_or(|hover| (hover.load)());
        self.0.state.borrow_mut().map = Some(MapView::new(self.0.graphics, hover));
        // The mouse's moves, reported while the map is open: a point plays as the mouse rests on it.
        self.0.tty.report_moves(true);
        self.0.scheduler.request();
        let data = load().await;
        if let Some(view) = self.0.state.borrow_mut().map.as_mut() {
            view.loaded(data);
        }
        self.0.scheduler.request();
    }

    /// /map open: the map as it is now, served to the browser on this computer until /map close or Kumi quits (a map
    /// opened before closes). Over SSH the browser is on another computer: Kumi says how to reach the map from it.
    async fn open_map_page(&self, load: crate::map::LoadMap) {
        let asked = {
            let mut state = self.0.state.borrow_mut();
            state.map_page_asked += 1;
            state.map_page_opening = true;
            state.map_page_asked
        };
        // Kumi quitting, /map close or another /map open while the map was read: this one opens nothing.
        let wanted = |app: &TuiApp| {
            let state = app.0.state.borrow();
            !state.closing && state.map_page_asked == asked
        };
        let reading = load();
        tokio::pin!(reading);
        let data = tokio::select! {
            data = &mut reading => data,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if wanted(self) {
                    self.notice("Kumi is reading your map for the browser…", NoticeTone::Info);
                }
                reading.await
            }
        };
        if !wanted(self) {
            return;
        }
        let page = Arc::new(crate::map::Page::new(data));
        let app = Rc::downgrade(&self.0);
        let served = Served {
            points: Box::new({
                let page = page.clone();
                move || {
                    let page = page.clone();
                    async move { tokio::task::spawn_blocking(move || page.points()).await.unwrap_or_default() }.boxed_local()
                }
            }),
            audio: Box::new({
                let page = page.clone();
                move |id| page.point(id).map(|(_, point)| (PathBuf::from(&point.path), point.from as f64, point.seconds as f64))
            }),
            action: Box::new(move |action, id| match (page.asked(action, id), app.upgrade()) {
                (Some(words), Some(app)) => {
                    TuiApp(app).from_map_page(&words);
                    true
                }
                _ => false,
            }),
        };
        let opened = crate::map_server::open(served).await;
        if !wanted(self) {
            return;
        }
        self.0.state.borrow_mut().map_page_opening = false;
        let server = match opened {
            Ok(server) => server,
            Err(error) => {
                self.notice(&format!("Kumi couldn't open the map in your browser: {error}"), NoticeTone::Warn);
                return;
            }
        };
        let (url, port, once) = (server.url.clone(), server.port, server.once.clone());
        self.0.state.borrow_mut().map_page = Some(server);
        if self.0.options.remote {
            self.notice(
                &format!(
                    "Kumi runs on another computer (over SSH), so the map is served there: connect with ssh -L {port}:127.0.0.1:{port} as well, then open {url} in your browser. /map close closes it."
                ),
                NoticeTone::Info,
            );
            return;
        }
        if let Some(open) = &self.0.options.open_browser {
            // A link that works once, which goes on to the map: the browser's command line (on Linux every account can
            // read it) never has the token.
            open(&once);
        }
        self.notice(&format!("The map is open in your browser: {url} (/map close closes it)."), NoticeTone::Info);
    }

    /// /map close: the browser's map stops answering (or, while it's being read, doesn't open).
    fn close_map_page(&self) {
        let (server, opening) = {
            let mut state = self.0.state.borrow_mut();
            state.map_page_asked += 1;
            (state.map_page.take(), std::mem::take(&mut state.map_page_opening))
        };
        let words = match (&server, opening) {
            (Some(_), _) => "The map in your browser is closed.",
            (None, true) => "The map won't open in your browser after all.",
            (None, false) => "The map isn't open in your browser: /map open opens it.",
        };
        drop(server);
        self.notice(words, NoticeTone::Info);
    }

    /// The music the browser's map shows changed (forgotten, or a folder taken away): that map closes, and one still
    /// being read doesn't open.
    pub(super) fn music_changed_under_map_page(&self) {
        let (server, opening) = {
            let mut state = self.0.state.borrow_mut();
            state.map_page_asked += 1;
            (state.map_page.take(), std::mem::take(&mut state.map_page_opening))
        };
        if server.is_some() || opening {
            drop(server);
            self.notice(
                "The map in your browser closed: it showed your music as it was (/map open shows it as it is now).",
                NoticeTone::Info,
            );
        }
    }

    /// What the producer chose on the browser's map, in the input box to send; the terminal's map closes so it shows.
    fn from_map_page(&self, words: &str) {
        if self.map_open() {
            self.close_map();
        }
        self.put_in_box(words);
    }

    /// Words for the producer to send, in the input box in place of what's there. Answers Kumi offered give way, so
    /// Enter sends these words.
    pub fn put_in_box(&self, words: &str) {
        let answers = self
            .0
            .state
            .borrow()
            .panel
            .as_ref()
            .is_some_and(|panel| matches!(&*panel.borrow(), Panel::Pick { picker, .. } if picker.borrow().options.answers));
        if answers {
            self.close_panel();
        }
        let mut state = self.0.state.borrow_mut();
        state.editor.set(words);
        state.menu_dismissed = false;
        drop(state);
        self.0.scheduler.request();
    }

    /// The map, full screen; its image goes up once, and the one it replaces comes down.
    pub(super) fn draw_map(&self, screen: &mut Screen) -> Option<Cursor> {
        let mut state = self.0.state.borrow_mut();
        let state = &mut *state;
        let (view, images, shown) = (state.map.as_mut()?, &mut state.images, &mut state.map_image);
        let mut upload = |key: &str, raster: &Raster, cols: usize, rows: usize| {
            if shown.as_deref() != Some(key) {
                images.forget("map");
                *shown = Some(key.to_string());
            }
            images.id(key, |id| kitty::transmit(id, raster, cols, rows))
        };
        view.draw(screen, &mut upload)
    }

    /// Keys, typing and the mouse go to the map while it's open; ctrl+c closes it.
    pub(super) fn map_input(&self, event: InputEvent) {
        let moved = matches!(event, InputEvent::Mouse { action: MouseAction::Move, .. });
        let asked = {
            let mut state = self.0.state.borrow_mut();
            let Some(view) = state.map.as_mut() else { return };
            match event {
                InputEvent::Key { name, mods, .. } if mods.ctrl => {
                    if name == "c" {
                        Asked::Close
                    } else {
                        Asked::Nothing
                    }
                }
                InputEvent::Key { name, mods, .. } => view.on_key(&name, mods.shift),
                InputEvent::Text { text } | InputEvent::Paste { text } => view.on_text(&text),
                InputEvent::Mouse { action: MouseAction::Press, button: MouseButton::Left, x, y, .. } => view.on_click(x, y),
                InputEvent::Mouse { action: MouseAction::Move, x, y, .. } => view.on_move(x, y),
                InputEvent::Mouse { action: MouseAction::Wheel, direction, x, y, .. } => {
                    view.on_wheel(direction == Some(WheelDirection::Up), x, y)
                }
                _ => Asked::Nothing,
            }
        };
        // The mouse crossing the map changes nothing on screen until it comes to a point or leaves one.
        if moved && asked == Asked::Nothing {
            return;
        }
        match asked {
            Asked::Nothing => {}
            Asked::Close => self.close_map(),
            Asked::Put(words) => {
                self.close_map();
                self.put_in_box(&words);
            }
            Asked::Play { path, from, seconds, name } => {
                // Space's play: a hover about to start doesn't, so it can't take Space's play over.
                self.0.state.borrow_mut().map_hover += 1;
                self.map_play(path, from, seconds, 0., name);
            }
            Asked::Stop => {
                self.0.state.borrow_mut().map_play += 1;
                if let Some(player) = &self.0.options.player {
                    player.stop();
                }
            }
            Asked::Hover { path, from, seconds, fade, name } => self.map_hover(path, from, seconds, fade, name),
            Asked::Leave => self.map_leave(),
            Asked::HoverPlays(on) => {
                if let Some(hover) = &self.0.options.map_hover {
                    (hover.save)(on);
                }
                if !on {
                    self.map_leave();
                }
            }
        }
        self.0.scheduler.request();
    }

    /// The mouse rests on a point: it plays once it has rested HOVER_REST, unless the mouse left it, or the map closed,
    /// meanwhile.
    fn map_hover(&self, path: String, from: f64, seconds: f64, fade: f64, name: String) {
        let number = {
            let mut state = self.0.state.borrow_mut();
            state.map_hover += 1;
            state.map_hover
        };
        let app = self.clone();
        tokio::task::spawn_local(async move {
            tokio::time::sleep(HOVER_REST).await;
            {
                let mut state = app.0.state.borrow_mut();
                if state.map_hover != number {
                    return;
                }
                let Some(view) = state.map.as_mut() else { return };
                if !view.hover_plays() {
                    return;
                }
            }
            app.map_play(path, from, seconds, fade, name);
            app.0.scheduler.request();
        });
    }

    /// The mouse left the point it was on (or hover plays were turned off): a play about to start doesn't, and a
    /// hover's play stops. Space's plays on.
    fn map_leave(&self) {
        let stop = {
            let mut state = self.0.state.borrow_mut();
            state.map_hover += 1;
            let stop = state.map.as_mut().is_some_and(|view| view.hover_ends());
            if stop {
                state.map_play += 1;
            }
            stop
        };
        if stop {
            if let Some(player) = &self.0.options.player {
                player.stop();
            }
        }
    }

    /// The map closes, its image comes down, what it played stops and the mouse's moves stop being reported. A job's
    /// answer that waited for the producer may start now.
    fn close_map(&self) {
        let mut state = self.0.state.borrow_mut();
        state.map = None;
        state.map_image = None;
        state.map_play += 1;
        state.map_hover += 1;
        state.images.forget("map");
        drop(state);
        self.0.tty.report_moves(false);
        if let Some(player) = &self.0.options.player {
            player.stop();
        }
        self.maybe_carry_on();
    }

    /// A point played (faded out over `fade` seconds: a hover's preview, cut short): shown as playing until it ends, or
    /// why it didn't play. Only the latest play (no other, no stop and no closing since) says so: one taken over says
    /// nothing.
    fn map_play(&self, path: String, from: f64, seconds: f64, fade: f64, name: String) {
        let Some(player) = self.0.options.player.clone() else {
            if let Some(view) = self.0.state.borrow_mut().map.as_mut() {
                view.played(Some(format!("This Kumi can't play {name}.")));
            }
            return;
        };
        let number = {
            let mut state = self.0.state.borrow_mut();
            state.map_play += 1;
            state.map_play
        };
        let latest = move |app: &TuiApp| app.0.state.borrow().map_play == number;
        let app = self.clone();
        tokio::task::spawn_local(async move {
            let said = match player.play_fading(&path, from, seconds, fade).await {
                Ok(Played::Overtaken) => return,
                Ok(Played::Began) => {
                    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                    // The output plays the last of it a moment later (a block at a time, a fade before it): until it has,
                    // a second at most.
                    for _ in 0..50 {
                        if !latest(&app) || !player.playing() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    None
                }
                Err(error) => Some(error.to_string()),
            };
            if !latest(&app) {
                return;
            }
            if let Some(view) = app.0.state.borrow_mut().map.as_mut() {
                view.played(said);
            }
            app.0.scheduler.request();
        });
    }
}
