//! What the bridge looks at outside Logic: on disk after Logic wrote there (a bounce, an export, a listening pass's
//! capture), whether a file is there and its size, a sound file's format and length from its header (WAVE or AIFF),
//! the files of a folder, the onsets in a stretch of a capture, a part cut out of it, a file taken away; and the host
//! clock a poll is dated by. Every look goes through one seam ([`set_hook`]), so the change recorder keeps what each
//! look saw in the scene and a replay answers from it: the file a recorded bounce wrote isn't on the Mac that replays
//! it, and a recording keeps no audio. Without a hook, the disk and the clock themselves.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::{json, Value};

/// A sound file as its header says: its size on disk, channels, sample rate, bits per sample, whether its samples are
/// floats, and its length in frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub bytes: u64,
    pub format: &'static str,
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub float: bool,
    pub frames: u64,
}

impl Sound {
    /// Its length in seconds, to the millisecond.
    pub fn seconds(&self) -> f64 {
        if self.rate == 0 {
            return 0.0;
        }
        (self.frames as f64 / f64::from(self.rate) * 1000.0).round() / 1000.0
    }

    pub fn to_json(&self) -> Value {
        json!({"bytes": self.bytes, "format": self.format, "channels": self.channels, "rate": self.rate, "bits": self.bits, "float": self.float, "frames": self.frames})
    }

    pub fn from_json(value: &Value) -> Option<Sound> {
        let format = match value.get("format")?.as_str()? {
            "wav" => "wav",
            "aiff" => "aiff",
            _ => return None,
        };
        Some(Sound {
            bytes: value.get("bytes")?.as_u64()?,
            format,
            channels: value.get("channels")?.as_u64()? as u16,
            rate: value.get("rate")?.as_u64()? as u32,
            bits: value.get("bits")?.as_u64()? as u16,
            float: value.get("float")?.as_bool()?,
            frames: value.get("frames")?.as_u64()?,
        })
    }
}

/// A look at the disk, as the seam sees it: its kind (`size`, `sound`, `list`), the path, and the real look to make.
pub type Hook = Box<dyn FnMut(&str, &str, &dyn Fn() -> Value) -> Value>;

thread_local! {
    /// The seam, per thread: the bridge's run loop and the recorder look from one thread, and each replaying test from
    /// its own.
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Sets this thread's seam (None: the disk itself).
pub fn set_hook(hook: Option<Hook>) {
    HOOK.with(|slot| *slot.borrow_mut() = hook);
}

fn look(kind: &str, path: &Path, real: &dyn Fn() -> Value) -> Value {
    let text = path.to_string_lossy().into_owned();
    HOOK.with(|slot| match slot.borrow_mut().as_mut() {
        Some(hook) => hook(kind, &text, real),
        None => real(),
    })
}

/// The size of the file at `path`, if one is there (a folder macOS keeps from Kumi reads as nothing there).
pub fn size(path: &Path) -> Option<u64> {
    let owned = path.to_path_buf();
    look("size", path, &move || std::fs::metadata(&owned).ok().filter(|m| m.is_file()).map_or(Value::Null, |m| m.len().into())).as_u64()
}

/// Whether anything (a file or a folder) is at `path`, as far as Kumi can see.
pub fn exists(path: &Path) -> bool {
    let owned = path.to_path_buf();
    look("exists", path, &move || std::fs::metadata(&owned).is_ok().into()) == Value::Bool(true)
}

/// The sound file at `path` as its header says, if it is a WAVE or AIFF file Kumi can read.
pub fn sound(path: &Path) -> Option<Sound> {
    let owned = path.to_path_buf();
    let value = look("sound", path, &move || read_sound(&owned).map_or(Value::Null, |sound| sound.to_json()));
    Sound::from_json(&value)
}

/// The files directly in the folder at `path`, by name with their sizes, sorted by name; None when it can't be read.
pub fn list(path: &Path) -> Option<Vec<(String, u64)>> {
    let owned = path.to_path_buf();
    let value = look("list", path, &move || {
        let Ok(entries) = std::fs::read_dir(&owned) else { return Value::Null };
        let mut files: Vec<(String, u64)> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok().filter(|m| m.is_file())?;
                Some((entry.file_name().to_string_lossy().into_owned(), meta.len()))
            })
            .collect();
        files.sort();
        Value::Array(files.into_iter().map(|(name, bytes)| json!([name, bytes])).collect())
    });
    value
        .as_array()
        .map(|files| files.iter().filter_map(|file| Some((file.get(0)?.as_str()?.to_string(), file.get(1)?.as_u64()?))).collect())
}

/// The onsets in frames `[from, to)` of the 32-bit float WAV at `path` (a capture), as frames of the file; None when
/// it can't be read.
pub fn onsets(path: &Path, from: u64, to: u64) -> Option<Vec<f64>> {
    let owned = path.to_path_buf();
    let value = look("onsets", path, &move || {
        crate::listen::audio::onsets_in(&owned, from, to).map_or(Value::Null, |found| found.into_iter().map(Value::from).collect())
    });
    value.as_array().map(|found| found.iter().filter_map(Value::as_f64).collect())
}

/// Frames `[from, from + frames)` of the capture at `source` written into a new file at `dest`, measured; None when
/// it couldn't be (a file there already, a capture that can't be read).
pub fn cut(source: &Path, dest: &Path, from: u64, frames: u64) -> Option<crate::listen::audio::Cut> {
    let (source, owned) = (source.to_path_buf(), dest.to_path_buf());
    let value = look("cut", dest, &move || match crate::listen::audio::cut(&source, &owned, from, frames) {
        Ok(cut) => json!({"frames": cut.frames, "rate": cut.rate, "channels": cut.channels, "peak": cut.peak, "rms": cut.rms}),
        Err(error) => {
            crate::log::line(&format!("couldn't cut {} from {}: {error}", owned.display(), source.display()));
            Value::Null
        }
    });
    Some(crate::listen::audio::Cut {
        frames: value.get("frames")?.as_u64()?,
        rate: u32::try_from(value.get("rate")?.as_u64()?).ok()?,
        channels: u16::try_from(value.get("channels")?.as_u64()?).ok()?,
        peak: value.get("peak")?.as_f64()?,
        rms: value.get("rms")?.as_f64()?,
    })
}

/// One key of Logic's preferences (the `com.apple.logic10` domain) as `defaults read` prints it ("1" for a switch that is
/// on), through the preferences daemon, so a setting just changed in Logic's Settings window is seen at once; None when
/// the key isn't written (Logic leaves a setting it never changed unwritten) or can't be read. The seam's path is the
/// key alone, so a recording names no home.
pub fn preference(key: &str) -> Option<String> {
    let owned = key.to_string();
    let value = look("preference", Path::new(key), &move || {
        let Ok(output) = std::process::Command::new(crate::tools::DEFAULTS).args(["read", "com.apple.logic10", &owned]).output() else {
            return Value::Null;
        };
        if !output.status.success() {
            return Value::Null;
        }
        Value::String(String::from_utf8_lossy(&output.stdout).trim().to_string())
    });
    value.as_str().map(str::to_string)
}

/// Where Spotlight finds Logic projects of these file names (`Song.logicx`): for each name, the packages of that name
/// macOS lets Kumi see, outside any other package and the Trash. Spotlight answers only for folders the process may
/// read, so a project under `~/Music`, which macOS keeps from a process without the Files and Folders permission,
/// isn't found. The seam's path is the names, one a line.
pub fn projects_named(names: &[String]) -> Vec<(String, Vec<String>)> {
    if names.is_empty() {
        return Vec::new();
    }
    let owned = names.to_vec();
    let value = look("spotlight", Path::new(&names.join("\n")), &move || {
        let quoted: Vec<String> =
            owned.iter().map(|name| format!("kMDItemFSName == \"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))).collect();
        let Ok(output) = std::process::Command::new(crate::tools::MDFIND).arg(quoted.join(" || ")).output() else { return Value::Null };
        if !output.status.success() {
            return Value::Null;
        }
        let mut found = serde_json::Map::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let path = Path::new(line.trim());
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let inside = path.parent().is_some_and(|parent| {
                parent.components().any(|part| {
                    let part = part.as_os_str().to_string_lossy();
                    part.ends_with(".logicx") || part == ".Trash"
                })
            });
            if inside || !owned.contains(&name) {
                continue;
            }
            if let Value::Array(paths) = found.entry(name).or_insert_with(|| Value::Array(vec![])) {
                paths.push(line.trim().into());
            }
        }
        Value::Object(found)
    });
    names
        .iter()
        .map(|name| {
            let paths = value.get(name).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string);
            (name.clone(), paths.collect())
        })
        .collect()
}

/// Takes the file at `path` away: whether it went.
pub fn remove(path: &Path) -> bool {
    let owned = path.to_path_buf();
    look("remove", path, &move || std::fs::remove_file(&owned).is_ok().into()) == Value::Bool(true)
}

/// The host clock now, in `mach_absolute_time` ticks (CoreMIDI's and Core Audio's clock): what a poll of Logic is
/// dated by.
pub fn host_now() -> u64 {
    look("clock", Path::new(""), &|| host_clock().into()).as_u64().unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn host_clock() -> u64 {
    extern "C" {
        fn mach_absolute_time() -> u64;
    }
    // SAFETY: mach_absolute_time takes nothing and only reads the clock.
    unsafe { mach_absolute_time() }
}

#[cfg(not(target_os = "macos"))]
fn host_clock() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos() as u64)
}

/// Reads a sound file's header: WAVE (PCM, float or extensible) or AIFF/AIFF-C.
pub fn read_sound(path: &Path) -> Option<Sound> {
    let mut file = File::open(path).ok()?;
    let bytes = file.metadata().ok()?.len();
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    match (&head[0..4], &head[8..12]) {
        (b"RIFF", b"WAVE") => wave(&mut file, bytes),
        (b"FORM", b"AIFF") | (b"FORM", b"AIFC") => aiff(&mut file, bytes),
        _ => None,
    }
}

/// The chunks after the 12-byte head, in order, until `wanted` has all it needs.
fn chunks(file: &mut File, little: bool, mut on: impl FnMut(&[u8; 4], u64, &mut File) -> Option<bool>) -> Option<()> {
    let mut at = 12u64;
    for _ in 0..64 {
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let id: [u8; 4] = header[0..4].try_into().ok()?;
        let size =
            if little { u32::from_le_bytes(header[4..8].try_into().ok()?) } else { u32::from_be_bytes(header[4..8].try_into().ok()?) };
        let size = u64::from(size);
        if on(&id, size, file)? {
            return Some(());
        }
        at += 8 + size + (size & 1);
    }
    None
}

fn wave(file: &mut File, bytes: u64) -> Option<Sound> {
    let (mut channels, mut rate, mut bits, mut float, mut block, mut data) = (0u16, 0u32, 0u16, false, 0u16, None);
    chunks(file, true, |id, size, file| {
        match id {
            b"fmt " => {
                let mut fmt = [0u8; 16];
                file.read_exact(&mut fmt).ok()?;
                let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
                channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                block = u16::from_le_bytes([fmt[12], fmt[13]]);
                bits = u16::from_le_bytes([fmt[14], fmt[15]]);
                float = tag == 3;
                if tag == 0xFFFE && size >= 40 {
                    // WAVE_FORMAT_EXTENSIBLE: the sub-format's first two bytes say PCM (1) or float (3).
                    let mut extension = [0u8; 24];
                    file.read_exact(&mut extension).ok()?;
                    float = u16::from_le_bytes([extension[8], extension[9]]) == 3;
                }
                Some(false)
            }
            b"data" => {
                data = Some(size);
                Some(true)
            }
            _ => Some(false),
        }
    })?;
    let data = data?;
    (block > 0 && rate > 0).then(|| Sound { bytes, format: "wav", channels, rate, bits, float, frames: data / u64::from(block) })
}

fn aiff(file: &mut File, bytes: u64) -> Option<Sound> {
    let mut found = None;
    chunks(file, false, |id, size, file| {
        if id != b"COMM" {
            return Some(false);
        }
        // AIFF's COMM is 18 bytes; AIFF-C's adds the compression type (and its name).
        let mut comm = [0u8; 22];
        let wanted = if size >= 22 { 22 } else { 18 };
        file.read_exact(&mut comm[..wanted]).ok()?;
        let channels = u16::from_be_bytes([comm[0], comm[1]]);
        let frames = u64::from(u32::from_be_bytes([comm[2], comm[3], comm[4], comm[5]]));
        let bits = u16::from_be_bytes([comm[6], comm[7]]);
        let rate = extended(&comm[8..18])?;
        let float = &comm[18..22] == b"fl32" || &comm[18..22] == b"FL32";
        found = Some(Sound { bytes, format: "aiff", channels, rate, bits, float, frames });
        Some(true)
    })?;
    found
}

/// An 80-bit IEEE extended float (AIFF's sample rate), as a whole number.
fn extended(raw: &[u8]) -> Option<u32> {
    let exponent = (i32::from(raw[0] & 0x7F) << 8 | i32::from(raw[1])) - 16383;
    let mantissa = u64::from_be_bytes(raw[2..10].try_into().ok()?);
    if mantissa == 0 || !(0..=31).contains(&exponent) {
        return None;
    }
    Some((mantissa >> (63 - exponent)) as u32)
}

/// A recorder's seam: each look made for real, and kept as `{kind, path, answer}` in `kept`.
pub fn recording(kept: std::rc::Rc<RefCell<Vec<Value>>>) -> Hook {
    Box::new(move |kind, path, real| {
        let answer = real();
        kept.borrow_mut().push(json!({"kind": kind, "path": path, "answer": answer}));
        answer
    })
}

/// A replay's seam: each look answered with what the recording saw for the same kind and path, in order, the last
/// one again once they run out; a look the recording never made answers nothing there. Where a recorded cut wrote a
/// file, the replay writes a short synthesized one of the same length and format (when its folder is there), since a
/// recording keeps no audio.
pub fn replaying(recorded: &[Value]) -> Hook {
    let mut queues: std::collections::HashMap<String, std::collections::VecDeque<Value>> = std::collections::HashMap::new();
    for look in recorded {
        let key = format!("{} {}", look["kind"].as_str().unwrap_or_default(), look["path"].as_str().unwrap_or_default());
        queues.entry(key).or_default().push_back(look["answer"].clone());
    }
    Box::new(move |kind, path, _real| {
        let key = format!("{kind} {path}");
        let answer = match queues.get_mut(&key) {
            Some(queue) if queue.len() > 1 => queue.pop_front().unwrap_or(Value::Null),
            Some(queue) => queue.front().cloned().unwrap_or(Value::Null),
            None => {
                if kind == "exists" {
                    false.into()
                } else {
                    Value::Null
                }
            }
        };
        if kind == "cut" {
            stand_in(Path::new(path), &answer);
        }
        answer
    })
}

/// The synthesized file a replayed cut leaves where the recording's was: as many frames, at its rate and channels.
fn stand_in(path: &Path, answer: &Value) {
    let (Some(frames), Some(rate), Some(channels)) = (
        answer.get("frames").and_then(Value::as_u64),
        answer.get("rate").and_then(Value::as_u64),
        answer.get("channels").and_then(Value::as_u64),
    ) else {
        return;
    };
    if path.parent().is_some_and(Path::is_dir) && !path.exists() {
        let _ = crate::listen::audio::synthesize(path, rate as u32, channels as u16, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave_bytes(channels: u16, rate: u32, bits: u16, tag: u16, frames: u32) -> Vec<u8> {
        let block = channels * bits / 8;
        let data = frames * u32::from(block);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4 + 8 + 16 + 8 + 12 + 8 + data).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
        out.extend_from_slice(&block.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        // A chunk of Logic's own before the samples, which the reader steps over.
        out.extend_from_slice(b"LGWV");
        out.extend_from_slice(&12u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, data as usize));
        out
    }

    #[test]
    fn a_sound_files_format_and_length_come_from_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bounce.wav");
        std::fs::write(&path, wave_bytes(2, 48000, 24, 1, 24000)).unwrap();
        let sound = read_sound(&path).unwrap();
        assert_eq!((sound.format, sound.channels, sound.rate, sound.bits, sound.float, sound.frames), ("wav", 2, 48000, 24, false, 24000));
        assert_eq!(sound.seconds(), 0.5);
        assert_eq!(Sound::from_json(&sound.to_json()), Some(sound.clone()));
        std::fs::write(&path, wave_bytes(1, 44100, 32, 3, 44100)).unwrap();
        assert!(read_sound(&path).unwrap().float);
        // AIFF: the COMM chunk, big-endian, its rate an 80-bit float (44100 = 0x400E AC44 0000…).
        let mut aiff = Vec::new();
        aiff.extend_from_slice(b"FORM");
        aiff.extend_from_slice(&(4u32 + 8 + 18).to_be_bytes());
        aiff.extend_from_slice(b"AIFF");
        aiff.extend_from_slice(b"COMM");
        aiff.extend_from_slice(&18u32.to_be_bytes());
        aiff.extend_from_slice(&2u16.to_be_bytes());
        aiff.extend_from_slice(&88200u32.to_be_bytes());
        aiff.extend_from_slice(&16u16.to_be_bytes());
        aiff.extend_from_slice(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        let aiff_path = dir.path().join("Bounce.aif");
        std::fs::write(&aiff_path, &aiff).unwrap();
        let sound = read_sound(&aiff_path).unwrap();
        assert_eq!((sound.format, sound.rate, sound.frames, sound.seconds()), ("aiff", 44100, 88200, 2.0));
        std::fs::write(&path, b"not a sound").unwrap();
        assert_eq!(read_sound(&path), None);
        assert_eq!(size(&dir.path().join("nothing.wav")), None);
        assert_eq!(size(&path), Some(11));
        assert!(exists(dir.path()) && !exists(&dir.path().join("nothing")));
        let listed = list(dir.path()).unwrap();
        assert_eq!(listed.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["Bounce.aif", "Bounce.wav"]);
    }

    #[test]
    fn the_seam_records_what_it_saw_and_a_replay_answers_the_same_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Mix.wav");
        let kept = std::rc::Rc::new(RefCell::new(Vec::new()));
        set_hook(Some(recording(kept.clone())));
        assert_eq!(size(&path), None);
        let bytes = wave_bytes(2, 48000, 24, 1, 4800);
        std::fs::write(&path, &bytes).unwrap();
        let length = bytes.len() as u64;
        assert_eq!(size(&path), Some(length));
        assert_eq!(sound(&path).unwrap().frames, 4800);
        set_hook(None);
        let recorded = kept.borrow().clone();
        assert_eq!(recorded.len(), 3);
        std::fs::remove_file(&path).unwrap();
        set_hook(Some(replaying(&recorded)));
        assert_eq!(size(&path), None, "the first look, as recorded");
        assert_eq!(size(&path), Some(length), "then the file, as recorded, though it is gone");
        assert_eq!(size(&path), Some(length), "the last answer again");
        assert_eq!(sound(&path).unwrap().seconds(), 0.1);
        assert!(!exists(&dir.path().join("never looked at")));
        set_hook(None);
    }

    #[test]
    fn a_listening_passs_looks_are_recorded_and_a_replay_leaves_a_stand_in_where_a_part_was_cut() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("tap.wav");
        // A 32-bit float capture: silence, a click at frame 2000.
        let mut samples = vec![0f32; 2 * 8000];
        for k in 0..32 {
            samples[(2000 + k) * 2] = 0.5;
            samples[(2000 + k) * 2 + 1] = 0.5;
        }
        let mut bytes = crate::listen::audio::header(44100, 2, 8000);
        for sample in &samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&capture, bytes).unwrap();
        let part = dir.path().join("part.wav");
        let kept = std::rc::Rc::new(RefCell::new(Vec::new()));
        set_hook(Some(recording(kept.clone())));
        assert_eq!(onsets(&capture, 1000, 4000), Some(vec![2000.0]));
        let made = cut(&capture, &part, 1000, 4000).unwrap();
        assert_eq!((made.frames, made.rate, made.channels, made.peak), (4000, 44100, 2, 0.5));
        assert!(cut(&capture, &part, 0, 10).is_none(), "never over a file");
        let now = host_now();
        assert!(now > 0);
        assert!(remove(&capture) && !remove(&capture));
        set_hook(None);
        let recorded = kept.borrow().clone();
        assert_eq!(
            recorded.iter().map(|look| look["kind"].as_str().unwrap()).collect::<Vec<_>>(),
            ["onsets", "cut", "cut", "clock", "remove", "remove"]
        );
        // Replayed without the files: the same answers, and a stand-in of the part's length where it was cut.
        std::fs::remove_file(&part).unwrap();
        set_hook(Some(replaying(&recorded)));
        assert_eq!(onsets(&capture, 1000, 4000), Some(vec![2000.0]));
        assert_eq!(cut(&capture, &part, 1000, 4000), Some(made));
        assert_eq!(read_sound(&part).map(|sound| (sound.frames, sound.rate, sound.channels, sound.float)), Some((4000, 44100, 2, true)));
        assert_eq!(host_now(), now);
        assert!(remove(&capture));
        set_hook(None);
    }
}
