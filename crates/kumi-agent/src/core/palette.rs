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
use kumi_types::{daw::DawId, errors::RuntimeError};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::{Rc, Weak},
};

/// What a palette works with in the producer's library: sounds that moved found again, the library's sounds (their
/// numbers, for a loose palette's nearest), and how many of the producer's own projects use each sound, for taste.
#[async_trait(?Send)]
pub trait PaletteLibrary {
    /// Sounds that moved, by their file's name and size: each one's path and modification time now, or None.
    async fn refind(&self, wanted: Vec<(String, i64)>) -> Vec<Option<(String, i64)>>;
    /// The library's sounds as the search compares them: None when they can't be read.
    async fn index(&self) -> Option<Rc<SoundIndex>>;
    /// How many indexes the library has built, read without building: while it stays the same, `index` would hand
    /// out the one it did, so what a palette worked out over it holds without asking.
    fn indexes_built(&self) -> u64;
    /// How many of the producer's projects use each sound, by its path.
    async fn used(&self) -> HashMap<String, usize>;
}

/// The producer's library, for palettes: its sounds as a session with this DAW is offered them (Live's when none is
/// known), the index the session's search reads too.
pub struct LibrarySounds(pub Rc<Library>, pub Option<DawId>);

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
        self.0.sound_index_in(self.1).await.ok()
    }
    fn indexes_built(&self) -> u64 {
        self.0.sound_indexes_built()
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
    /// What the palette brought in last, what it was worked out for, and its sounds' nearest, kept for the next change.
    /// One look at a time, held while the app is told of a change: a later change starts from what an earlier one kept,
    /// and the app hears them in order.
    reached: tokio::sync::Mutex<Option<Reaching>>,
    current: RefCell<Option<String>>,
    /// Told the palette as it is after each change (None: there's none now).
    on_change: Rc<dyn Fn(Option<PaletteView>)>,
}

fn failed(error: kumi_store::StoreError) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}

impl Palettes {
    pub fn new(store: StoreClient, library: Option<Rc<dyn PaletteLibrary>>, on_change: Rc<dyn Fn(Option<PaletteView>)>) -> Palettes {
        Palettes { store, library, reached: tokio::sync::Mutex::new(None), current: RefCell::new(None), on_change }
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
    /// then the nearest. Worked out again only when the palette or the library's sounds changed, and then only as far as
    /// the change needs.
    pub async fn reached(&self) -> Result<Rc<Vec<Reached>>, RuntimeError> {
        let mut kept = self.reached.lock().await;
        let Some(view) = self.view().await? else { return Ok(Rc::default()) };
        self.reached_for(&view, &mut kept).await
    }

    /// `reached`, for `view`, with what was kept (its lock held).
    async fn reached_for(&self, view: &PaletteView, kept: &mut Option<Reaching>) -> Result<Rc<Vec<Reached>>, RuntimeError> {
        let Some(library) = self.library.clone().filter(|_| view.reach > 0) else { return Ok(Rc::default()) };
        let key = format!(
            "{} {} {:?} {:?}",
            view.id,
            view.reach,
            view.files.iter().map(|file| &file.path).collect::<Vec<_>>(),
            view.excluded.iter().map(|file| &file.path).collect::<Vec<_>>()
        );
        // The palette as it was, with no index built since: what it brought then, without asking the library.
        let built = library.indexes_built();
        if let Some(kept) = kept.as_ref().filter(|kept| kept.key == key && kept.built == built) {
            return Ok(kept.reached.clone());
        }
        // An index that can't be read keeps what this palette brought last.
        let Some(index) = library.index().await else {
            return Ok(kept.as_ref().filter(|kept| kept.key == key).map(|kept| kept.reached.clone()).unwrap_or_default());
        };
        // Asking may have built one: the count after it is the one this index goes with.
        let built = library.indexes_built();
        let used = library.used().await;
        // Brought up to date in place: a look stopped partway leaves what it did, which the next one carries on from.
        let reaching = kept.get_or_insert_with(Reaching::default);
        let reached = Rc::new(reaching.nearest.reach(view, &index, &used).await);
        (reaching.key, reaching.built, reaching.reached) = (key, built, reached.clone());
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

    /// The app hears the palette as it is now, with what it brings in: one change after another (the lock held from
    /// reading the palette to telling the app), so the last it hears is the palette as the last change left it.
    async fn changed(&self) {
        let mut kept = self.reached.lock().await;
        let mut view = self.view().await.ok().flatten();
        if let Some(view) = view.as_mut() {
            view.reached = self
                .reached_for(view, &mut kept)
                .await
                .map(|reached| reached.iter().map(|sound| sound.path.clone()).collect())
                .unwrap_or_default();
        }
        (self.on_change)(view);
    }
}

/// What a loose palette brought in, what it was worked out for (its files, its excluded sounds, its reach, and how many
/// indexes the library had built), and its sounds' nearest then.
#[derive(Default)]
struct Reaching {
    key: String,
    built: u64,
    reached: Rc<Vec<Reached>>,
    nearest: Nearest,
}

/// A sound's nearest: how far apart (squared) and where, by its place among the index's measured sounds (`compared`).
type Near = (f32, usize);

/// Each of a loose palette's sounds' nearest in the library, kept from one change to the next. A change looks through the
/// library again only for the sounds new to the palette, and for one left with fewer than the palette brings when it
/// takes their nearest in; an index with other sounds (sounds learned since) for every one.
#[derive(Default)]
struct Nearest {
    /// The library's sounds they're among, held weakly: the library makes its next index from this one when nothing else
    /// holds it.
    index: Weak<SoundIndex>,
    /// What they depend on in that index (`print`): another index with the same keeps them.
    print: u64,
    /// The sounds the palette had or excluded, by their places.
    own: HashSet<usize>,
    /// By a palette sound's place: the nearest it can bring in (none the palette has or excludes, none gone), nearest
    /// first, and whether they're every one there is.
    lists: HashMap<usize, (Vec<Near>, bool)>,
}

impl Nearest {
    /// Whether these are among `index`'s sounds.
    fn among(&self, index: &Rc<SoundIndex>) -> bool {
        std::ptr::eq(self.index.as_ptr(), Rc::as_ptr(index))
    }

    /// Each of `palette`'s sounds' `reach` nearest in `index` (none of the palette's own, none it excludes), ranked by
    /// taste (how many of the producer's projects use each), then by how many of the palette's sounds call it near, then
    /// by how near. Brought up to date first, as far as the change needs: the library is looked through on Kumi's thread a
    /// sound at a time, the thread's other work going on between them.
    async fn reach(&mut self, palette: &PaletteView, index: &Rc<SoundIndex>, used: &HashMap<String, usize>) -> Vec<Reached> {
        let reach = palette.reach as usize;
        if reach == 0 {
            return vec![];
        }
        // Another index: the same sounds again (the library builds one after each pass, and for sounds it hides here) keep
        // what's kept; other sounds, or other numbers for them, start afresh.
        if !self.among(index) {
            let print = print(index);
            if print != self.print {
                *self = Nearest { print, ..Nearest::default() };
            }
            self.index = Rc::downgrade(index);
        }
        let measured: Vec<(usize, &[f32])> = index.compared().collect();
        let place: HashMap<&str, usize> =
            measured.iter().enumerate().map(|(at, (row, _))| (index.entries[*row].path.as_str(), at)).collect();
        let own: HashSet<usize> =
            palette.files.iter().chain(&palette.excluded).filter_map(|file| place.get(file.path.as_str()).copied()).collect();
        let sounds: Vec<(&PaletteFile, usize)> =
            palette.files.iter().filter(|file| !file.song).filter_map(|file| place.get(file.path.as_str()).map(|at| (file, *at))).collect();
        // As many again to spare: the palette taking in one of a sound's nearest seldom leaves it too few.
        let keep = 2 * reach;
        // Let go since (taken out): brought again.
        let freed: Vec<usize> = self.own.difference(&own).copied().filter(|at| index.entries[measured[*at].0].gone != Some(true)).collect();
        let wanted: HashSet<usize> = sounds.iter().map(|(_, at)| *at).collect();
        self.lists.retain(|at, _| wanted.contains(at));
        let mut again = vec![];
        for (&at, (list, whole)) in self.lists.iter_mut() {
            // None the palette has or excludes now (taken in since), and no more than it brings now.
            list.retain(|(_, other)| !own.contains(other));
            if list.len() > keep {
                list.truncate(keep);
                *whole = false;
            }
            // Each one let go, once (a look stopped partway may have put it in already).
            for &other in &freed {
                if !list.iter().any(|(_, there)| *there == other) {
                    offer(list, whole, (apart(measured[at].1, measured[other].1), other), keep);
                }
            }
            if list.len() < reach && !*whole {
                again.push(at);
            }
            if !freed.is_empty() {
                tokio::task::yield_now().await;
            }
        }
        self.own = own;
        let new: Vec<usize> = wanted.into_iter().filter(|at| !self.lists.contains_key(at)).collect();
        for at in again.into_iter().chain(new) {
            let looked = look(index, &measured, at, &self.own, keep);
            self.lists.insert(at, looked);
            tokio::task::yield_now().await;
        }
        let mut found: HashMap<usize, Reached> = HashMap::new();
        for (file, at) in &sounds {
            for &(apart, other) in self.lists[at].0.iter().take(reach) {
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
        }
        let mut reached: Vec<Reached> = found.into_values().collect();
        reached.sort_by(|a, b| {
            b.used.cmp(&a.used).then(b.near.len().cmp(&a.near.len())).then(a.apart.total_cmp(&b.apart)).then(a.path.cmp(&b.path))
        });
        reached
    }
}

/// The `keep` nearest the sound at `at` can bring in among `measured` (none of `own`, which has it, none gone), nearest
/// first, and whether they're every one there is.
fn look(index: &SoundIndex, measured: &[(usize, &[f32])], at: usize, own: &HashSet<usize>, keep: usize) -> (Vec<Near>, bool) {
    let here = measured[at].1;
    let (mut nearest, mut whole) = (Vec::with_capacity(keep + 1), true);
    for (other, (row, there)) in measured.iter().enumerate() {
        if own.contains(&other) || index.entries[*row].gone == Some(true) {
            continue;
        }
        offer(&mut nearest, &mut whole, (apart(here, there), other), keep);
    }
    (nearest, whole)
}

/// One more sound offered to a sound's nearest, which hold the nearest of what it can bring in (`whole`: every one):
/// it goes where it belongs, unless it's past the last of nearest that aren't whole; over `keep`, the last goes. A look
/// and a change's update both go through here, so what an update keeps is what a look would find.
fn offer(nearest: &mut Vec<Near>, whole: &mut bool, near: Near, keep: usize) {
    if !*whole && !nearest.last().is_some_and(|last| before(near, *last)) {
        return;
    }
    let place = nearest.partition_point(|seen| before(*seen, near));
    nearest.insert(place, near);
    if nearest.len() > keep {
        nearest.pop();
        *whole = false;
    }
}

/// What a sound's nearest depend on in `index`: each measured sound's path, whether it's gone, and its numbers, in their
/// order. Two indexes with the same give every sound the same nearest.
fn print(index: &SoundIndex) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (row, numbers) in index.compared() {
        index.entries[row].path.hash(&mut hasher);
        (index.entries[row].gone == Some(true)).hash(&mut hasher);
        for number in numbers {
            hasher.write_u32(number.to_bits());
        }
    }
    hasher.finish()
}

/// How far apart two sounds are (squared), in the numbers the search compares.
fn apart(here: &[f32], there: &[f32]) -> f32 {
    here.iter().zip(there).map(|(a, b)| (a - b) * (a - b)).sum()
}

/// Whether one of a sound's nearest comes before another: nearer, or as near and earlier in the index.
fn before(one: Near, other: Near) -> bool {
    one.0 < other.0 || (one.0 == other.0 && one.1 < other.1)
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

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_library::{
        learn::SoundEntry,
        sources::{Source, SourceKind},
        store::{pack_vector, Entry},
    };

    /// A library of `kicks` kicks, then `pads` pads: each kind alike, kick n n steps from kick 0, pad n from pad 0.
    fn library(kicks: usize, pads: usize) -> Rc<SoundIndex> {
        let entries = [("Kick", kicks), ("Pad", pads)].into_iter().flat_map(|(kind, count)| {
            (0..count).map(move |n| {
                let numbers: Vec<f32> = (0..kumi_library::features::VECTOR_LENGTH)
                    .map(|d| if (d % 2 == 0) == (kind == "Kick") { 5. } else { 0. } + d as f32 * 0.001 * n as f32)
                    .collect();
                let file = Entry { path: format!("/Samples/{kind} {n}.wav"), size: 10, mtime: 1, gone: None };
                SoundEntry { file, vector: Some(pack_vector(&numbers)), ..Default::default() }
            })
        });
        let source = Source { path: "/Samples".into(), label: "Samples".into(), kind: SourceKind::Folder };
        Rc::new(SoundIndex::of_every_source(entries, &[source]))
    }

    fn file(name: &str) -> PaletteFile {
        PaletteFile { path: format!("/Samples/{name}.wav"), size: 10, mtime: 1, name: name.into(), song: false, class: None }
    }

    /// A palette reaching `reach`, with these sounds, excluding those.
    fn palette(reach: u32, sounds: &[&str], excluded: &[&str]) -> PaletteView {
        PaletteView {
            id: "1".into(),
            name: "Palette 1".into(),
            reach,
            files: sounds.iter().map(|name| file(name)).collect(),
            excluded: excluded.iter().map(|name| file(name)).collect(),
            reached: vec![],
        }
    }

    /// Where each palette sound's nearest are kept, by its name.
    fn kept(nearest: &Nearest, index: &SoundIndex) -> HashMap<String, usize> {
        let measured: Vec<(usize, &[f32])> = index.compared().collect();
        nearest.lists.iter().map(|(at, (list, _))| (index.name(measured[*at].0).to_string(), list.as_ptr() as usize)).collect()
    }

    /// `nearest` brought up to date for `palette`: it brings what a fresh look brings, having looked through the library
    /// again only for `looked` (every other sound's nearest kept where they were).
    async fn step(nearest: &mut Nearest, index: &Rc<SoundIndex>, palette: PaletteView, looked: &[&str]) {
        let (before, used) = (kept(nearest, index), HashMap::new());
        let reached = nearest.reach(&palette, index, &used).await;
        assert_eq!(reached, Nearest::default().reach(&palette, index, &used).await, "as a fresh look");
        let mut again: Vec<String> =
            kept(nearest, index).into_iter().filter(|(name, at)| before.get(name) != Some(at)).map(|(name, _)| name).collect();
        again.sort();
        assert_eq!(again, looked, "looked for again");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_change_looks_through_the_library_again_only_for_the_sounds_it_needs_and_brings_what_a_fresh_look_does() {
        let index = library(12, 6);
        let mut nearest = Nearest::default();
        let sounds = ["Kick 0", "Pad 0", "Pad 5"];
        step(&mut nearest, &index, palette(2, &sounds[..2], &[]), &["Kick 0", "Pad 0"]).await;
        // New to it: only that one, Kick 1 though it's among Kick 0's nearest (Kick 0 has more to spare).
        step(&mut nearest, &index, palette(2, &sounds, &[]), &["Pad 5"]).await;
        step(&mut nearest, &index, palette(2, &[&sounds[..], &["Kick 1"][..]].concat(), &[]), &["Kick 1"]).await;
        // Taken out, it's among Kick 0's nearest again; another excluded, it's not.
        step(&mut nearest, &index, palette(2, &sounds, &[]), &[]).await;
        step(&mut nearest, &index, palette(2, &sounds, &["Kick 2"]), &[]).await;
        // Kick 0 is left with one of its nearest when the palette takes two more of them: looked for again.
        let more = [&sounds[..], &["Kick 3", "Kick 4"][..]].concat();
        step(&mut nearest, &index, palette(2, &more, &["Kick 2"]), &["Kick 0", "Kick 3", "Kick 4"]).await;
        // Reaching further, within what each keeps to spare, and nearer.
        step(&mut nearest, &index, palette(3, &more, &["Kick 2"]), &[]).await;
        step(&mut nearest, &index, palette(1, &more, &["Kick 2"]), &[]).await;
        // The same sounds in another index, as the library builds after each pass (the one before let go first, as the
        // library lets it go): every one's nearest kept.
        drop(index);
        let index = library(12, 6);
        assert!(!nearest.among(&index));
        step(&mut nearest, &index, palette(2, &more, &["Kick 2"]), &[]).await;
        assert!(nearest.among(&index));
        // Sounds learned since (another index): every one looked for again.
        let learned = library(14, 6);
        let reached = nearest.reach(&palette(2, &more, &["Kick 2"]), &learned, &HashMap::new()).await;
        assert_eq!(reached, Nearest::default().reach(&palette(2, &more, &["Kick 2"]), &learned, &HashMap::new()).await);
        assert!(nearest.among(&learned) && !nearest.among(&index));
        assert_eq!(nearest.lists.len(), more.len());

        // A library with fewer than a sound keeps: they're all there, so one let go goes among them however far it is.
        let index = library(3, 1);
        let mut nearest = Nearest::default();
        step(&mut nearest, &index, palette(3, &["Kick 0", "Pad 0"], &[]), &["Kick 0", "Pad 0"]).await;
        step(&mut nearest, &index, palette(3, &["Kick 0"], &[]), &[]).await;
        let reached = nearest.reach(&palette(3, &["Kick 0"], &[]), &index, &HashMap::new()).await;
        assert_eq!(reached.iter().map(|sound| sound.name.as_str()).collect::<Vec<_>>(), ["Kick 1", "Kick 2", "Pad 0"]);
    }
}
