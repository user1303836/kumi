//! Palettes in a session: the sounds and songs the producer gathers to work with (on /map, in the browser's map, or by
//! asking Kumi), kept by name in Kumi's database and shared by every session. A session works with one at a time, its
//! palette: the first thing gathered makes one when there's none, and /palette names, lists and chooses them. The words
//! /palette answers with are made here, the same in every app.

use crate::core::store_client::StoreClient;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::time::now_ms;
use kumi_library::Library;
use kumi_store::palettes::{self, Item, ItemKind, Palette};
use kumi_types::errors::RuntimeError;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, rc::Rc};

/// Where sounds that moved are now, by their file's name and size: each one's path and modification time, or None.
pub type Refind = Rc<dyn Fn(Vec<(String, i64)>) -> LocalBoxFuture<'static, Vec<Option<(String, i64)>>>>;

/// Sounds that moved, found again in the library: a file of the same name and size that's still there.
pub fn library_refind(library: Rc<Library>) -> Refind {
    Rc::new(move |wanted: Vec<(String, i64)>| {
        let library = library.clone();
        async move {
            let Ok(index) = library.sound_index().await else { return vec![None; wanted.len()] };
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
        .boxed_local()
    })
}

/// How far a loose palette reaches when /palette loose names no number: each sound brings its five nearest.
pub const LOOSE: u32 = 5;
/// The most a palette reaches: each sound's nearest past this are hardly like it.
pub const FARTHEST: u32 = 50;

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

/// A session's palettes: kumi.db's, and which one this session works with.
pub struct Palettes {
    store: StoreClient,
    /// Where a sound that moved is found again (by its name and size).
    refind: Option<Refind>,
    current: RefCell<Option<String>>,
    /// Told the palette as it is after each change (None: there's none now).
    on_change: Rc<dyn Fn(Option<PaletteView>)>,
}

fn failed(error: kumi_store::StoreError) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}

impl Palettes {
    pub fn new(store: StoreClient, refind: Option<Refind>, on_change: Rc<dyn Fn(Option<PaletteView>)>) -> Palettes {
        Palettes { store, refind, current: RefCell::new(None), on_change }
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
        let Some(refind) = self.refind.as_ref().filter(|_| !missing.is_empty()) else { return items };
        let found = refind(missing.iter().map(|at| (file_name(&items[*at].path).to_string(), items[*at].size)).collect()).await;
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

    /// The app hears the palette as it is now.
    async fn changed(&self) {
        let view = self.view().await.ok().flatten();
        (self.on_change)(view);
    }
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
    }
}

/// A gathered file as a palette keeps it: by its path, size and modification time (0 where it can't be read).
fn item_of(file: Gathered, now: i64) -> Item {
    let (size, mtime) = std::fs::metadata(&file.path)
        .map(|meta| {
            let mtime =
                meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |at| at.as_millis() as i64);
            (meta.len() as i64, mtime)
        })
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
