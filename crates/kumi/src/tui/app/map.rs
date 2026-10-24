//! /map: the producer's sounds and songs as two maps, full screen in place of the conversation until it closes (the
//! view is tui/map.rs). Space plays a point through Kumi's player; an action puts what it asks in the input box.
//! /map open serves the same maps to the browser (map_server.rs), whose actions land in the input box too.
use super::super::{
    keys::{MouseAction, MouseButton, WheelDirection},
    map::{Asked, MapView},
    pictures::{kitty, Raster},
    render::Cursor,
};
use super::*;
use crate::map_server::Served;
use kumi_media::audio::player::PlayerError;
use std::{path::PathBuf, sync::Arc};

impl TuiApp {
    pub(super) fn map_open(&self) -> bool {
        self.0.state.borrow().map.is_some()
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
        self.0.state.borrow_mut().map = Some(MapView::new(self.0.graphics));
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
        let page = Arc::new(crate::map::Page::new(load().await));
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
        let server = match crate::map_server::open(served).await {
            Ok(server) => server,
            Err(error) => {
                self.notice(&format!("Kumi couldn't open the map in your browser: {error}"), NoticeTone::Warn);
                return;
            }
        };
        let (url, port) = (server.url.clone(), server.port);
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
            open(&url);
        }
        self.notice(&format!("The map is open in your browser: {url} (/map close closes it)."), NoticeTone::Info);
    }

    /// /map close: the browser's map stops answering.
    fn close_map_page(&self) {
        let server = self.0.state.borrow_mut().map_page.take();
        let words =
            if server.is_some() { "The map in your browser is closed." } else { "The map isn't open in your browser: /map open opens it." };
        drop(server);
        self.notice(words, NoticeTone::Info);
    }

    /// What the producer chose on the browser's map, in the input box to send; the terminal's map closes so it shows.
    fn from_map_page(&self, words: &str) {
        if self.map_open() {
            self.close_map();
        }
        self.put_in_box(words);
    }

    /// Words for the producer to send, in the input box in place of what's there.
    pub fn put_in_box(&self, words: &str) {
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
                InputEvent::Mouse { action: MouseAction::Wheel, direction, x, y, .. } => {
                    view.on_wheel(direction == Some(WheelDirection::Up), x, y)
                }
                _ => Asked::Nothing,
            }
        };
        match asked {
            Asked::Nothing => {}
            Asked::Close => self.close_map(),
            Asked::Put(words) => {
                self.close_map();
                self.put_in_box(&words);
            }
            Asked::Play { path, from, seconds, name } => self.map_play(path, from, seconds, name),
            Asked::Stop => {
                if let Some(player) = &self.0.options.player {
                    player.stop();
                }
            }
        }
        self.0.scheduler.request();
    }

    /// The map closes, its image comes down, and what it played stops.
    fn close_map(&self) {
        let mut state = self.0.state.borrow_mut();
        state.map = None;
        state.map_image = None;
        state.images.forget("map");
        drop(state);
        if let Some(player) = &self.0.options.player {
            player.stop();
        }
    }

    /// A point played: shown as playing until it ends, or why it didn't play.
    fn map_play(&self, path: String, from: f64, seconds: f64, name: String) {
        let Some(player) = self.0.options.player.clone() else {
            if let Some(view) = self.0.state.borrow_mut().map.as_mut() {
                view.played(Some(format!("This Kumi can't play {name}.")));
            }
            return;
        };
        let app = self.clone();
        tokio::task::spawn_local(async move {
            let said = match player.play(&path, from, seconds, Signal::new()).await {
                Ok(()) => {
                    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                    // Another sound took over: it says when it ends.
                    if player.playing() {
                        return;
                    }
                    None
                }
                // Another sound, or a stop, came while this one's file was read.
                Err(PlayerError::Stopped) => return,
                Err(error) => Some(error.to_string()),
            };
            if let Some(view) = app.0.state.borrow_mut().map.as_mut() {
                view.played(said);
            }
            app.0.scheduler.request();
        });
    }
}
