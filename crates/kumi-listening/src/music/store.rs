//! What Kumi knows of the producer's songs: `music.db` in its music folder. A cache it can always build again from the
//! folders (deleting it loses nothing a walk and a listen don't bring back), apart from kumi.db because it's large and
//! churns while songs are heard. The scan writes it; the app reads it beside, on connections of its own.

use super::{folders::Walked, tags::Tags};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::{collections::HashMap, fmt, path::Path, time::Duration};

const V1: &str = "
CREATE TABLE songs (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  folder TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  seconds REAL,
  artist TEXT, album_artist TEXT, album TEXT, title TEXT, genre TEXT, year INTEGER, track INTEGER,
  names_from TEXT NOT NULL,
  duplicate_of INTEGER,
  gone INTEGER NOT NULL DEFAULT 0,
  stage INTEGER NOT NULL DEFAULT 0,
  skipped TEXT,
  profile TEXT,
  heard_at INTEGER,
  used_at INTEGER,
  cluster INTEGER,
  x REAL, y REAL
) STRICT;
CREATE INDEX songs_artist ON songs(artist COLLATE NOCASE);
CREATE INDEX songs_album ON songs(album COLLATE NOCASE);
CREATE INDEX songs_genre ON songs(genre COLLATE NOCASE);
CREATE INDEX songs_stage ON songs(stage, mtime);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE vectors_pending (model TEXT NOT NULL, id INTEGER NOT NULL, vector BLOB NOT NULL, PRIMARY KEY (model, id)) STRICT;
";

/// The songs taste used, which the scan hears first, found without walking every song not heard in full; and the
/// songs only in the cloud now, named but not heard until they're on this computer.
const V2: &str = "
CREATE INDEX songs_used ON songs(used_at) WHERE used_at IS NOT NULL;
ALTER TABLE songs ADD COLUMN cloud INTEGER NOT NULL DEFAULT 0;
";

/// Forward-only, as kumi.db's: migration N brings music.db from version N-1 to N (SQLite's `user_version`).
const MIGRATIONS: &[&str] = &[V1, V2];
/// The schema version this build writes.
pub const SCHEMA_VERSION: usize = MIGRATIONS.len();
/// How long a read waits for another process's write (the scan's, the app's) to finish.
const BUSY: Duration = Duration::from_secs(5);
/// Rows written in one transaction.
pub const BATCH: usize = 2_000;

/// How far a song has been heard: by its tags only, lightly (a window and a minute), or in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Tags = 0,
    Light = 1,
    Full = 2,
}

/// Why a song isn't heard.
pub const PROTECTED: &str = "protected";
pub const TOO_SHORT: &str = "too short";
pub const TOO_QUIET: &str = "too quiet";
pub const WONT_DECODE: &str = "won't decode";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicError(pub String);
impl fmt::Display for MusicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for MusicError {}
impl From<rusqlite::Error> for MusicError {
    fn from(error: rusqlite::Error) -> Self {
        MusicError(error.to_string())
    }
}
impl From<std::io::Error> for MusicError {
    fn from(error: std::io::Error) -> Self {
        MusicError(error.to_string())
    }
}

/// A song's file as last walked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub id: i64,
    pub folder: String,
    pub size: u64,
    pub mtime: i64,
    pub gone: bool,
    /// Only in the cloud when it was walked.
    pub cloud: bool,
    /// How far it's heard (`Stage`).
    pub stage: i64,
}

/// A song, as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub id: i64,
    pub path: String,
    pub folder: String,
    pub size: u64,
    pub mtime: i64,
    pub tags: Tags,
    pub duplicate_of: Option<i64>,
    pub gone: bool,
    pub stage: i64,
    pub skipped: Option<String>,
    pub profile: Option<String>,
    pub heard_at: Option<i64>,
    pub used_at: Option<i64>,
    pub cluster: Option<i64>,
    /// Only in the cloud now: not heard until it's on this computer.
    pub cloud: bool,
}

/// How many songs there are, and how far they're heard: songs counts each once (no duplicate, nothing gone).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub songs: usize,
    pub light: usize,
    pub full: usize,
    pub skipped: usize,
    pub duplicates: usize,
    pub gone: usize,
    /// Songs not heard yet that are only in the cloud.
    pub cloud: usize,
}

pub struct MusicStore {
    connection: Connection,
}

impl MusicStore {
    /// Open music.db for writing, making it (and its folder, only the producer's) and bringing its schema up to date, as
    /// kumi.db is opened. A music.db from a newer Kumi isn't opened. A new one starts without the vector files an older
    /// one left (their rows are that one's songs).
    pub fn open(path: &Path) -> Result<MusicStore, MusicError> {
        let made = version_of(path) == Some(0);
        let connection = kumi_store::open_connection(path, MIGRATIONS).map_err(|error| match error {
            kumi_store::StoreError::Newer { found, .. } => newer(found),
            kumi_store::StoreError::Sqlite(text) | kumi_store::StoreError::Io(text) => MusicError(text),
            other => MusicError(other.to_string()),
        })?;
        if made {
            if let Some(folder) = path.parent() {
                super::vectors::remove_files(folder)?;
            }
        }
        Ok(MusicStore { connection })
    }

    /// Open music.db for writing only while it's there: what taste writes after a forget makes no new one.
    pub fn open_existing(path: &Path) -> Result<Option<MusicStore>, MusicError> {
        if !path.is_file() {
            return Ok(None);
        }
        MusicStore::open(path).map(Some)
    }

    /// Open music.db only to read it, writing nothing: None when there's none yet (or it's from a newer Kumi).
    pub fn open_read(path: &Path) -> Result<Option<MusicStore>, MusicError> {
        if !path.is_file() {
            return Ok(None);
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        connection.busy_timeout(BUSY)?;
        let found = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?.max(0) as usize;
        if found == 0 || found > SCHEMA_VERSION {
            return Ok(None);
        }
        Ok(Some(MusicStore { connection }))
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Kumi's music folder, where music.db is: its vector files sit beside it.
    pub fn dir(&self) -> Option<std::path::PathBuf> {
        let path = Path::new(self.connection.path()?);
        path.parent().map(Path::to_path_buf)
    }

    /// Every song's file as last walked, by path.
    pub fn stamps(&self) -> Result<HashMap<String, Stamp>, MusicError> {
        let mut statement = self.connection.prepare("SELECT path, id, folder, size, mtime, gone, cloud, stage FROM songs")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Stamp {
                    id: row.get(1)?,
                    folder: row.get(2)?,
                    size: row.get::<_, i64>(3)? as u64,
                    mtime: row.get(4)?,
                    gone: row.get(5)?,
                    cloud: row.get(6)?,
                    stage: row.get(7)?,
                },
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// New and changed files with their tags, in one transaction: a changed file is a song to hear again (what was
    /// heard of it goes, and its place on the map; its style vectors go with `vectors::forget`). A protected file is
    /// known but never heard. The rows written.
    pub fn write_songs(&mut self, songs: &[(Walked, Tags)]) -> Result<usize, MusicError> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO songs (path, folder, size, mtime, seconds, artist, album_artist, album, title, genre, year, track, names_from, skipped, cloud)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(path) DO UPDATE SET folder = excluded.folder, size = excluded.size, mtime = excluded.mtime,
                   seconds = excluded.seconds, artist = excluded.artist, album_artist = excluded.album_artist, album = excluded.album,
                   title = excluded.title, genre = excluded.genre, year = excluded.year, track = excluded.track,
                   names_from = excluded.names_from, skipped = excluded.skipped, cloud = excluded.cloud, gone = 0, stage = 0,
                   profile = NULL, heard_at = NULL, cluster = NULL, duplicate_of = NULL, x = NULL, y = NULL",
            )?;
            for (file, tags) in songs {
                statement.execute(params![
                    file.path,
                    file.folder,
                    file.size as i64,
                    file.mtime,
                    tags.seconds,
                    tags.artist,
                    tags.album_artist,
                    tags.album,
                    tags.title,
                    tags.genre,
                    tags.year,
                    tags.track,
                    tags.names_from.as_str(),
                    file.protected.then_some(PROTECTED),
                    file.cloud,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(songs.len())
    }

    /// Songs whose files are as they were, now under another of the producer's folders (one around theirs added, or
    /// taken away): their folder and the names read again, in one transaction, and what was heard of them kept. The
    /// rows written.
    pub fn move_songs(&mut self, songs: &[(Walked, Tags)]) -> Result<usize, MusicError> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "UPDATE songs SET folder = ?2, artist = ?3, album_artist = ?4, album = ?5, title = ?6, genre = ?7, year = ?8,
                   track = ?9, names_from = ?10, cloud = ?11, gone = 0 WHERE path = ?1",
            )?;
            for (file, tags) in songs {
                statement.execute(params![
                    file.path,
                    file.folder,
                    tags.artist,
                    tags.album_artist,
                    tags.album,
                    tags.title,
                    tags.genre,
                    tags.year,
                    tags.track,
                    tags.names_from.as_str(),
                    file.cloud,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(songs.len())
    }

    /// Songs found only in the cloud now (or on this computer again), as they are otherwise: the rows changed.
    pub fn set_cloud(&mut self, songs: &[(i64, bool)]) -> Result<usize, MusicError> {
        let transaction = self.connection.transaction()?;
        let mut changed = 0;
        {
            let mut statement = transaction.prepare_cached("UPDATE songs SET cloud = ?2 WHERE id = ?1 AND cloud != ?2")?;
            for (id, cloud) in songs {
                changed += statement.execute(params![id, cloud])?;
            }
        }
        transaction.commit()?;
        Ok(changed)
    }

    /// Every song set aside as gone (the producer took away their last music folder): found as it was, it's back.
    pub fn set_all_gone(&self) -> Result<usize, MusicError> {
        Ok(self.connection.execute("UPDATE songs SET gone = 1 WHERE gone = 0", [])?)
    }

    /// Songs marked gone (or back): the rows changed.
    pub fn set_gone(&mut self, ids: &[i64], gone: bool) -> Result<usize, MusicError> {
        let transaction = self.connection.transaction()?;
        let mut changed = 0;
        {
            let mut statement = transaction.prepare_cached("UPDATE songs SET gone = ?2 WHERE id = ?1 AND gone != ?2")?;
            for id in ids {
                changed += statement.execute(params![id, gone])?;
            }
        }
        transaction.commit()?;
        Ok(changed)
    }

    /// Duplicates found again: the same artist and title (any case) within 2 s of length are one song, kept as a file
    /// that can be heard (a protected one, or one that won't decode, hands over to its copy), its lossless one before
    /// others, then its biggest. The rows changed.
    pub fn resolve_duplicates(&mut self) -> Result<usize, MusicError> {
        struct Row {
            id: i64,
            seconds: Option<f64>,
            playable: bool,
            lossless: bool,
            size: i64,
            was: Option<i64>,
        }
        let mut groups: HashMap<(String, String), Vec<Row>> = HashMap::new();
        let mut alone: Vec<(i64, Option<i64>)> = vec![];
        {
            let mut statement = self
                .connection
                .prepare("SELECT id, artist, title, seconds, path, size, duplicate_of, skipped IS NULL FROM songs WHERE gone = 0")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let (id, artist, title): (i64, Option<String>, Option<String>) = (row.get(0)?, row.get(1)?, row.get(2)?);
                let was: Option<i64> = row.get(6)?;
                let (Some(artist), Some(title)) = (artist, title) else {
                    alone.push((id, was));
                    continue;
                };
                let path: String = row.get(4)?;
                let lossless =
                    [".wav", ".wave", ".aif", ".aiff", ".aifc", ".flac", ".caf"].iter().any(|ext| path.to_lowercase().ends_with(ext));
                groups.entry((folded(&artist), folded(&title))).or_default().push(Row {
                    id,
                    seconds: row.get(3)?,
                    playable: row.get(7)?,
                    lossless,
                    size: row.get(5)?,
                    was,
                });
            }
        }
        let mut wanted: Vec<(i64, Option<i64>, Option<i64>)> = alone.into_iter().map(|(id, was)| (id, was, None)).collect();
        for (_, mut rows) in groups {
            // Songs of unknown length are each their own.
            rows.sort_by(|a, b| a.seconds.unwrap_or(f64::INFINITY).total_cmp(&b.seconds.unwrap_or(f64::INFINITY)).then(a.id.cmp(&b.id)));
            let mut at = 0;
            while at < rows.len() {
                let first = rows[at].seconds;
                let end = match first {
                    Some(first) => at + rows[at..].iter().take_while(|row| row.seconds.is_some_and(|s| s - first <= 2.)).count(),
                    None => at + 1,
                };
                let keeper = rows[at..end]
                    .iter()
                    .max_by(|a, b| {
                        a.playable.cmp(&b.playable).then(a.lossless.cmp(&b.lossless)).then(a.size.cmp(&b.size)).then(b.id.cmp(&a.id))
                    })
                    .map(|row| row.id)
                    .unwrap();
                for row in &rows[at..end] {
                    wanted.push((row.id, row.was, (row.id != keeper).then_some(keeper)));
                }
                at = end;
            }
        }
        let transaction = self.connection.transaction()?;
        let mut changed = 0;
        {
            let mut statement = transaction.prepare_cached("UPDATE songs SET duplicate_of = ?2 WHERE id = ?1")?;
            for (id, was, now) in wanted {
                if was != now {
                    changed += statement.execute(params![id, now])?;
                }
            }
        }
        transaction.commit()?;
        Ok(changed)
    }

    /// How many songs there are, and how far they're heard.
    pub fn counts(&self) -> Result<Counts, MusicError> {
        Ok(self.connection.query_row(
            "SELECT
               count(*) FILTER (WHERE gone = 0 AND duplicate_of IS NULL),
               count(*) FILTER (WHERE gone = 0 AND duplicate_of IS NULL AND skipped IS NULL AND stage = 1),
               count(*) FILTER (WHERE gone = 0 AND duplicate_of IS NULL AND skipped IS NULL AND stage = 2),
               count(*) FILTER (WHERE gone = 0 AND duplicate_of IS NULL AND skipped IS NOT NULL),
               count(*) FILTER (WHERE gone = 0 AND duplicate_of IS NOT NULL),
               count(*) FILTER (WHERE gone = 1),
               count(*) FILTER (WHERE gone = 0 AND duplicate_of IS NULL AND skipped IS NULL AND stage = 0 AND cloud = 1)
             FROM songs",
            [],
            |row| {
                let count = |at| row.get::<_, i64>(at).map(|count| count as usize);
                Ok(Counts {
                    songs: count(0)?,
                    light: count(1)?,
                    full: count(2)?,
                    skipped: count(3)?,
                    duplicates: count(4)?,
                    gone: count(5)?,
                    cloud: count(6)?,
                })
            },
        )?)
    }

    /// One song by its id.
    pub fn song(&self, id: i64) -> Result<Option<Song>, MusicError> {
        Ok(self.connection.query_row(&format!("SELECT {SONG_COLUMNS} FROM songs WHERE id = ?1"), [id], song_row).optional()?)
    }

    /// One song by its path.
    pub fn song_at(&self, path: &str) -> Result<Option<Song>, MusicError> {
        Ok(self.connection.query_row(&format!("SELECT {SONG_COLUMNS} FROM songs WHERE path = ?1"), [path], song_row).optional()?)
    }

    /// Songs that meet `condition` (SQL over the songs table, on top of being heard or hearable), in `order`, at most
    /// `limit`.
    pub fn songs_where(&self, condition: &str, order: &str, limit: usize) -> Result<Vec<Song>, MusicError> {
        let sql = format!("SELECT {SONG_COLUMNS} FROM songs WHERE {LIVE} AND ({condition}) ORDER BY {order} LIMIT {limit}");
        let mut statement = self.connection.prepare(&sql)?;
        let songs = statement.query_map([], song_row)?.collect::<Result<_, _>>()?;
        Ok(songs)
    }

    /// Songs by their ids, in no order (those gone, duplicates or skipped left out).
    pub fn songs_by_id(&self, ids: &[i64]) -> Result<Vec<Song>, MusicError> {
        let mut songs = vec![];
        for chunk in ids.chunks(500) {
            let list = chunk.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            songs.extend(self.songs_where(&format!("id IN ({list})"), "id", chunk.len())?);
        }
        Ok(songs)
    }

    /// A song heard at a stage: its profile (JSON), and when.
    pub fn heard(&self, id: i64, stage: Stage, profile: &str, at: i64) -> Result<(), MusicError> {
        self.connection.execute(
            "UPDATE songs SET stage = ?2, profile = ?3, heard_at = ?4, skipped = NULL WHERE id = ?1",
            params![id, stage as i64, profile, at],
        )?;
        Ok(())
    }

    /// A song that won't be heard, and why.
    pub fn skip(&self, id: i64, why: &str, at: i64) -> Result<(), MusicError> {
        self.connection.execute("UPDATE songs SET skipped = ?2, heard_at = ?3 WHERE id = ?1", params![id, why, at])?;
        Ok(())
    }

    /// The songs heard at least lightly, by id.
    pub fn heard_ids(&self) -> Result<Vec<i64>, MusicError> {
        let mut statement = self.connection.prepare(&format!("SELECT id FROM songs WHERE {LIVE} AND stage >= 1 ORDER BY id"))?;
        let ids = statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
        Ok(ids)
    }

    /// Each heard song's cluster (the rest none). Only the songs whose cluster changed are written, `BATCH` to a
    /// transaction, so the app's own writes never wait long behind them.
    pub fn set_clusters(&mut self, clusters: &[(i64, usize)]) -> Result<(), MusicError> {
        let mut was: HashMap<i64, i64> = {
            let mut statement = self.connection.prepare("SELECT id, cluster FROM songs WHERE cluster IS NOT NULL")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut changed: Vec<(i64, Option<i64>)> = vec![];
        for (id, cluster) in clusters {
            if was.remove(id) != Some(*cluster as i64) {
                changed.push((*id, Some(*cluster as i64)));
            }
        }
        changed.extend(was.into_keys().map(|id| (id, None)));
        self.in_batches(&changed, "UPDATE songs SET cluster = ?2 WHERE id = ?1", |statement, (id, cluster)| {
            statement.execute(params![id, cluster])
        })
    }

    /// Each laid out song's place on the map (x, y in [0, 1]); every other song has none. Only the places that changed
    /// are written, as `set_clusters` writes.
    pub fn set_places(&mut self, places: &[(i64, [f32; 2])]) -> Result<(), MusicError> {
        let mut was: HashMap<i64, (Option<f64>, Option<f64>)> = {
            let mut statement = self.connection.prepare("SELECT id, x, y FROM songs WHERE x IS NOT NULL OR y IS NOT NULL")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut changed: Vec<(i64, Option<f64>, Option<f64>)> = vec![];
        for (id, [x, y]) in places {
            let place = (Some(*x as f64), Some(*y as f64));
            if was.remove(id) != Some(place) {
                changed.push((*id, place.0, place.1));
            }
        }
        changed.extend(was.into_keys().map(|id| (id, None, None)));
        self.in_batches(&changed, "UPDATE songs SET x = ?2, y = ?3 WHERE id = ?1", |statement, (id, x, y)| {
            statement.execute(params![id, x, y])
        })
    }

    /// `rows` written with `sql`, `BATCH` to a transaction.
    fn in_batches<T>(
        &mut self,
        rows: &[T],
        sql: &str,
        write: impl Fn(&mut rusqlite::CachedStatement, &T) -> rusqlite::Result<usize>,
    ) -> Result<(), MusicError> {
        for batch in rows.chunks(BATCH) {
            let transaction = self.connection.transaction()?;
            {
                let mut statement = transaction.prepare_cached(sql)?;
                for row in batch {
                    write(&mut statement, row)?;
                }
            }
            transaction.commit()?;
        }
        Ok(())
    }

    /// Taste used these songs now: they're heard in full before others.
    pub fn mark_used(&self, ids: &[i64], at: i64) -> Result<(), MusicError> {
        for chunk in ids.chunks(500) {
            let list = chunk.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            self.connection.execute(&format!("UPDATE songs SET used_at = ?1 WHERE id IN ({list})"), [at])?;
        }
        Ok(())
    }

    /// The heard songs a name names, and what it named: an artist (or album artist), an album, a song's title (or
    /// "artist – title"), or a genre tag, tried in that order, any case.
    pub fn named(&self, name: &str) -> Result<Option<(Named, Vec<Song>)>, MusicError> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        let heard = |condition: &str, values: &[&str]| -> Result<Vec<Song>, MusicError> {
            let sql = format!("SELECT {SONG_COLUMNS} FROM songs WHERE {LIVE} AND stage >= 1 AND ({condition}) ORDER BY id");
            let mut statement = self.connection.prepare(&sql)?;
            let songs = statement.query_map(rusqlite::params_from_iter(values), song_row)?.collect::<Result<_, _>>()?;
            Ok(songs)
        };
        let artist = heard("artist = ?1 COLLATE NOCASE OR album_artist = ?1 COLLATE NOCASE", &[name])?;
        if !artist.is_empty() {
            return Ok(Some((Named::Artist, artist)));
        }
        let album = heard("album = ?1 COLLATE NOCASE", &[name])?;
        if !album.is_empty() {
            return Ok(Some((Named::Album, album)));
        }
        let mut song = heard("title = ?1 COLLATE NOCASE", &[name])?;
        if song.is_empty() {
            if let Some((by, title)) = name.split_once(" – ").or_else(|| name.split_once(" - ")) {
                song = heard(
                    "(artist = ?1 COLLATE NOCASE OR album_artist = ?1 COLLATE NOCASE) AND title = ?2 COLLATE NOCASE",
                    &[by.trim(), title.trim()],
                )?;
            }
        }
        if !song.is_empty() {
            return Ok(Some((Named::Song, song)));
        }
        let wanted = folded(name);
        let genre: Vec<Song> = heard("genre LIKE ?1", &[&format!("%{name}%")])?
            .into_iter()
            .filter(|song| song.tags.genre.as_deref().is_some_and(|genre| genre.split("; ").any(|tag| folded(tag) == wanted)))
            .collect();
        Ok((!genre.is_empty()).then_some((Named::Genre, genre)))
    }

    /// The genre tags of the heard songs, each with how many songs carry it (any case: as most of them spell it).
    pub fn genres(&self) -> Result<Vec<(String, usize)>, MusicError> {
        let mut statement = self
            .connection
            .prepare(&format!("SELECT genre, count(*) FROM songs WHERE {LIVE} AND stage >= 1 AND genre IS NOT NULL GROUP BY genre"))?;
        let rows: Vec<(String, i64)> = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?;
        let mut tags: indexmap::IndexMap<String, (usize, String, usize)> = indexmap::IndexMap::new();
        for (genre, count) in rows {
            for tag in genre.split("; ").map(str::trim).filter(|tag| !tag.is_empty()) {
                let entry = tags.entry(folded(tag)).or_insert((0, tag.to_string(), 0));
                entry.0 += count as usize;
                if count as usize > entry.2 {
                    (entry.1, entry.2) = (tag.to_string(), count as usize);
                }
            }
        }
        Ok(tags.into_values().map(|(count, spelling, _)| (spelling, count)).collect())
    }

    /// The songs on the map (heard and laid out, none gone), by id, and the producer's styles, numbered as the songs'
    /// clusters are.
    pub fn map(&self) -> Result<(Vec<MapSong>, Vec<MapStyle>), MusicError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT id, x, y, cluster, path, artist, title, genre, seconds FROM songs \
             WHERE {LIVE} AND x IS NOT NULL AND y IS NOT NULL ORDER BY id"
        ))?;
        let songs = statement
            .query_map([], |row| {
                let path: String = row.get(4)?;
                let (artist, title): (Option<String>, Option<String>) = (row.get(5)?, row.get(6)?);
                Ok(MapSong {
                    id: row.get(0)?,
                    x: row.get(1)?,
                    y: row.get(2)?,
                    cluster: row.get::<_, Option<i64>>(3)?.and_then(|cluster| usize::try_from(cluster).ok()),
                    name: song_name(artist.as_deref(), title.as_deref(), &path),
                    path,
                    genre: row.get(7)?,
                    seconds: row.get(8)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        #[derive(serde::Deserialize)]
        struct Kept {
            #[serde(default)]
            clusters: Vec<MapStyle>,
        }
        let styles = self.meta("clusters")?.and_then(|text| serde_json::from_str::<Kept>(&text).ok()).map(|kept| kept.clusters);
        Ok((songs, styles.unwrap_or_default()))
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, MusicError> {
        Ok(self.connection.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), MusicError> {
        self.connection
            .execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value])?;
        Ok(())
    }
}

/// A song on the map: where it sits (x and y in [0, 1]), its style, and what the map shows of it.
#[derive(Debug, Clone, PartialEq)]
pub struct MapSong {
    pub id: i64,
    pub x: f64,
    pub y: f64,
    /// Its style, numbered from 0 (the style c1 is 0).
    pub cluster: Option<usize>,
    pub path: String,
    /// "Artist – Title", or its file's name.
    pub name: String,
    pub genre: Option<String>,
    pub seconds: Option<f64>,
}

/// One of the producer's styles (a cluster of their songs), as the scan last kept it.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct MapStyle {
    #[serde(default)]
    pub songs: usize,
    /// The songs nearest its centre, by id.
    #[serde(default)]
    pub examples: Vec<i64>,
    /// Its commonest genre tags.
    #[serde(default)]
    pub genres: Vec<String>,
    /// Its two words, once CLAP's text half named it.
    #[serde(default)]
    pub names: Vec<String>,
}

/// A song's name: "Artist – Title", its title alone, or its file's name.
pub fn song_name(artist: Option<&str>, title: Option<&str>, path: &str) -> String {
    match (artist, title) {
        (Some(artist), Some(title)) => format!("{artist} – {title}"),
        (None, Some(title)) => title.to_string(),
        _ => path.rsplit(['/', '\\']).next().unwrap_or("").to_string(),
    }
}

/// What a name named among the producer's songs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    Artist,
    Album,
    Song,
    Genre,
}

/// A song that can be heard (or was): known, not gone, not a duplicate, not skipped.
pub const LIVE: &str = "gone = 0 AND duplicate_of IS NULL AND skipped IS NULL";

/// The columns `song_row` reads, in its order.
pub const SONG_COLUMNS: &str =
    "id, path, folder, size, mtime, seconds, artist, album_artist, album, title, genre, year, track, names_from, \
     duplicate_of, gone, stage, skipped, profile, heard_at, used_at, cluster, cloud";

/// A song from a row of `SONG_COLUMNS`.
pub fn song_row(row: &rusqlite::Row) -> rusqlite::Result<Song> {
    Ok(Song {
        id: row.get(0)?,
        path: row.get(1)?,
        folder: row.get(2)?,
        size: row.get::<_, i64>(3)? as u64,
        mtime: row.get(4)?,
        tags: Tags {
            seconds: row.get(5)?,
            artist: row.get(6)?,
            album_artist: row.get(7)?,
            album: row.get(8)?,
            title: row.get(9)?,
            genre: row.get(10)?,
            year: row.get(11)?,
            track: row.get(12)?,
            names_from: super::tags::NamesFrom::parse(&row.get::<_, String>(13)?),
        },
        duplicate_of: row.get(14)?,
        gone: row.get(15)?,
        stage: row.get(16)?,
        skipped: row.get(17)?,
        profile: row.get(18)?,
        heard_at: row.get(19)?,
        used_at: row.get(20)?,
        cluster: row.get(21)?,
        cloud: row.get(22)?,
    })
}

/// A music.db's schema version, making nothing: 0 when there's none yet, None when it can't be read now.
fn version_of(path: &Path) -> Option<usize> {
    if !path.exists() {
        return Some(0);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).ok().map(|version| version.max(0) as usize)
}

fn newer(found: usize) -> MusicError {
    MusicError(format!("music.db is from a newer Kumi (schema {found}; this one knows {SCHEMA_VERSION})"))
}

/// A name as compared: one case, single spaces.
pub fn folded(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_music_db_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("music.db");
        drop(MusicStore::open(&path).unwrap());
        Connection::open(&path).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION as i64 + 1).unwrap();
        assert_eq!(MusicStore::open(&path).err(), Some(newer(SCHEMA_VERSION + 1)));
        assert!(MusicStore::open_read(&path).unwrap().is_none());
    }
}
