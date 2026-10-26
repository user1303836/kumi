//! Palettes in a session: the sounds and songs the producer gathers to work with (on /map, in the browser's map, or by
//! asking Kumi), kept by name in Kumi's database and shared by every session. A session works with one at a time, its
//! palette: the first thing gathered makes one when there's none, and /palette names, lists and chooses them. The words
//! /palette answers with are made here, the same in every app.
//!
//! Kumi reads the palette each turn (`brief`) and works with it through the palette tool. A strict palette keeps Kumi to
//! its own sounds; a loose one (reach k) brings each sound's k nearest in the library, by the numbers the search
//! compares, ranked by the producer's taste: the sounds their own projects use first.

use crate::core::store_client::StoreClient;
use async_trait::async_trait;
use kumi_common::time::now_ms;
use kumi_library::{search::SoundIndex, Library};
use kumi_store::palettes::{self, Item, ItemKind, Palette};
use kumi_types::errors::RuntimeError;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::Rc,
};

/// What a palette works with in the producer's library: sounds that moved found again, the library's sounds (their
/// numbers, for a loose palette's nearest), and how many of the producer's own projects use each sound, for taste.
#[async_trait(?Send)]
pub trait PaletteLibrary {
    /// Sounds that moved, by their file's name and size: each one's path and modification time now, or None.
    async fn refind(&self, wanted: Vec<(String, i64)>) -> Vec<Option<(String, i64)>>;
    /// The library's sounds as the search compares them: None when they can't be read.
    async fn index(&self) -> Option<Rc<SoundIndex>>;
    /// How many of the producer's projects use each sound, by its path.
    async fn used(&self) -> HashMap<String, usize>;
}

/// The producer's library, for palettes.
pub struct LibrarySounds(pub Rc<Library>);

#[async_trait(?Send)]
impl PaletteLibrary for LibrarySounds {
    async fn refind(&self, wanted: Vec<(String, i64)>) -> Vec<Option<(String, i64)>> {
        let Some(index) = self.index().await else { return vec![None; wanted.len()] };
        wanted
            .iter()
            .map(|(name, size)| {
                index
                    .entries
                    .iter()
                    .find(|entry| entry.gone != Some(true) && entry.size as i64 == *size && file_name(&entry.path) == name)
                    .map(|entry| (entry.path.clone(), entry.mtime))
            })
            .collect()
    }
    async fn index(&self) -> Option<Rc<SoundIndex>> {
        self.0.sound_index().await.ok()
    }
    async fn used(&self) -> HashMap<String, usize> {
        self.0.samples_in_sets().await
    }
}

/// How far a loose palette reaches when /palette loose names no number: each sound brings its five nearest.
pub const LOOSE: u32 = 5;
/// The most a palette reaches: each sound's nearest past this are hardly like it.
pub const FARTHEST: u32 = 50;
/// The most files gathered at once (on the map, or the browser's): a palette is what to work with, not the library.
pub const MOST_GATHERED: usize = 10_000;

/// What the map or the browser's map gathers: a file, what it's called, a sound or a song, and its class or style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gathered {
    pub path: String,
    pub name: String,
    pub song: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

/// A file of a palette, as the app shows it and Kumi reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteFile {
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub name: String,
    pub song: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

impl From<&Item> for PaletteFile {
    fn from(item: &Item) -> PaletteFile {
        PaletteFile {
            path: item.path.clone(),
            size: item.size,
            mtime: item.mtime,
            name: item.name.clone(),
            song: item.kind == ItemKind::Song,
            class: item.class.clone(),
        }
    }
}

/// The session's palette as it is now: its name, how far it reaches (0: only its own sounds), its files in the order
/// they came, and the sounds it never brings in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteView {
    pub id: String,
    pub name: String,
    pub reach: u32,
    pub files: Vec<PaletteFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<PaletteFile>,
    /// What a loose palette brings in, by path (the app marks them on the map): none when it's strict.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reached: Vec<String>,
}

impl PaletteView {
    /// "Drums (strict)", "Pads (loose, 5 nearest)".
    pub fn mode(&self) -> String {
        match self.reach {
            0 => "strict".into(),
            reach => format!("loose, {reach} nearest"),
        }
    }
}

/// What gathering did: the palette it went into, how many were new to it, and how many it had already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gathering {
    pub palette: String,
    pub added: usize,
    pub had: usize,
}

impl Gathering {
    /// "Added 3 sounds to Drums." / "Drums has that sound already."
    pub fn words(&self) -> String {
        let things = |n: usize| if n == 1 { "1 sound".to_string() } else { format!("{n} sounds") };
        match (self.added, self.had) {
            (0, had) => {
                format!("{} has {} already.", self.palette, if had == 1 { "that sound".into() } else { format!("those {had} sounds") })
            }
            (added, 0) => format!("Added {} to {}.", things(added), self.palette),
            (added, had) => format!("Added {} to {} ({had} it had already).", things(added), self.palette),
        }
    }
}

/// A sound a loose palette brings in: one of its sounds' nearest that isn't its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reached {
    pub path: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The palette's sounds it's among the nearest of, by name.
    pub near: Vec<String>,
    /// How many of the producer's projects use it.
    pub used: usize,
    /// How far it is from the nearest of them, in the numbers the search compares.
    pub apart: f32,
}

/// A session's palettes: kumi.db's, and which one this session works with.
pub struct Palettes {
    store: StoreClient,
    /// The producer's library: where a sound that moved is found again (by its name and size), and what a loose
    /// palette brings in.
    library: Option<Rc<dyn PaletteLibrary>>,
    /// What the palette brought in last, and what it was worked out for (its files, its excluded sounds, its reach).
    reached: RefCell<Option<(String, Rc<Vec<Reached>>)>>,
    current: RefCell<Option<String>>,
    /// Told the palette as it is after each change (None: there's none now).
    on_change: Rc<dyn Fn(Option<PaletteView>)>,
}

fn failed(error: kumi_store::StoreError) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}

impl Palettes {
    pub fn new(store: StoreClient, library: Option<Rc<dyn PaletteLibrary>>, on_change: Rc<dyn Fn(Option<PaletteView>)>) -> Palettes {
        Palettes { store, library, reached: RefCell::new(None), current: RefCell::new(None), on_change }
    }

    /// The session's palette, with its files: None before one is made or chosen.
    pub async fn view(&self) -> Result<Option<PaletteView>, RuntimeError> {
        let Some(id) = self.current.borrow().clone() else { return Ok(None) };
        let read = self
            .store
            .read(move |connection| {
                let Some(palette) = palettes::by_id(connection, &id)? else { return Ok(None) };
                Ok(Some((palette, palettes::items(connection, &id)?)))
            })
            .await
            .map_err(failed)?;
        let Some((palette, items)) = read else {
            // Forgotten by another session: none now.
            *self.current.borrow_mut() = None;
            return Ok(None);
        };
        let items = self.refound(&palette.id, items).await;
        Ok(Some(view_of(&palette, &items)))
    }

    /// The palette's sounds, each where it is now: one no longer at its path is found again in the library by its file's
    /// name and size (a folder renamed or moved), and kept so. One the library doesn't have stays as it was.
    async fn refound(&self, id: &str, items: Vec<Item>) -> Vec<Item> {
        let paths: Vec<String> = items.iter().map(|item| item.path.clone()).collect();
        let Ok(gone) =
            tokio::task::spawn_blocking(move || paths.iter().map(|path| !std::path::Path::new(path).exists()).collect::<Vec<bool>>()).await
        else {
            return items;
        };
        let mut items = items;
        let missing: Vec<usize> = (0..items.len()).filter(|at| gone[*at] && items[*at].kind == ItemKind::Sound).collect();
        let Some(library) = self.library.as_ref().filter(|_| !missing.is_empty()) else { return items };
        let found = library.refind(missing.iter().map(|at| (file_name(&items[*at].path).to_string(), items[*at].size)).collect()).await;
        for (at, now) in missing.into_iter().zip(found) {
            let Some((path, mtime)) = now else { continue };
            let item = &items[at];
            let (id, was, to) = (id.to_string(), (item.path.clone(), item.size, item.mtime), (path.clone(), mtime));
            let moved = self.store.write(move |connection| palettes::moved(connection, &id, (&was.0, was.1, was.2), (&to.0, to.1))).await;
            if moved.unwrap_or(false) {
                (items[at].path, items[at].mtime) = (path, mtime);
            }
        }
        items
    }

    /// Files into the session's palette (a new one when there's none yet): each read for its size and modification time
    /// off Kumi's thread, as the palette names a file by them (one that can't be read is named by its path alone).
    pub async fn gather(&self, gathered: Vec<Gathered>) -> Result<Gathering, RuntimeError> {
        let now = now_ms();
        let items = tokio::task::spawn_blocking(move || gathered.into_iter().map(|file| item_of(file, now)).collect::<Vec<Item>>())
            .await
            .map_err(|error| RuntimeError::plain(error.to_string()))?;
        let current = self.current.borrow().clone();
        let count = items.len();
        let (palette, added) = self
            .store
            .write(move |connection| {
                let palette = match current.and_then(|id| palettes::by_id(connection, &id).transpose()).transpose()? {
                    Some(palette) => palette,
                    None => {
                        let name = palettes::next_name(connection)?;
                        palettes::create(connection, &name, now)?.expect("a free name")
                    }
                };
                let added = palettes::add(connection, &palette.id, &items, now)?;
                Ok((palette, added))
            })
            .await
            .map_err(failed)?;
        *self.current.borrow_mut() = Some(palette.id.clone());
        self.changed().await;
        Ok(Gathering { palette: palette.name, added, had: count - added })
    }

    /// A file out of the session's palette: whether it had it.
    pub async fn remove(&self, path: &str, size: i64, mtime: i64) -> Result<bool, RuntimeError> {
        let Some(id) = self.current.borrow().clone() else { return Ok(false) };
        let path = path.to_string();
        let removed =
            self.store.write(move |connection| palettes::remove(connection, &id, (&path, size, mtime), now_ms())).await.map_err(failed)?;
        if removed {
            self.changed().await;
        }
        Ok(removed)
    }

    /// /palette and its words, answered in words: alone (or `list`), the palettes; `new <name>` makes one and works
    /// with it; `use <name>` works with another; `rename <name>`; `forget <name>`; `strict` keeps Kumi to the palette's
    /// own sounds; `loose [k]` lets each one bring its k nearest (5 unless said).
    pub async fn command(&self, words: &str) -> Result<String, RuntimeError> {
        let words = words.trim();
        let (verb, rest) = words.split_once(char::is_whitespace).map_or((words, ""), |(verb, rest)| (verb, rest.trim()));
        match verb.to_lowercase().as_str() {
            "" | "list" => self.list().await,
            "new" if !rest.is_empty() => self.create(rest).await,
            "use" if !rest.is_empty() => self.choose(rest).await,
            "rename" if !rest.is_empty() => self.rename(rest).await,
            "forget" if !rest.is_empty() => self.forget(rest).await,
            "strict" if rest.is_empty() => self.reach(0).await,
            "loose" => match rest {
                "" => self.reach(LOOSE).await,
                number => match number.parse::<u32>() {
                    Ok(reach) if (1..=FARTHEST).contains(&reach) => self.reach(reach).await,
                    _ => Ok(format!("/palette loose takes how many of each sound's nearest to bring, from 1 to {FARTHEST}.")),
                },
            },
            _ => Ok(USAGE.into()),
        }
    }

    async fn list(&self) -> Result<String, RuntimeError> {
        let all = self.store.read(|connection| palettes::all(connection)).await.map_err(failed)?;
        if all.is_empty() {
            return Ok("No palettes yet: pick sounds on /map and press Enter, or /palette new and a name.".into());
        }
        let current = self.current.borrow().clone();
        let listed: Vec<String> = all
            .iter()
            .map(|palette| {
                let mark = if Some(&palette.id) == current.as_ref() { ", this session's" } else { "" };
                format!("{} ({}{mark})", palette.name, counted(palette.items))
            })
            .collect();
        Ok(format!("Palettes: {}. /palette use and a name works with another.", listed.join(" · ")))
    }

    async fn create(&self, name: &str) -> Result<String, RuntimeError> {
        let wanted = name.to_string();
        let made = self.store.write(move |connection| palettes::create(connection, &wanted, now_ms())).await.map_err(failed)?;
        let Some(palette) = made else {
            return Ok(format!("There's a palette called {name} already: /palette use {name} works with it."));
        };
        *self.current.borrow_mut() = Some(palette.id.clone());
        self.changed().await;
        Ok(format!("{} is this session's palette now: pick sounds on /map and press Enter to add them.", palette.name))
    }

    async fn choose(&self, name: &str) -> Result<String, RuntimeError> {
        let Some(palette) = self.named(name).await? else { return Ok(missing(name)) };
        *self.current.borrow_mut() = Some(palette.id.clone());
        self.changed().await;
        Ok(format!("{} ({}) is this session's palette now.", palette.name, counted(palette.items)))
    }

    async fn rename(&self, name: &str) -> Result<String, RuntimeError> {
        let Some(id) = self.current.borrow().clone() else { return Ok(NONE.into()) };
        let wanted = name.to_string();
        let renamed = self.store.write(move |connection| palettes::rename(connection, &id, &wanted, now_ms())).await.map_err(failed)?;
        if !renamed {
            return Ok(format!("There's a palette called {name} already."));
        }
        self.changed().await;
        Ok(format!("This session's palette is called {name} now."))
    }

    async fn forget(&self, name: &str) -> Result<String, RuntimeError> {
        let Some(palette) = self.named(name).await? else { return Ok(missing(name)) };
        let id = palette.id.clone();
        self.store.write(move |connection| palettes::forget(connection, &id)).await.map_err(failed)?;
        let was_current = self.current.borrow().as_deref() == Some(palette.id.as_str());
        if was_current {
            *self.current.borrow_mut() = None;
            self.changed().await;
        }
        Ok(format!("Forgot the palette {} and its {}.", palette.name, counted(palette.items)))
    }

    async fn reach(&self, reach: u32) -> Result<String, RuntimeError> {
        let Some(id) = self.current.borrow().clone() else { return Ok(NONE.into()) };
        self.store.write(move |connection| palettes::set_reach(connection, &id, reach, now_ms())).await.map_err(failed)?;
        self.changed().await;
        Ok(match reach {
            0 => "Strict: Kumi keeps to the palette's own sounds.".into(),
            reach => format!("Loose: each of the palette's sounds brings its {reach} nearest, and Kumi says when it uses one of those."),
        })
    }

    async fn named(&self, name: &str) -> Result<Option<Palette>, RuntimeError> {
        let wanted = name.to_string();
        self.store.read(move |connection| palettes::named(connection, &wanted)).await.map_err(failed)
    }

    /// What the session's palette brings in when it's loose: each of its sounds' `reach` nearest in the library (none of
    /// its own or those it excludes), the ones the producer's own projects use first, then those near more of its sounds,
    /// then the nearest. Worked out again only when the palette changed.
    pub async fn reached(&self) -> Result<Rc<Vec<Reached>>, RuntimeError> {
        let Some(view) = self.view().await? else { return Ok(Rc::default()) };
        self.reached_for(&view).await
    }

    async fn reached_for(&self, view: &PaletteView) -> Result<Rc<Vec<Reached>>, RuntimeError> {
        let Some(library) = self.library.clone().filter(|_| view.reach > 0) else { return Ok(Rc::default()) };
        let key = format!(
            "{} {} {:?} {:?}",
            view.id,
            view.reach,
            view.files.iter().map(|file| &file.path).collect::<Vec<_>>(),
            view.excluded.iter().map(|file| &file.path).collect::<Vec<_>>()
        );
        if let Some((kept, reached)) = self.reached.borrow().as_ref() {
            if *kept == key {
                return Ok(reached.clone());
            }
        }
        let Some(index) = library.index().await else { return Ok(Rc::default()) };
        let used = library.used().await;
        let reached = Rc::new(reach_of(view, &index, &used).await);
        *self.reached.borrow_mut() = Some((key, reached.clone()));
        Ok(reached)
    }

    /// What Kumi's turn is told of the session's palette (nothing without one, or with nothing in it): its name, its
    /// sounds by class, and how far Kumi may look past them.
    pub async fn brief(&self) -> String {
        let Some(view) = self.view().await.ok().flatten().filter(|view| !view.files.is_empty()) else { return String::new() };
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        for file in &view.files {
            *classes.entry(file.class.as_deref().unwrap_or(if file.song { "song" } else { "other" })).or_insert(0) += 1;
        }
        let mut classes: Vec<(&str, usize)> = classes.into_iter().collect();
        classes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let classes: Vec<String> = classes.into_iter().map(|(class, count)| format!("{count} {class}")).collect();
        let count = view.files.len();
        let mode = match view.reach {
            0 => "It's strict: when they ask for sounds, use these (palette lists and searches them), and look further only \
                  when they ask you to."
                .to_string(),
            reach => format!(
                "It's loose: these, and each one's {reach} nearest in their library (palette neighbours, the ones their own \
                 projects use first). When you use one of those, say which, and offer to add it to the palette."
            ),
        };
        let excluded = match view.excluded.len() {
            0 => String::new(),
            1 => " Never use the sound it excludes.".into(),
            n => format!(" Never use the {n} sounds it excludes."),
        };
        format!(
            "\n\n[Kumi] The producer's palette, {}: {} ({}). {mode}{excluded}",
            view.name.replace('<', "‹").replace('>', "›"),
            counted(count),
            classes.join(", ")
        )
    }

    /// Sounds into the session's palette by their paths (from the palette tool): named and classed as the library has
    /// them, else by their file's name. What happened, as gathering says it.
    pub async fn add_paths(&self, paths: &[String]) -> Result<Gathering, RuntimeError> {
        let index = match &self.library {
            Some(library) => library.index().await,
            None => None,
        };
        let gathered = paths
            .iter()
            .map(|path| {
                let row =
                    index.as_ref().and_then(|index| index.entries.iter().position(|entry| entry.path == *path).map(|row| (index, row)));
                Gathered {
                    path: path.clone(),
                    name: row.map_or_else(|| file_stem(path).to_string(), |(index, row)| index.name(row).to_string()),
                    song: false,
                    class: row.and_then(|(index, row)| index.entries[row].r#class).map(|class| class.as_str().to_string()),
                }
            })
            .collect();
        self.gather(gathered).await
    }

    /// Sounds out of the session's palette by their paths: how many it had.
    pub async fn remove_paths(&self, paths: &[String]) -> Result<usize, RuntimeError> {
        let Some(view) = self.view().await? else { return Ok(0) };
        let id = view.id.clone();
        let keys: Vec<(String, i64, i64)> =
            view.files.iter().filter(|file| paths.contains(&file.path)).map(|file| (file.path.clone(), file.size, file.mtime)).collect();
        let removed = self
            .store
            .write(move |connection| {
                let mut removed = 0;
                for (path, size, mtime) in &keys {
                    removed += usize::from(palettes::remove(connection, &id, (path, *size, *mtime), now_ms())?);
                }
                Ok(removed)
            })
            .await
            .map_err(failed)?;
        if removed > 0 {
            self.changed().await;
        }
        Ok(removed)
    }

    /// Sounds the session's palette never brings in, by their paths (one of its own stops being one): how many.
    pub async fn exclude_paths(&self, paths: &[String]) -> Result<usize, RuntimeError> {
        let Some(view) = self.view().await? else { return Ok(0) };
        let now = now_ms();
        let items: Vec<Item> = tokio::task::spawn_blocking({
            let paths = paths.to_vec();
            move || {
                paths
                    .into_iter()
                    .map(|path| item_of(Gathered { name: file_stem(&path).to_string(), path, song: false, class: None }, now))
                    .collect()
            }
        })
        .await
        .map_err(|error| RuntimeError::plain(error.to_string()))?;
        // One of its own files: the same file, by the size and time the palette has for it.
        let items: Vec<Item> = items
            .into_iter()
            .map(|mut item| {
                if let Some(own) = view.files.iter().find(|file| file.path == item.path) {
                    (item.size, item.mtime, item.name, item.class) = (own.size, own.mtime, own.name.clone(), own.class.clone());
                }
                item
            })
            .collect();
        let (id, count) = (view.id.clone(), items.len());
        self.store
            .write(move |connection| {
                for item in &items {
                    palettes::exclude(connection, &id, item, true, now)?;
                }
                Ok(())
            })
            .await
            .map_err(failed)?;
        self.changed().await;
        Ok(count)
    }

    /// The app hears the palette as it is now, with what it brings in.
    async fn changed(&self) {
        let mut view = self.view().await.ok().flatten();
        if let Some(view) = view.as_mut() {
            view.reached =
                self.reached_for(view).await.map(|reached| reached.iter().map(|sound| sound.path.clone()).collect()).unwrap_or_default();
        }
        (self.on_change)(view);
    }
}

/// Each of `palette`'s sounds' `reach` nearest in `index` (none of the palette's own, none it excludes), ranked by taste
/// (how many of the producer's projects use each), then by how many of the palette's sounds call it near, then by how near.
/// Read on Kumi's thread a sound at a time, the thread's other work going on between them.
async fn reach_of(palette: &PaletteView, index: &SoundIndex, used: &HashMap<String, usize>) -> Vec<Reached> {
    let reach = palette.reach as usize;
    if reach == 0 {
        return vec![];
    }
    let own: HashSet<&str> = palette.files.iter().chain(&palette.excluded).map(|file| file.path.as_str()).collect();
    let measured: Vec<(usize, &[f32])> = index.compared().collect();
    let place: HashMap<&str, usize> = measured.iter().enumerate().map(|(at, (row, _))| (index.entries[*row].path.as_str(), at)).collect();
    let mut found: HashMap<usize, Reached> = HashMap::new();
    for file in palette.files.iter().filter(|file| !file.song) {
        let Some(at) = place.get(file.path.as_str()).copied() else { continue };
        let here = measured[at].1;
        let mut nearest: Vec<(f32, usize)> = Vec::with_capacity(reach + 1);
        for (other, (row, there)) in measured.iter().enumerate() {
            if other == at || own.contains(index.entries[*row].path.as_str()) || index.entries[*row].gone == Some(true) {
                continue;
            }
            let apart: f32 = here.iter().zip(*there).map(|(a, b)| (a - b) * (a - b)).sum();
            if nearest.len() < reach || apart < nearest[nearest.len() - 1].0 {
                let place = nearest.partition_point(|(seen, _)| *seen <= apart);
                nearest.insert(place, (apart, other));
                nearest.truncate(reach);
            }
        }
        for (apart, other) in nearest {
            let row = measured[other].0;
            let entry = &index.entries[row];
            let reached = found.entry(other).or_insert_with(|| Reached {
                path: entry.path.clone(),
                name: index.name(row).to_string(),
                class: entry.r#class.map(|class| class.as_str().to_string()),
                near: vec![],
                used: used.get(&entry.path).copied().unwrap_or(0),
                apart: f32::INFINITY,
            });
            reached.near.push(file.name.clone());
            reached.apart = reached.apart.min(apart.sqrt());
        }
        tokio::task::yield_now().await;
    }
    let mut reached: Vec<Reached> = found.into_values().collect();
    reached.sort_by(|a, b| {
        b.used.cmp(&a.used).then(b.near.len().cmp(&a.near.len())).then(a.apart.total_cmp(&b.apart)).then(a.path.cmp(&b.path))
    });
    reached
}

const USAGE: &str = "/palette lists your palettes; /palette new <name>, use <name>, rename <name> or forget <name>; /palette strict keeps Kumi to its own sounds, /palette loose [k] lets each bring its k nearest.";
const NONE: &str = "This session has no palette yet: pick sounds on /map and press Enter, or /palette new and a name.";

fn missing(name: &str) -> String {
    format!("There's no palette called {name}: /palette lists them.")
}

/// A path's file name, whichever way its folders are written.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A path's file name without its extension: what a sound is called when nothing names it.
fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// "1 sound", "12 sounds".
fn counted(count: usize) -> String {
    if count == 1 {
        "1 sound".into()
    } else {
        format!("{count} sounds")
    }
}

fn view_of(palette: &Palette, items: &[Item]) -> PaletteView {
    let (excluded, files): (Vec<&Item>, Vec<&Item>) = items.iter().partition(|item| item.excluded);
    PaletteView {
        id: palette.id.clone(),
        name: palette.name.clone(),
        reach: palette.reach,
        files: files.into_iter().map(PaletteFile::from).collect(),
        excluded: excluded.into_iter().map(PaletteFile::from).collect(),
        reached: vec![],
    }
}

/// A gathered file as a palette keeps it: by its path, size and modification time as the library names them (0 where it
/// can't be read), so a sound the library found again is the same file when it's gathered again.
fn item_of(file: Gathered, now: i64) -> Item {
    let (size, mtime) = std::fs::metadata(&file.path)
        .map(|meta| (meta.len() as i64, meta.modified().map_or(0, kumi_library::learn::mtime)))
        .unwrap_or((0, 0));
    Item {
        path: file.path,
        size,
        mtime,
        name: file.name,
        kind: if file.song { ItemKind::Song } else { ItemKind::Sound },
        class: file.class,
        excluded: false,
        added_at: now,
    }
}
