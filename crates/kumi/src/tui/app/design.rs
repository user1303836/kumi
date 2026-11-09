//! Sound design in the app: the palette's sounds or the map's picks crossed, fused or stacked into a new sound
//! (kumi-agent's core design, which kumi-design renders). PALETTE lists a row for each way to make one and, under MADE,
//! the sounds made this run: Space plays one, r makes another variation, k keeps it in the palette, x lets it go. The
//! map makes one from its picks with d and a mode's key, then plays it; r, k and p work on it there.
use super::super::width::{text_width, truncate};
use super::drawing::{sp, st};
use super::*;
use kumi_agent::core::{
    design::{self, Made},
    palette::Gathered,
};
use kumi_common::abort::Signal;
use kumi_design::{Mode, Op, Options, Request as DesignRequest};

/// How much of a made sound Space plays, at most (seconds).
const PLAYED: f64 = 30.;

/// The ways to make a sound, as PALETTE lists them and the map's keys choose them: the op and mode, its name, and what
/// it does in a few words.
pub(super) const WAYS: [(Op, Option<Mode>, &str, &str); 8] = [
    (Op::Cross, Some(Mode::Blend), "Blend", "one body morphed from them all"),
    (Op::Cross, Some(Mode::Attack), "Attack", "one's attack on the blend of the rest"),
    (Op::Cross, Some(Mode::Resonate), "Resonate", "half ring through the other half"),
    (Op::Cross, Some(Mode::Modulate), "Modulate", "they modulate each other"),
    (Op::Fuse, Some(Mode::Texture), "Texture", "a loop that moves between them"),
    (Op::Fuse, Some(Mode::Loop), "Loop", "their hits as a 2-bar beat"),
    (Op::Fuse, Some(Mode::Smear), "Smear", "a long granular stretch"),
    (Op::Stack, None, "Stack", "printed as one hit"),
];

/// A sound being made or made: its request (the files and the op), then what came of it.
pub(super) struct Design {
    pub id: u64,
    pub request: DesignRequest,
    pub made: Option<Made>,
    pub error: Option<String>,
}

/// What a way is called in words ("a blend", "a stack").
fn named(op: Op, mode: Option<Mode>) -> String {
    let name = WAYS.iter().find(|w| w.0 == op && w.1 == mode).map_or("sound", |w| w.2).to_lowercase();
    format!("{} {name}", if name.starts_with('a') { "an" } else { "a" })
}

impl TuiApp {
    /// Whether this Kumi makes sounds (it has a folder for them).
    pub(super) fn designs(&self) -> bool {
        self.0.options.design_dir.is_some()
    }

    /// Makes a sound from `files` (at most as many as the op takes), listed under MADE at once and played when it's
    /// made: its number.
    pub(super) fn design(&self, op: Op, mode: Option<Mode>, files: Vec<String>, options: Options, from_map: bool) -> Option<u64> {
        self.make(design::request(op, mode, &files, None, options), from_map)
    }

    /// The map's path grown from one sound toward another: its points made one after another (each listed under MADE and
    /// on the map as it's made, and played), until it's done or a newer path takes over.
    pub(super) fn grow_path(&self, requests: Vec<DesignRequest>) {
        let Some(folder) = self.0.options.design_dir.clone() else { return };
        let count = requests.len();
        let path = {
            let mut state = self.0.state.borrow_mut();
            state.map_grow += 1;
            if let Some(view) = state.map.as_mut() {
                view.made(Some(format!("Growing {count} sounds along the way…")));
            }
            state.map_grow
        };
        let app = self.clone();
        tokio::task::spawn_local(async move {
            for (at, request) in requests.into_iter().enumerate() {
                let seed = request.seed.unwrap_or_default();
                let id = {
                    let mut state = app.0.state.borrow_mut();
                    if state.map_grow != path {
                        return;
                    }
                    state.design_seq += 1;
                    let id = state.design_seq;
                    state.designs.insert(0, Design { id, request: request.clone(), made: None, error: None });
                    state.palette_rows = None;
                    id
                };
                let made = design::make(request, &folder, &Signal::new()).await;
                let mut state = app.0.state.borrow_mut();
                if let Some(design) = state.designs.iter_mut().find(|d| d.id == id) {
                    match made.clone() {
                        Ok(made) => design.made = Some(made),
                        Err(why) => design.error = Some(why),
                    }
                }
                state.palette_rows = None;
                if state.map_grow != path {
                    return;
                }
                match made {
                    Ok(made) => {
                        state.map_design = Some(id);
                        if let Some(view) = state.map.as_mut() {
                            view.bud(seed, at, id, made.path.to_string_lossy().into_owned());
                            view.made(Some(format!("Grown {} of {count}: {}", at + 1, made.answer.recipe)));
                        }
                        drop(state);
                        app.play_made(&made);
                    }
                    Err(why) => {
                        if let Some(view) = state.map.as_mut() {
                            view.made(Some(why.clone()));
                        }
                        drop(state);
                        app.notice(&why, NoticeTone::Warn);
                        return;
                    }
                }
                app.0.scheduler.request();
            }
        });
    }

    fn make(&self, request: DesignRequest, from_map: bool) -> Option<u64> {
        let folder = self.0.options.design_dir.clone()?;
        let id = {
            let mut state = self.0.state.borrow_mut();
            state.design_seq += 1;
            let id = state.design_seq;
            state.designs.insert(0, Design { id, request: request.clone(), made: None, error: None });
            state.palette_rows = None;
            if from_map {
                state.map_design = Some(id);
                if let Some(view) = state.map.as_mut() {
                    view.made(Some(format!("Making {} of {}…", named(request.op, request.mode), request.sources.len())));
                }
            }
            id
        };
        self.0.scheduler.request();
        let (app, made_on_map) = (self.clone(), from_map);
        tokio::task::spawn_local(async move {
            let made = design::make(request, &folder, &Signal::new()).await;
            let said = {
                let mut state = app.0.state.borrow_mut();
                // Every sound made on the map is listed under it, to make the next one from.
                if let (Ok(made), true, Some(view)) = (&made, made_on_map, state.map.as_mut()) {
                    view.made_sound(id, made.path.to_string_lossy().into_owned());
                }
                let from_map = state.map_design == Some(id);
                let said = match &made {
                    Ok(made) => format!("Made: {}", made.answer.recipe),
                    Err(why) => why.clone(),
                };
                if let Some(design) = state.designs.iter_mut().find(|d| d.id == id) {
                    match made.clone() {
                        Ok(made) => design.made = Some(made),
                        Err(why) => design.error = Some(why),
                    }
                }
                state.palette_rows = None;
                if from_map {
                    if let Some(view) = state.map.as_mut() {
                        view.made(Some(said.clone()));
                    }
                }
                said
            };
            match made {
                Ok(made) => app.play_made(&made),
                Err(_) => app.notice(&said, NoticeTone::Warn),
            }
            app.0.scheduler.request();
        });
        Some(id)
    }

    /// Another variation of a sound made: the same sounds and way, a new seed.
    pub(super) fn reroll_design(&self, id: u64) {
        let request = self.0.state.borrow().designs.iter().find(|d| d.id == id).map(|d| d.request.clone());
        if let Some(mut request) = request {
            request.seed = None;
            let from_map = self.0.state.borrow().map_design == Some(id);
            self.make(request, from_map);
        }
    }

    /// A sound made, into the session's palette: what happened, in words (on the map too when it made it).
    pub(super) fn keep_design(&self, id: u64) {
        let Some(made) = self.made(id) else { return };
        let path = made.path.to_string_lossy().into_owned();
        let name = made.path.file_stem().map_or_else(|| path.clone(), |s| s.to_string_lossy().into_owned());
        let app = self.clone();
        tokio::task::spawn_local(async move {
            let Some(said) = app.palette_gather(vec![Gathered { path, name, song: false, class: None }]).await else {
                app.notice("This Kumi keeps no palette.", NoticeTone::Warn);
                return;
            };
            let said = said.unwrap_or_else(|why| why);
            let mut told_map = false;
            if app.0.state.borrow().map_design == Some(id) {
                if let Some(view) = app.0.state.borrow_mut().map.as_mut() {
                    view.made(Some(said.clone()));
                    told_map = true;
                }
            }
            if !told_map {
                app.notice(&said, NoticeTone::Info);
            }
            app.0.scheduler.request();
        });
    }

    fn made(&self, id: u64) -> Option<Made> {
        self.0.state.borrow().designs.iter().find(|d| d.id == id).and_then(|d| d.made.clone())
    }

    /// The sound made, played (again).
    pub(super) fn play_design(&self, id: u64) {
        if let Some(made) = self.made(id) {
            self.play_made(&made);
        }
    }

    fn play_made(&self, made: &Made) {
        let Some(player) = self.0.options.player.clone() else { return };
        let (app, path) = (self.clone(), made.path.to_string_lossy().into_owned());
        tokio::task::spawn_local(async move {
            if let Err(error) = player.play(&path, 0., PLAYED).await {
                app.notice(&format!("The sound made didn't play: {error}"), NoticeTone::Warn);
            }
        });
    }

    /// A sound made let go from MADE (its file stays where it was kept).
    fn forget_design(&self, id: u64) {
        let mut state = self.0.state.borrow_mut();
        state.designs.retain(|d| d.id != id);
        state.palette_rows = None;
        drop(state);
        self.0.scheduler.request();
    }

    /// PALETTE's rows for making sounds from the palette's first sounds (`files`), then MADE's.
    pub(super) fn design_rows(&self, files: &[String], width: i32) -> Vec<TabRow> {
        let mut rows = Vec::new();
        if !self.designs() {
            return rows;
        }
        let app = Rc::downgrade(&self.0);
        if files.len() >= kumi_design::MIN_SOURCES {
            rows.push(TabRow { spans: vec![sp(format!("MAKE from {}", sounds(files.len())), st::LABEL)], ..Default::default() });
            for (op, mode, name, does) in WAYS {
                let make: Rc<dyn Fn()> = {
                    let (app, files) = (app.clone(), files.to_vec());
                    Rc::new(move || {
                        if let Some(app) = app.upgrade() {
                            TuiApp(app).design(op, mode, files.clone(), Options::default(), false);
                        }
                    })
                };
                let room = (width - text_width(name) - 4).max(4);
                rows.push(TabRow {
                    spans: vec![sp(format!("{name:<9}"), st::ACCENT), sp(truncate(does, (room - 9).max(4)), st::FAINT)],
                    action: Some(make.clone()),
                    keys: vec![("space", make)],
                    ..Default::default()
                });
            }
        }
        let designs = &self.0.state.borrow().designs;
        if designs.is_empty() {
            return rows;
        }
        rows.push(TabRow { spans: vec![sp("MADE  space plays · r another · k keeps · x lets go", st::LABEL)], ..Default::default() });
        for design in designs {
            let id = design.id;
            let on = |what: fn(&TuiApp, u64)| -> Rc<dyn Fn()> {
                let app = app.clone();
                Rc::new(move || {
                    if let Some(app) = app.upgrade() {
                        what(&TuiApp(app), id);
                    }
                })
            };
            let (text, style, right) = match (&design.made, &design.error) {
                (Some(made), _) => (made.answer.recipe.clone(), st::TEXT, format!("{:.1} s", made.answer.duration)),
                (None, Some(why)) => (why.clone(), st::WARN, "didn't make".into()),
                (None, None) => (
                    format!("Making {} of {}…", named(design.request.op, design.request.mode), design.request.sources.len()),
                    st::FAINT,
                    "making".into(),
                ),
            };
            let room = (width - text_width(&right) - 3).max(8);
            let play = on(TuiApp::play_design);
            rows.push(TabRow {
                spans: vec![sp(truncate(&text, room), style)],
                right: Some(sp(right, st::FAINT)),
                action: Some(play.clone()),
                keys: vec![
                    ("space", play),
                    ("r", on(TuiApp::reroll_design)),
                    ("k", on(TuiApp::keep_design)),
                    ("x", on(TuiApp::forget_design)),
                ],
                ..Default::default()
            });
        }
        rows
    }
}

fn sounds(n: usize) -> String {
    if n == 1 {
        "1 sound".into()
    } else {
        format!("{n} sounds")
    }
}
