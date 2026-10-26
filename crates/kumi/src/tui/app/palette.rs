//! The session's palette in the app: the PALETTE tab (on the strip once the session has one), /palette, and files the
//! map or the browser's map gathers into it. The session keeps the palette (kumi-agent's core palette); the app shows it
//! as the session last told it.
use super::super::width::{text_width, truncate};
use super::drawing::{sp, st};
use super::*;
use kumi_agent::core::palette::{Gathered, PaletteFile, PaletteView};
use kumi_listening::music::tool::tilde;

/// How much of a palette's file Space plays in PALETTE, at most (seconds).
const PLAYED: f64 = 30.;

/// The PALETTE tab: the session's palette, a row a file, and loading its sounds as a kit.
pub(super) struct PaletteTab {
    pub app: Weak<Inner>,
}
impl Tab for PaletteTab {
    fn id(&self) -> &str {
        "palette"
    }
    fn title(&self) -> &str {
        "PALETTE"
    }
    fn badge(&self) -> Option<i64> {
        self.app.upgrade().and_then(|a| a.state.borrow().palette.as_ref().map(|palette| palette.files.len() as i64))
    }
    fn rows(&self, width: i32) -> Rc<[TabRow]> {
        self.app.upgrade().map(|a| TuiApp(a).palette_rows(width)).unwrap_or_default()
    }
    fn empty(&self) -> Option<&str> {
        Some("Nothing in it yet: pick sounds on /map and press Enter")
    }
    fn shown(&self) -> bool {
        self.app.upgrade().is_some_and(|a| a.state.borrow().palette.is_some())
    }
}

impl TuiApp {
    /// The session's palette changed (or there's none now): PALETTE and the map show it as it is.
    pub(super) fn palette_event(&self, palette: Option<PaletteView>) {
        {
            let mut state = self.0.state.borrow_mut();
            let paths = palette.as_ref().map(|palette| palette.files.iter().map(|file| file.path.clone()).collect()).unwrap_or_default();
            let reached = palette.as_ref().map(|palette| palette.reached.iter().cloned().collect()).unwrap_or_default();
            if let Some(map) = state.map.as_mut() {
                map.in_palette(paths, reached);
            }
            state.palette = palette;
        }
        self.0.scheduler.request();
    }

    /// /palette and its words: the session answers, in words.
    pub(super) async fn palette_command(&self, words: &str) {
        let controller = self.0.options.controller.clone();
        match controller.palette_command(words.trim()).await {
            Ok(said) => self.notice(&said, NoticeTone::Info),
            Err(error) => self.notice(&error.to_string(), NoticeTone::Warn),
        }
    }

    /// Files gathered on the map or in the browser's map, into the session's palette: what happened, in words (None when
    /// this Kumi keeps no palettes).
    pub(super) async fn palette_gather(&self, files: Vec<Gathered>) -> Option<Result<String, String>> {
        let controller = self.0.options.controller.clone();
        if !controller.has_palette() || files.is_empty() {
            return None;
        }
        Some(controller.palette_gather(files).await.map_err(|error| error.to_string()))
    }

    /// A row a file (its name, and its class or a song's style at the right), then a row that asks Kumi to load the
    /// palette's sounds as a kit.
    fn palette_rows(&self, width: i32) -> Rc<[TabRow]> {
        let Some(palette) = self.0.state.borrow().palette.clone() else { return Rc::default() };
        let mut rows: Vec<TabRow> = palette.files.iter().map(|file| self.palette_row(file, width)).collect();
        let sounds: Vec<&PaletteFile> = palette.files.iter().filter(|file| !file.song).collect();
        if !sounds.is_empty() {
            let words = format!(
                "Load these sounds from my palette {} as a kit: {}",
                palette.name,
                sounds.iter().map(|file| tilde(&file.path)).collect::<Vec<_>>().join(", ")
            );
            let app = Rc::downgrade(&self.0);
            let action: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(app) = app.upgrade() {
                    TuiApp(app).put_in_box(&words);
                }
            });
            rows.push(TabRow {
                spans: vec![sp("Load as a kit", st::ACCENT)],
                right: Some(sp(format!("{} · {}", palette.name, palette.mode()), st::FAINT)),
                action: Some(action),
                ..Default::default()
            });
        }
        rows.into()
    }

    fn palette_row(&self, file: &PaletteFile, width: i32) -> TabRow {
        let what = file.class.clone().unwrap_or_else(|| if file.song { "song".into() } else { "sound".into() });
        let room = (width - text_width(&what) - 3).max(8);
        let app = Rc::downgrade(&self.0);
        let play: Rc<dyn Fn()> = {
            let (app, path, name) = (app.clone(), file.path.clone(), file.name.clone());
            Rc::new(move || {
                if let Some(app) = app.upgrade() {
                    TuiApp(app).palette_play(path.clone(), name.clone());
                }
            })
        };
        let remove: Rc<dyn Fn()> = {
            let (path, size, mtime) = (file.path.clone(), file.size, file.mtime);
            Rc::new(move || {
                let Some(app) = app.upgrade() else { return };
                let (app, path) = (TuiApp(app), path.clone());
                tokio::task::spawn_local(async move {
                    let controller = app.0.options.controller.clone();
                    if let Err(error) = controller.palette_remove(&path, size, mtime).await {
                        app.notice(&error.to_string(), NoticeTone::Warn);
                    }
                });
            })
        };
        TabRow {
            spans: vec![sp(truncate(&file.name, room), st::TEXT)],
            right: Some(sp(what, st::FAINT)),
            action: Some(play.clone()),
            keys: vec![("space", play), ("x", remove.clone()), ("delete", remove.clone()), ("backspace", remove)],
            ..Default::default()
        }
    }

    /// A palette's file played through Kumi's player (its first PLAYED seconds), or why it can't be.
    fn palette_play(&self, path: String, name: String) {
        let Some(player) = self.0.options.player.clone() else {
            self.notice(&format!("This Kumi can't play {name}."), NoticeTone::Warn);
            return;
        };
        let app = self.clone();
        tokio::task::spawn_local(async move {
            if let Err(error) = player.play(&path, 0., PLAYED).await {
                app.notice(&format!("{name} didn't play: {error}"), NoticeTone::Warn);
            }
        });
    }
}
