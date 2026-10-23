//! (M4b) A bounce and an export, as the bridge's methods `bounce` and `export_tracks` make them over the real Logic
//! (`crates/logic-kumi/src/changes/bounce.rs`): no change and no row, as `project_save`. A bounce is Logic's Bounce
//! dialog (File > Bounce > Project or Section…: Uncompressed alone, its file type and bit depth, Normalize off, the
//! range in its Start and End) and the save panel, the file found on disk and its header read; an export is File >
//! Export > All Tracks as Audio Files… into a folder that is there, the folder's new files the answer. The simulated
//! Logic writes them where it is asked to: silence of the range's length at the project's tempo, 44.1 kHz stereo,
//! never over a file and only into a folder that is there.
use super::{bad, uncertain, Answer, Fault, State};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// The longest bounce Kumi asks for, in beats (an hour at 240 BPM).
pub(super) const MOST_BEATS: f64 = 14400.0;
pub(super) const SAMPLE_RATE: u32 = 44_100;
pub(super) const CHANNELS: u16 = 2;

/// A file format a bounce writes, by the tool's word: its container, extensions, bits and whether its samples are
/// floats (the bridge's `FORMATS`).
#[derive(Clone, Copy)]
struct Format {
    name: &'static str,
    aiff: bool,
    extensions: &'static [&'static str],
    bits: u16,
    float: bool,
}

const FORMATS: [Format; 4] = [
    Format { name: "wav24", aiff: false, extensions: &["wav", "wave"], bits: 24, float: false },
    Format { name: "wav16", aiff: false, extensions: &["wav", "wave"], bits: 16, float: false },
    Format { name: "wav32f", aiff: false, extensions: &["wav", "wave"], bits: 32, float: true },
    Format { name: "aiff24", aiff: true, extensions: &["aif", "aiff"], bits: 24, float: false },
];

/// How long Logic's own WAV header is (its RIFF, fmt and data chunks with the chunks it adds before them): the
/// `change-bounce` scene's 3 s of 24-bit stereo at 44.1 kHz is 796020 bytes, 793800 of them sound.
const LOGIC_HEADER: u64 = 2220;

/// What a file of silence holds: its size in bytes and its length in seconds, as its header says.
struct Sound {
    bytes: u64,
    seconds: f64,
}

/// An audio file of silence `seconds` long at `path`, made new (never over a file).
fn write_sound(path: &Path, seconds: f64, format: Format) -> std::io::Result<Sound> {
    let block = u64::from(CHANNELS * format.bits / 8);
    let frames = (seconds * f64::from(SAMPLE_RATE)).round() as u64;
    let data = frames * block;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut header = Vec::with_capacity(54);
    if format.aiff {
        // FORM AIFF: COMM (channels, frames, bits, the rate as an 80-bit float: 44100 is 0x400E AC44 0…) and SSND.
        header.extend_from_slice(b"FORM");
        header.extend_from_slice(&((4 + 26 + 16 + data) as u32).to_be_bytes());
        header.extend_from_slice(b"AIFFCOMM");
        header.extend_from_slice(&18u32.to_be_bytes());
        header.extend_from_slice(&CHANNELS.to_be_bytes());
        header.extend_from_slice(&(frames as u32).to_be_bytes());
        header.extend_from_slice(&format.bits.to_be_bytes());
        header.extend_from_slice(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        header.extend_from_slice(b"SSND");
        header.extend_from_slice(&((8 + data) as u32).to_be_bytes());
        header.extend_from_slice(&[0; 8]);
    } else {
        // RIFF WAVE, a JUNK chunk standing for what Logic writes beside the format (so the file's size is Logic's), fmt
        // and data.
        let junk = LOGIC_HEADER - 52;
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&((LOGIC_HEADER - 8 + data) as u32).to_le_bytes());
        header.extend_from_slice(b"WAVEJUNK");
        header.extend_from_slice(&(junk as u32).to_le_bytes());
        header.extend(std::iter::repeat_n(0u8, junk as usize));
        header.extend_from_slice(b"fmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&(if format.float { 3u16 } else { 1u16 }).to_le_bytes());
        header.extend_from_slice(&CHANNELS.to_le_bytes());
        header.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        header.extend_from_slice(&(SAMPLE_RATE * block as u32).to_le_bytes());
        header.extend_from_slice(&(block as u16).to_le_bytes());
        header.extend_from_slice(&format.bits.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&(data as u32).to_le_bytes());
    }
    file.write_all(&header)?;
    let zeros = vec![0u8; 64 * 1024];
    let mut left = data;
    while left > 0 {
        let n = left.min(zeros.len() as u64) as usize;
        file.write_all(&zeros[..n])?;
        left -= n as u64;
    }
    Ok(Sound { bytes: header.len() as u64 + data, seconds: frames as f64 / f64::from(SAMPLE_RATE) })
}

/// A whole path with no `..`, a name Logic's panels take (no colon or control character).
fn plain(text: &str, what: &str) -> Result<PathBuf, Fault> {
    let path = PathBuf::from(text);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(bad(format!("{what} must be a whole path, with no \"..\" in it")));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() || name.contains(':') || name.chars().any(char::is_control) || name.chars().count() > 200 {
        return Err(bad(format!("{what} must end in a name Logic Pro can write: no colon in it")));
    }
    Ok(path)
}

fn beats(params: &Value, key: &str) -> Result<f64, Fault> {
    params
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
        .ok_or_else(|| bad(format!("{key} must be a number from 0: beats from the project's start")))
}

impl State {
    /// Seconds from the project's start to `beats`, through the Tempo List.
    pub(super) fn seconds_to(&self, beats: f64) -> f64 {
        let (mut seconds, mut at, mut bpm) = (0.0, 0.0, self.tempo);
        for (change, change_bpm) in &self.tempo_changes {
            if *change >= beats {
                break;
            }
            seconds += (change - at) * 60.0 / bpm;
            (at, bpm) = (*change, *change_bpm);
        }
        seconds + (beats - at) * 60.0 / bpm
    }

    /// The epoch every call that names the project sends: the one it was read in.
    fn same_epoch(&self, params: &Value) -> Result<(), Fault> {
        let sent = params.get("epoch").and_then(Value::as_str).ok_or_else(|| bad("epoch: the epoch the refs were read in"))?;
        if sent != self.epoch_text() {
            return Err((
                "stale".into(),
                format!("those refs are from an earlier epoch ({sent}; now {}): the tracks or regions changed, read again", self.epoch),
            ));
        }
        Ok(())
    }

    /// `bounce {epoch, startBeats, endBeats, path, format?}`: the range bounced to `path` (a whole path to a file that
    /// isn't there yet, in a folder that is) in `format` (`wav24` by default, `wav16`, `wav32f`, `aiff24`), the file
    /// found on disk and its header read. No change: nothing in the project moves, and Logic's history takes no step.
    pub(super) fn bounce(&mut self, params: &Value) -> Answer {
        self.same_epoch(params)?;
        let (start, end) = (beats(params, "startBeats")?, beats(params, "endBeats")?);
        if end <= start {
            return Err(bad("endBeats must be after startBeats"));
        }
        if end - start > MOST_BEATS {
            return Err(bad(format!("Kumi bounces at most {MOST_BEATS} beats at once")));
        }
        let format =
            match params.get("format") {
                None | Some(Value::Null) => FORMATS[0],
                Some(Value::String(name)) => FORMATS.iter().copied().find(|f| f.name == name).ok_or_else(|| {
                    bad(format!("format must be one of {}", FORMATS.iter().map(|f| f.name).collect::<Vec<_>>().join(", ")))
                })?,
                Some(_) => return Err(bad("format must be a string, such as \"wav24\"")),
            };
        let text = params
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| bad("path: a whole path to the audio file to write (a .wav that isn't there yet, in a folder that is)"))?;
        let path = plain(text, "path")?;
        let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !format.extensions.contains(&extension.as_str()) {
            return Err(bad(format!("path must end in .{} for {}", format.extensions[0], format.name)));
        }
        if path.exists() {
            return Err(bad(format!("{} is there already: Kumi never writes over a file", path.display())));
        }
        let folder = path.parent().unwrap_or(Path::new("/"));
        if !folder.is_dir() {
            return Err(bad(format!("the folder {} isn't there (or Kumi can't see it); Kumi makes no folders", folder.display())));
        }
        let seconds = self.seconds_to(end) - self.seconds_to(start);
        let sound = write_sound(&path, seconds, format)
            .map_err(|error| uncertain(format!("Logic Pro didn't finish writing {}: {error}", path.display())))?;
        // (the bridge checks the file's length against the range at the control bar's tempo)
        let expected = (end - start) * 60.0 / self.tempo.max(1.0);
        if (sound.seconds - expected).abs() > 0.5 + expected * 0.05 {
            return Err(uncertain(format!(
                "Logic Pro wrote {} but it lasts {} s, where the range is {expected:.3} s at {} BPM",
                path.display(),
                sound.seconds,
                self.tempo
            )));
        }
        Ok(json!({
            "epoch": self.epoch_text(), "bounced": path.to_string_lossy(), "format": format.name, "bytes": sound.bytes,
            "seconds": sound.seconds, "rate": SAMPLE_RATE, "channels": CHANNELS, "bits": format.bits, "startBeats": start, "endBeats": end,
        }))
    }

    /// `export_tracks {epoch, path}`: File > Export > All Tracks as Audio Files into the folder `path` (whole, there
    /// already), with the options the producer set, as on the bridge's Mac: a 24-bit WAV for each track with sound
    /// (one with no region writes none), named by number ("01.wav"; a name there already numbered on: Logic never
    /// writes over a file), from the project's start to its last region's end: the folder's new files the answer. No
    /// change.
    pub(super) fn export_tracks(&mut self, params: &Value) -> Answer {
        self.same_epoch(params)?;
        let text = params
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| bad("path: a whole path to the folder the tracks' audio files go into (it must be there)"))?;
        let folder = plain(text, "path")?;
        if !folder.is_dir() {
            return Err(bad(format!("the folder {} isn't there (or Kumi can't read it); Kumi makes no folders", folder.display())));
        }
        let mut files = Vec::new();
        let mut number = 0;
        for track in self.tracks.iter().filter(|track| !track.is_stack()) {
            let Some(end) = self.regions.iter().filter(|region| region.track == track.id).map(|region| region.end).reduce(f64::max) else {
                continue;
            };
            let seconds = self.seconds_to(end);
            number += 1;
            let mut name = format!("{number:02}.wav");
            while folder.join(&name).exists() {
                number += 1;
                name = format!("{number:02}.wav");
            }
            let sound = write_sound(&folder.join(&name), seconds, FORMATS[0])
                .map_err(|error| uncertain(format!("Logic Pro didn't finish exporting into {}: {error}", folder.display())))?;
            files.push(json!({"name": name, "bytes": sound.bytes, "seconds": sound.seconds}));
        }
        files.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(json!({"epoch": self.epoch_text(), "folder": folder.to_string_lossy(), "files": files}))
    }
}
