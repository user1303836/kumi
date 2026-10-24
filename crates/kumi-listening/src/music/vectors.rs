//! The songs' style vectors (CLAP's 512 numbers), one file for each style model: `vectors-<model>.i8` in the music
//! folder. A header (magic, version, dimensions, the quantization scale and the model's `style_id`), then 512 signed
//! bytes a song at `HEADER + id × 512`: a row of zeros is a song not embedded yet. Read through a memory map, never
//! loaded whole (300,000 songs are 150 MB).
//!
//! A file has one scale, set from its first 1,000 vectors: the 99.9th percentile of |value| maps to 127, and beyond it
//! is clamped. Until then the vectors wait in music.db (`vectors_pending`), as they are.

use super::store::{MusicError, MusicStore};
use memmap2::Mmap;
use rusqlite::params;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// A style vector's numbers.
pub const DIMS: usize = 512;
/// The header's bytes, before the first row.
pub const HEADER: usize = 256;
const MAGIC: &[u8; 8] = b"KUMIVEC\0";
const VERSION: u32 = 1;
/// The longest model id the header holds as it is (a longer one, a slot's file path, is kept as its hash).
const ID_BYTES: usize = HEADER - 24;
/// The vectors that set a file's scale.
pub const SCALE_FROM: usize = 1_000;

/// The vector file for a style model.
pub fn file_for(music_dir: &Path, model: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    music_dir.join(format!("vectors-{}.i8", &hex::encode(Sha256::digest(model.as_bytes()))[..16]))
}

/// The id a header keeps for a model.
fn header_id(model: &str) -> String {
    if model.len() <= ID_BYTES {
        return model.to_string();
    }
    use sha2::{Digest, Sha256};
    format!("sha256-of:{}", hex::encode(Sha256::digest(model.as_bytes())))
}

/// The scale for a file from its first vectors: what the 99.9th percentile of |value| is (it maps to 127).
pub fn scale_of(vectors: &[Vec<f32>]) -> f32 {
    let mut values: Vec<f32> = vectors.iter().flatten().map(|value| value.abs()).filter(|value| value.is_finite()).collect();
    if values.is_empty() {
        return 1.;
    }
    values.sort_by(f32::total_cmp);
    let at = ((values.len() as f64 * 0.999).ceil() as usize).clamp(1, values.len()) - 1;
    values[at].max(1e-6)
}

/// A vector at `DIMS` numbers, as every file row and waiting vector is kept: a style model of another size (one a slot
/// runs) is cut to them, or filled out with zeros, so every vector of a model has the one length.
pub fn fitted(vector: &[f32]) -> Vec<f32> {
    let mut fitted = vector[..vector.len().min(DIMS)].to_vec();
    fitted.resize(DIMS, 0.);
    fitted
}

/// A vector as a file keeps it: each value at `scale` maps to 127, clamped beyond. A vector that's all zero stays a
/// "not yet", so it's kept as the smallest step instead.
pub fn quantize(vector: &[f32], scale: f32) -> Vec<i8> {
    let mut row: Vec<i8> =
        (0..DIMS).map(|at| (vector.get(at).copied().unwrap_or(0.) / scale * 127.).round().clamp(-127., 127.) as i8).collect();
    if row.iter().all(|value| *value == 0) {
        row[0] = 1;
    }
    row
}

fn write_header(file: &mut File, model: &str, scale: f32) -> std::io::Result<()> {
    let mut header = vec![0u8; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(DIMS as u32).to_le_bytes());
    header[16..20].copy_from_slice(&scale.to_le_bytes());
    let id = header_id(model);
    header[20..24].copy_from_slice(&(id.len() as u32).to_le_bytes());
    header[24..24 + id.len()].copy_from_slice(id.as_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header)
}

/// A file's header: its scale and model id, when it's a vector file this Kumi reads.
fn read_header(file: &mut File) -> Option<(f32, String)> {
    let mut header = vec![0u8; HEADER];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut header).ok()?;
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    if &header[..8] != MAGIC || word(8) != VERSION || word(12) as usize != DIMS {
        return None;
    }
    let scale = f32::from_le_bytes(header[16..20].try_into().unwrap());
    let length = (word(20) as usize).min(ID_BYTES);
    Some((scale, String::from_utf8_lossy(&header[24..24 + length]).into_owned()))
}

/// Where the scan puts the vectors it makes for one style model.
pub struct VectorWriter {
    path: PathBuf,
    model: String,
    /// The open file, once the scale is set.
    file: Option<(File, f32)>,
}

impl VectorWriter {
    /// The writer for `model`'s file: its scale read when the file is there, else its vectors wait in music.db.
    pub fn open(music_dir: &Path, model: &str) -> Result<VectorWriter, MusicError> {
        let path = file_for(music_dir, model);
        let file = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(mut file) => match read_header(&mut file) {
                Some((scale, id)) if id == header_id(model) && scale > 0. => Some((file, scale)),
                // Not one this Kumi reads, or another model's under the same name: made again.
                _ => {
                    drop(file);
                    std::fs::remove_file(&path)?;
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(VectorWriter { path, model: model.to_string(), file })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One song's vector: into the file once its scale is set, else waiting in music.db until `SCALE_FROM` wait.
    pub fn put(&mut self, store: &MusicStore, id: i64, vector: &[f32]) -> Result<(), MusicError> {
        if let Some((file, scale)) = &mut self.file {
            file.seek(SeekFrom::Start((HEADER + id as usize * DIMS) as u64))?;
            file.write_all(&quantize(vector, *scale).iter().map(|value| *value as u8).collect::<Vec<_>>())?;
            return Ok(());
        }
        let bytes: Vec<u8> = fitted(vector).iter().flat_map(|value| value.to_le_bytes()).collect();
        store.connection().execute(
            "INSERT INTO vectors_pending (model, id, vector) VALUES (?1, ?2, ?3)
             ON CONFLICT(model, id) DO UPDATE SET vector = excluded.vector",
            params![self.model, id, bytes],
        )?;
        if pending(store, &self.model)?.len() >= SCALE_FROM {
            self.settle(store)?;
        }
        Ok(())
    }

    /// The scale set from the vectors waiting (however few, when the scan has nothing more to hear for now), the file
    /// made and the waiting vectors written to it.
    pub fn settle(&mut self, store: &MusicStore) -> Result<(), MusicError> {
        if self.file.is_some() {
            return self.settle_left(store);
        }
        let waiting = pending(store, &self.model)?;
        if waiting.is_empty() {
            return Ok(());
        }
        let scale = scale_of(&waiting.values().cloned().collect::<Vec<_>>());
        let temporary = self.path.with_extension(format!("i8.{}", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&temporary)?;
        write_header(&mut file, &self.model, scale)?;
        let mut ids: Vec<&i64> = waiting.keys().collect();
        ids.sort();
        for id in ids {
            file.seek(SeekFrom::Start((HEADER + *id as usize * DIMS) as u64))?;
            file.write_all(&quantize(&waiting[id], scale).iter().map(|value| *value as u8).collect::<Vec<_>>())?;
        }
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)?;
        store.connection().execute("DELETE FROM vectors_pending WHERE model = ?1", [&self.model])?;
        self.file = Some((file, scale));
        Ok(())
    }

    /// With the file made, the vectors still waiting beside it (a scan ended between making it and clearing them):
    /// written to it, so a song's waiting copy never stands in for its row. Nothing before the file is made.
    pub fn settle_left(&mut self, store: &MusicStore) -> Result<(), MusicError> {
        let Some((file, scale)) = &mut self.file else { return Ok(()) };
        let waiting = pending(store, &self.model)?;
        if waiting.is_empty() {
            return Ok(());
        }
        let mut ids: Vec<&i64> = waiting.keys().collect();
        ids.sort();
        for id in ids {
            file.seek(SeekFrom::Start((HEADER + *id as usize * DIMS) as u64))?;
            file.write_all(&quantize(&waiting[id], *scale).iter().map(|value| *value as u8).collect::<Vec<_>>())?;
        }
        file.sync_all()?;
        store.connection().execute("DELETE FROM vectors_pending WHERE model = ?1", [&self.model])?;
        Ok(())
    }

    /// Whether the scale is set (the vectors are in the file).
    pub fn settled(&self) -> bool {
        self.file.is_some()
    }
}

/// The vectors waiting for a model's file, by song (each at `DIMS` numbers).
fn pending(store: &MusicStore, model: &str) -> Result<HashMap<i64, Vec<f32>>, MusicError> {
    let mut statement = store.connection().prepare_cached("SELECT id, vector FROM vectors_pending WHERE model = ?1")?;
    let rows = statement.query_map([model], |row| {
        let bytes: Vec<u8> = row.get(1)?;
        let vector: Vec<f32> = bytes.chunks_exact(4).map(|four| f32::from_le_bytes(four.try_into().unwrap())).collect();
        Ok((row.get::<_, i64>(0)?, fitted(&vector)))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The vector files in a music folder, one for each style model that heard its songs.
fn files_in(music_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(music_dir) else { return vec![] };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("vectors-") && name.ends_with(".i8"))
        })
        .collect()
}

/// Songs whose files changed: what every style model heard of them goes (their rows emptied, their waiting vectors
/// dropped), so they're heard afresh and never stand for the audio they had.
pub fn forget(music_dir: &Path, store: &MusicStore, ids: &[i64]) -> Result<(), MusicError> {
    if ids.is_empty() {
        return Ok(());
    }
    for chunk in ids.chunks(500) {
        let list = chunk.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        store.connection().execute(&format!("DELETE FROM vectors_pending WHERE id IN ({list})"), [])?;
    }
    let empty = [0u8; DIMS];
    for path in files_in(music_dir) {
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&path)?;
        let length = file.metadata()?.len();
        for id in ids {
            let at = (HEADER + usize::try_from(*id).unwrap_or(0) * DIMS) as u64;
            if *id >= 0 && at + DIMS as u64 <= length {
                file.seek(SeekFrom::Start(at))?;
                file.write_all(&empty)?;
            }
        }
    }
    Ok(())
}

/// Every vector file in a music folder taken away: a music.db made afresh numbers its songs from 1 again, so rows
/// another one wrote would stand for other songs.
pub fn remove_files(music_dir: &Path) -> Result<(), MusicError> {
    for path in files_in(music_dir) {
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(())
}

/// The vectors one style model made, as the app reads them: the file through a memory map, and those still waiting.
pub struct VectorReader {
    map: Option<Mmap>,
    scale: f32,
    waiting: HashMap<i64, Vec<f32>>,
}

impl VectorReader {
    /// `model`'s vectors: none at all when it made none yet.
    pub fn open(music_dir: &Path, store: &MusicStore, model: &str) -> Result<VectorReader, MusicError> {
        let waiting = pending(store, model)?;
        let path = file_for(music_dir, model);
        let (map, scale) = match File::open(&path) {
            Ok(mut file) => match read_header(&mut file) {
                Some((scale, id)) if id == header_id(model) && scale > 0. => {
                    // Safety: the scan only ever writes rows past the header in place (a whole new file is renamed
                    // over it), so a row read while it's written is at worst a half-new vector, never out of bounds.
                    (Some(unsafe { Mmap::map(&file)? }), scale)
                }
                _ => (None, 1.),
            },
            Err(_) => (None, 1.),
        };
        Ok(VectorReader { map, scale, waiting })
    }

    /// A song's row in the file (None: not embedded yet, or waiting).
    pub fn row(&self, id: i64) -> Option<&[i8]> {
        let map = self.map.as_ref()?;
        let at = HEADER + usize::try_from(id).ok()? * DIMS;
        let bytes = map.get(at..at + DIMS)?;
        // Safety: i8 and u8 have one layout.
        let row: &[i8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast(), DIMS) };
        row.iter().any(|value| *value != 0).then_some(row)
    }

    /// A song's vector, back as numbers (unit length to within the file's steps).
    pub fn get(&self, id: i64) -> Option<Vec<f32>> {
        if let Some(waiting) = self.waiting.get(&id) {
            return Some(waiting.clone());
        }
        let step = self.scale / 127.;
        self.row(id).map(|row| row.iter().map(|value| *value as f32 * step).collect())
    }

    /// Whether a song has a vector.
    pub fn has(&self, id: i64) -> bool {
        self.waiting.contains_key(&id) || self.row(id).is_some()
    }

    /// The `count` songs whose vectors point nearest a unit `target`, nearest first, with how far each lies (as
    /// `distance` reads it). One pass over the file: 300,000 songs in a few tens of milliseconds.
    pub fn nearest(&self, target: &[f32], count: usize) -> Vec<(i64, f64)> {
        let length = target.iter().map(|value| (*value as f64).powi(2)).sum::<f64>().sqrt();
        if count == 0 || length == 0. || target.len() < DIMS {
            return vec![];
        }
        // Only the order matters: each row's cosine with the target, the target's length left out.
        let mut best: Vec<(f64, i64)> = Vec::with_capacity(count + 1);
        let mut offer = |alike: f64, id: i64| {
            if best.len() == count && best.last().is_some_and(|(worst, _)| alike <= *worst) {
                return;
            }
            let at = best.partition_point(|(other, _)| *other >= alike);
            best.insert(at, (alike, id));
            best.truncate(count);
        };
        if let Some(map) = &self.map {
            let rows = map.len().saturating_sub(HEADER) / DIMS;
            for id in 0..rows {
                let row = &map[HEADER + id * DIMS..HEADER + (id + 1) * DIMS];
                let (mut dot, mut squares) = (0f32, 0i32);
                for (value, wanted) in row.iter().zip(target) {
                    let value = *value as i8;
                    dot += value as f32 * wanted;
                    squares += value as i32 * value as i32;
                }
                if squares > 0 && !self.waiting.contains_key(&(id as i64)) {
                    offer(dot as f64 / (squares as f64).sqrt(), id as i64);
                }
            }
        }
        for (id, vector) in &self.waiting {
            if let Some(far) = crate::embed::distance(vector, target) {
                offer((1. - far) * length, *id);
            }
        }
        best.into_iter().map(|(alike, id)| (id, 1. - alike / length)).collect()
    }

    /// How far a song lies from a unit `target`, as `embed::distance` reads it (0 alike, 2 opposite).
    pub fn distance(&self, id: i64, target: &[f32]) -> Option<f64> {
        if let Some(waiting) = self.waiting.get(&id) {
            return crate::embed::distance(waiting, target);
        }
        let row = self.row(id)?;
        let (mut dot, mut length) = (0f64, 0f64);
        for (value, wanted) in row.iter().zip(target) {
            let value = *value as f64;
            dot += value * *wanted as f64;
            length += value * value;
        }
        let target_length = target.iter().map(|value| (*value as f64).powi(2)).sum::<f64>().sqrt();
        (length > 0. && target_length > 0.).then(|| 1. - dot / (length.sqrt() * target_length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_keeps_its_direction_through_eight_bits() {
        let vector: Vec<f32> = (0..DIMS).map(|at| (at as f32 * 0.37).sin() * 0.06).collect();
        let scale = scale_of(std::slice::from_ref(&vector));
        let row = quantize(&vector, scale);
        let back: Vec<f32> = row.iter().map(|value| *value as f32 * scale / 127.).collect();
        assert!(crate::embed::distance(&vector, &back).unwrap() < 1e-3);
        assert!(quantize(&[0.; DIMS], 1.).iter().any(|value| *value != 0), "a zero vector isn't read as not yet");
    }

    #[test]
    fn a_long_model_id_is_kept_as_its_hash() {
        assert_eq!(header_id("sha256:abc"), "sha256:abc");
        assert!(header_id(&"file:/x".repeat(100)).starts_with("sha256-of:"));
        assert!(header_id(&"file:/x".repeat(100)).len() <= ID_BYTES);
    }
}
