//! A REAPER project (`.rpp`) as the library keeps one, read without REAPER: its tracks (a folder as a group), their FX,
//! items and media, the master's FX, the tempo and the time signature. A line is looked at once: MIDI events, envelope
//! points and plug-in state are passed over undecoded and not held, so a big project reads quickly and a big plug-in state
//! doesn't lose it; what's kept is bounded, so a damaged or hostile file costs little. Words not in UTF-8 are read as near
//! as they come.
use std::io::BufRead;
use std::path::Path;
use std::sync::LazyLock;

use kumi_common::abort::Signal;
use kumi_daw_own::changes::is_kumi_copy;
use kumi_library::daws::ProjectFormat;
use kumi_library::samples::SAMPLE_EXTENSIONS;
use kumi_library::sets::{DeviceRole, SetDevice, SetSummary, SetTrack, TrackKind};
use kumi_library::sources::{dirname, is_absolute, join};
use kumi_types::{daw::DawId, errors::RuntimeError};
use regex::Regex;

/// The most of a project read. A REAPER project is a few megabytes, a MIDI-heavy one tens; a plug-in that keeps its samples
/// in its state can make it gigabytes, and those lines aren't held.
pub const MOST_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The most of one line held: REAPER wraps a plug-in's state well below this; what's past it is passed over.
const MOST_LINE: usize = 1024 * 1024;
/// The most of one word kept (a name, a preset, a path).
const MOST_VALUE: usize = 1024;
/// How much a project may hold of what's kept (tracks, FX, plug-in names, media): more is a damaged or hostile file, whose
/// every read would pay for it.
const MOST_KEPT: usize = 100_000;
/// How deep its blocks may go.
const MOST_DEPTH: usize = 256;
/// The most media a track lists, as a Live Set's does.
const MOST_SAMPLES: usize = 32;
/// The most FX a container lists of what's inside it, as a Live rack does.
const MOST_INSIDE: usize = 16;
/// How often (in lines) a read looks whether it's been stopped.
const CHECK_EVERY: usize = 4096;

/// REAPER's projects, as the library learns them.
pub static FORMAT: ProjectFormat = ProjectFormat { extension: ".rpp", skip, read: read_project };

/// Whether a `.rpp` at `path` isn't one of the producer's projects: one of Kumi's copies of a project, a hidden file
/// (macOS's `._` files on other disks), or one REAPER keeps as a backup or an auto-save, in its `Backups` or `AutoSaves`
/// folder or with its time in the name.
pub fn skip(path: &Path) -> bool {
    static STAMPED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)-\d{4}-\d{2}-\d{2}_\d{4}(\d{2})?\.rpp$").unwrap());
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let kept = path.parent().is_some_and(|folder| {
        folder.components().any(|part| {
            let part = part.as_os_str().to_string_lossy();
            part.eq_ignore_ascii_case("Backups") || part.eq_ignore_ascii_case("AutoSaves")
        })
    });
    name.starts_with('.') || is_kumi_copy(&name) || kept || STAMPED.is_match(&name)
}

/// The project at `path`, read as far as `MOST_BYTES`.
pub fn read_project(path: &Path, signal: &Signal) -> Result<SetSummary, RuntimeError> {
    read_at_most(path, signal, MOST_BYTES)
}

/// The project at `path`, unless it's bigger than `most` bytes.
fn read_at_most(path: &Path, signal: &Signal, most: u64) -> Result<SetSummary, RuntimeError> {
    let shown = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let size = std::fs::metadata(path).map_err(|why| RuntimeError::plain(why.to_string()))?.len();
    if size > most {
        return Err(RuntimeError::plain(format!("{shown} is bigger than Kumi reads ({} MB)", most / (1024 * 1024))));
    }
    let file = std::fs::File::open(path).map_err(|why| RuntimeError::plain(why.to_string()))?;
    let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = dirname(&path.to_string_lossy());
    read_lines(std::io::BufReader::new(file), &name, &folder, signal)
        .and_then(|set| set.ok_or_else(|| RuntimeError::plain(format!("{shown} isn't a REAPER project"))))
}

/// A line's words as REAPER writes them: apart by spaces, one with a space in it quoted with whichever of `"`, `'` and
/// `` ` `` it doesn't hold. Each at most `MOST_VALUE` long.
fn words(line: &str) -> Vec<&str> {
    let short = |word: &'_ str| -> usize { word.floor_char_boundary(MOST_VALUE) };
    let mut said = vec![];
    let mut rest = line.trim_start();
    while let Some(first) = rest.chars().next() {
        if matches!(first, '"' | '\'' | '`') {
            match rest[1..].find(first) {
                Some(end) => {
                    let word = &rest[1..1 + end];
                    said.push(&word[..short(word)]);
                    rest = rest[2 + end..].trim_start();
                }
                None => {
                    let word = &rest[1..];
                    said.push(&word[..short(word)]);
                    break;
                }
            }
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = &rest[..end];
            said.push(&word[..short(word)]);
            rest = rest[end..].trim_start();
        }
    }
    said
}

/// What a block is, by its tag, for what's read in it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Project,
    Track,
    /// A track's FX, or the master's.
    Chain,
    Fx,
    Container,
    Item,
    /// A take's media; MIDI is read for no names, only to say the track holds MIDI.
    Audio,
    Midi,
    Frozen,
    /// Anything else (envelopes, settings, plug-in state): its lines are passed over.
    Other,
}

/// An FX as it's named in its block's line (`<VST "VSTi: ReaSynth (Cockos)" reasynth.vst.dylib 0 "Lead" …`, `<JS
/// Utility/volume ""`, `<CONTAINER Container 0 "" 0`), and whether it's one of REAPER's own (Cockos's, a JSFX, a
/// container), which name no plug-in.
fn device_of(tag: &str, words: &[&str]) -> (SetDevice, bool) {
    let named = words.get(1).copied().unwrap_or_default();
    match tag {
        "JS" => {
            let midi = named.to_ascii_lowercase().starts_with("midi");
            (SetDevice { name: named.into(), role: if midi { DeviceRole::Midi } else { DeviceRole::Audio }, ..Default::default() }, true)
        }
        "CONTAINER" => {
            let renamed = words.get(3).copied().filter(|renamed| !renamed.is_empty());
            (SetDevice { name: "Container".into(), role: DeviceRole::Rack, preset: renamed.map(Into::into), ..Default::default() }, true)
        }
        _ => {
            // "VSTi: ReaSynth (Cockos)", "AUi: Apple: DLSMusicDevice", "CLAP: Surge XT Effects (Surge Synth Team)".
            let (kind, rest) = named.split_once(": ").unwrap_or(("", named));
            let (name, maker) = match rest.rsplit_once(" (") {
                Some((name, maker)) if maker.ends_with(')') => (name, &maker[..maker.len() - 1]),
                _ => match rest.split_once(": ") {
                    Some((maker, name)) if kind.starts_with("AU") => (name, maker),
                    _ => (rest, ""),
                },
            };
            let role = if kind.ends_with('i') { DeviceRole::Instrument } else { DeviceRole::Audio };
            let own = maker == "Cockos";
            // A VST's own name for this one, given in REAPER (its line's fifth word).
            let renamed = (tag == "VST").then(|| words.get(4).copied()).flatten().filter(|renamed| !renamed.is_empty());
            let device = SetDevice {
                name: name.into(),
                role,
                plugin: (!own).then(|| named.into()),
                preset: renamed.map(Into::into),
                ..Default::default()
            };
            (device, own)
        }
    }
}

/// A track while it's read: what it is so far, and its folder's start (the line's first `ISBUS` word is 1) and how many
/// folders end after it (its second, negative).
#[derive(Default)]
struct TrackRead {
    track: SetTrack,
    folder: bool,
    closes: usize,
    midi: bool,
}

/// What's read so far.
#[derive(Default)]
struct Read {
    set: SetSummary,
    blocks: Vec<Block>,
    track: Option<TrackRead>,
    /// The folders open where the next track comes, by name, innermost last.
    folders: Vec<String>,
    /// The FX a chain has, the master's or a track's, and a container's inside it; the one last closed, which a
    /// `PRESETNAME` line after it names.
    devices: Vec<SetDevice>,
    containers: Vec<SetDevice>,
    master: bool,
    item: (f64, f64),
    end: f64,
    tempo: Option<f64>,
    folder: String,
    /// How much has been kept, against `MOST_KEPT`.
    kept: usize,
}

impl Read {
    fn open(&mut self, tag: &str, words: &[&str]) {
        let inside = self.blocks.last().copied();
        let block = match (inside, tag) {
            (Some(Block::Project), "TRACK") => {
                self.track =
                    Some(TrackRead { track: SetTrack { group: self.folders.last().cloned(), ..Default::default() }, ..Default::default() });
                Block::Track
            }
            (Some(Block::Project), "MASTERFXLIST") => {
                self.master = true;
                self.devices.clear();
                Block::Chain
            }
            (Some(Block::Track), "FXCHAIN") => {
                self.devices.clear();
                Block::Chain
            }
            (Some(Block::Chain) | Some(Block::Container), "CONTAINER") => {
                let (device, _) = device_of(tag, words);
                self.containers.push(device);
                Block::Container
            }
            (Some(Block::Chain) | Some(Block::Container), "VST" | "AU" | "JS" | "CLAP" | "DX" | "LV2") => {
                let (device, own) = device_of(tag, words);
                self.add(device, own);
                Block::Fx
            }
            (Some(Block::Track), "ITEM") => {
                self.item = (0., 0.);
                if let Some(track) = self.track.as_mut() {
                    track.track.clips.arrangement += 1;
                }
                Block::Item
            }
            (Some(Block::Item) | Some(Block::Audio), "SOURCE") => match words.get(1).copied().unwrap_or_default() {
                "MIDI" | "MIDIPOOL" => {
                    if let Some(track) = self.track.as_mut() {
                        track.midi = true;
                    }
                    Block::Midi
                }
                "VIDEO" | "VIDEOEFFECT" | "EMPTY" | "CLICK" | "TIMECODE" | "LTC" => Block::Other,
                _ => Block::Audio,
            },
            (Some(Block::Track), "FREEZE") => {
                if let Some(track) = self.track.as_mut() {
                    track.track.frozen = Some(true);
                }
                Block::Frozen
            }
            _ => Block::Other,
        };
        self.blocks.push(block);
    }

    /// An FX in the chain, or inside the container open; a plug-in's name on the track too.
    fn add(&mut self, device: SetDevice, own: bool) {
        self.kept += 1;
        let master = self.master;
        if let (false, false, Some(track)) = (own, master, self.track.as_mut()) {
            let plugins = track.track.plugins.get_or_insert_with(Vec::new);
            if !plugins.contains(&device.name) {
                plugins.push(device.name.clone());
            }
        }
        if device.role == DeviceRole::Instrument {
            if let Some(track) = self.track.as_mut() {
                track.midi = true;
            }
        }
        match self.containers.last_mut() {
            Some(container) => {
                let inside = container.inside.get_or_insert_with(Vec::new);
                if inside.len() < MOST_INSIDE {
                    inside.push(device.name);
                }
            }
            None => self.devices.push(device),
        }
    }

    fn close(&mut self) {
        match self.blocks.pop() {
            Some(Block::Container) => {
                if let Some(container) = self.containers.pop() {
                    self.add(container, true);
                }
            }
            Some(Block::Chain) if self.master => {
                self.master = false;
                let devices = std::mem::take(&mut self.devices);
                self.set.main = Some(SetTrack { name: "Master".into(), kind: TrackKind::Main, devices, ..Default::default() });
            }
            Some(Block::Chain) => {
                if let Some(track) = self.track.as_mut() {
                    track.track.devices = std::mem::take(&mut self.devices);
                }
            }
            Some(Block::Item) => self.end = self.end.max(self.item.0 + self.item.1),
            Some(Block::Track) => {
                let Some(mut read) = self.track.take() else { return };
                self.kept += 1;
                // REAPER's tracks have no kind: one with audio in it is said to, and one without says nothing (`my_sets`
                // leaves out another DAW's "midi").
                read.track.kind = if read.folder {
                    TrackKind::Group
                } else if !read.midi && !read.track.samples.is_empty() {
                    TrackKind::Audio
                } else {
                    TrackKind::Midi
                };
                if read.folder {
                    self.folders.push(read.track.name.clone());
                }
                for _ in 0..read.closes {
                    self.folders.pop();
                }
                self.set.tracks.push(read.track);
            }
            _ => {}
        }
    }

    /// A line in a block that isn't another block's start or end: only some, in some blocks, say anything.
    fn line(&mut self, line: &str) {
        let key = line.split_ascii_whitespace().next().unwrap_or_default();
        match (self.blocks.last().copied(), key) {
            (Some(Block::Project), "TEMPO") => {
                let said = words(line);
                self.tempo = said.get(1).and_then(|bpm| bpm.parse().ok());
                self.set.tempo = self.tempo;
                if let (Some(top), Some(bottom)) = (said.get(2), said.get(3)) {
                    self.set.signature = Some(format!("{top}/{bottom}"));
                }
            }
            (Some(Block::Track), "NAME") => {
                if let Some(track) = self.track.as_mut() {
                    track.track.name = words(line).get(1).copied().unwrap_or_default().into();
                }
            }
            (Some(Block::Track), "ISBUS") => {
                let said = words(line);
                if let Some(track) = self.track.as_mut() {
                    track.folder = said.get(1) == Some(&"1");
                    track.closes =
                        said.get(2).and_then(|n| n.parse::<i64>().ok()).filter(|n| *n < 0).map_or(0, |n| n.unsigned_abs() as usize);
                }
            }
            (Some(Block::Item), "POSITION") => self.item.0 = words(line).get(1).and_then(|at| at.parse().ok()).unwrap_or(0.),
            (Some(Block::Item), "LENGTH") => self.item.1 = words(line).get(1).and_then(|at| at.parse().ok()).unwrap_or(0.),
            (Some(Block::Audio), "FILE") => {
                // (a path cut short would name another file)
                let Some(file) = words(line).get(1).copied().filter(|file| !file.is_empty() && file.len() < MOST_VALUE) else { return };
                let lower = file.to_ascii_lowercase();
                if !SAMPLE_EXTENSIONS.iter().any(|extension| lower.ends_with(extension)) {
                    return;
                }
                // REAPER finds media named relative to the project's folder there.
                let file = if is_absolute(file) { file.to_string() } else { join(&self.folder, file) };
                if let Some(track) = self.track.as_mut() {
                    if track.track.samples.len() < MOST_SAMPLES && !track.track.samples.contains(&file) {
                        track.track.samples.push(file);
                        self.kept += 1;
                    }
                }
            }
            // An FX's preset, after its block (before the next's).
            (Some(Block::Chain) | Some(Block::Container), "PRESETNAME") => {
                let named = words(line).get(1).map(|named| named.to_string()).filter(|named| !named.is_empty());
                let last = match self.containers.last_mut() {
                    Some(_) => None,
                    None => self.devices.last_mut(),
                };
                if let (Some(device), Some(named)) = (last, named) {
                    device.preset.get_or_insert(named);
                }
            }
            _ => {}
        }
    }
}

/// The next line into `bytes`, as much of it as `MOST_LINE`, the rest passed over; false at the end of the file.
fn next_line(lines: &mut impl BufRead, bytes: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut any = false;
    loop {
        let buffer = lines.fill_buf()?;
        if buffer.is_empty() {
            return Ok(any);
        }
        any = true;
        let end = buffer.iter().position(|byte| *byte == b'\n');
        let used = end.map_or(buffer.len(), |at| at + 1);
        let room = MOST_LINE.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..used.min(room)]);
        lines.consume(used);
        if end.is_some() {
            return Ok(true);
        }
    }
}

/// The project in `lines`, named `name` in `folder`; none when it doesn't start as one, or holds more than a project does.
fn read_lines(mut lines: impl BufRead, name: &str, folder: &str, signal: &Signal) -> Result<Option<SetSummary>, RuntimeError> {
    let mut read = Read {
        set: SetSummary { name: name.into(), daw: Some(DawId::Reaper), ..Default::default() },
        folder: folder.into(),
        ..Default::default()
    };
    let mut bytes = Vec::new();
    let mut count = 0usize;
    loop {
        bytes.clear();
        if !next_line(&mut lines, &mut bytes).map_err(|why| RuntimeError::plain(why.to_string()))? {
            break;
        }
        count += 1;
        if count % CHECK_EVERY == 0 && signal.is_cancelled() {
            return Err(RuntimeError::Aborted);
        }
        if read.kept > MOST_KEPT || read.blocks.len() > MOST_DEPTH {
            return Ok(None);
        }
        let start = bytes.iter().position(|byte| !byte.is_ascii_whitespace()).unwrap_or(bytes.len());
        let first = bytes.get(start).copied();
        if count == 1 {
            if !bytes[start..].starts_with(b"<REAPER_PROJECT") {
                return Ok(None);
            }
            // The REAPER that saved it: `<REAPER_PROJECT 0.1 "7.82/macOS-arm64" …`.
            let line = String::from_utf8_lossy(&bytes[start..]);
            read.set.made_in =
                words(&line).get(2).and_then(|saved| saved.split('/').next()).filter(|v| !v.is_empty()).map(|v| format!("REAPER {v}"));
            read.blocks.push(Block::Project);
            continue;
        }
        match first {
            Some(b'>') => read.close(),
            Some(b'<') => {
                let inside = read.blocks.last().copied();
                // A block inside one whose lines say nothing (MIDI, plug-in state) is passed over too.
                if matches!(inside, Some(Block::Other | Block::Midi | Block::Fx | Block::Frozen)) {
                    read.blocks.push(Block::Other);
                    continue;
                }
                let line = String::from_utf8_lossy(&bytes[start..]);
                let said = words(&line[1..]);
                let tag = said.first().copied().unwrap_or_default();
                read.open(tag, &said);
            }
            Some(_) => {
                if !matches!(
                    read.blocks.last(),
                    Some(Block::Project | Block::Track | Block::Item | Block::Audio | Block::Chain | Block::Container)
                ) {
                    continue;
                }
                let line = String::from_utf8_lossy(&bytes[start..]);
                read.line(line.trim_end());
            }
            None => {}
        }
    }
    // (an empty file: its first line would have said)
    if count == 0 {
        return Ok(None);
    }
    if read.set.main.is_none() {
        read.set.main = Some(SetTrack { name: "Master".into(), kind: TrackKind::Main, ..Default::default() });
    }
    if let (Some(tempo), true) = (read.tempo, read.end > 0.) {
        read.set.arrangement_beats = Some((read.end * tempo / 60. * 1000.).round() / 1000.);
    }
    Ok(Some(read.set))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = r#"<REAPER_PROJECT 0.1 "7.82/macOS-arm64" 1792451282 0
  <NOTES 0 2
  >
  TEMPO 98 7 8 0
  <MASTERFXLIST
    SHOW 0
    BYPASS 0 0 0
    <VST "VST: ReaLimit (Cockos)" realimit.vst.dylib 0 "" 1919250529<56535472656C6D7265616C696D6974> ""
      bGltae5e7f4CAAAAAQAAAAAAAAACAAAAAAAAAAIAAAABAAAAAAAAAAIAAAAAAAAA
    >
    FXID {M}
  >
  <TRACK {A}
    NAME "Drum Bus"
    ISBUS 1 1
    <FXCHAIN
      BYPASS 0 0 0
      <VST "VST3: Pro-C 2 (FabFilter)" "FabFilter Pro-C 2.vst3" 0 "Glue" 1234<5653> ""
        AAAA
      >
      PRESETNAME "Gentle Glue"
      FXID {B}
    >
  >
  <TRACK {C}
    NAME Kick
    ISBUS 2 -1
    <ITEM
      POSITION 2
      LENGTH 4.5
      <SOURCE WAVE
        FILE "Media/kick.wav"
      >
    >
    <ITEM
      POSITION 0
      LENGTH 1
      <SOURCE SECTION
        LENGTH 1
        <SOURCE WAVE
          FILE "/Samples/Kick 2.WAV"
        >
      >
    >
  >
  <TRACK {D}
    NAME 'Lead "Hook"'
    ISBUS 0 0
    <FXCHAIN
      BYPASS 0 0 0
      <VST "VSTi: Serum (Xfer Records)" Serum.vst 0 "" 5678<5653> ""
        AAAA
        BBBB
      >
      <CONTAINER Container 0 "Space" 0
        <JS "Utility/volume" ""
          0 - -
        >
        <VST "VST: ReaVerbate (Cockos)" reaverbate.vst.dylib 0 "" 9<5653> ""
          CCCC
        >
      >
      <JS "MIDI/midi_transpose" ""
        0 - -
      >
    >
    <ITEM
      POSITION 8
      LENGTH 2
      <SOURCE MIDI
        HASDATA 1 960 QN
        E 0 90 3c 64
        E 480 80 3c 00
        <X 0 0
          8H5/CQH3
        >
        FILE "not/a/sample.wav"
      >
    >
    <VOLENV2
      PT 0 1 0
      PT 4 0.5 0
    >
    <FREEZE 0
      <ITEM
        POSITION 0
      >
    >
  >
>
"#;

    fn read_text(text: &[u8]) -> Option<SetSummary> {
        read_lines(text, "Song", "/Projects/Song", &Signal::new()).unwrap()
    }

    #[test]
    fn a_project_says_its_tracks_folders_fx_media_master_and_tempo() {
        let set = read_text(PROJECT.as_bytes()).unwrap();
        assert_eq!(set.name, "Song");
        assert_eq!(set.daw, Some(DawId::Reaper));
        assert_eq!(set.made_in.as_deref(), Some("REAPER 7.82"));
        assert_eq!(set.live, None, "Live's version alone");
        assert_eq!((set.tempo, set.signature.as_deref()), (Some(98.), Some("7/8")));
        // The last item ends at 10 s: 16.333 beats at 98 BPM.
        assert_eq!(set.arrangement_beats, Some(16.333));
        let master = set.main.as_ref().unwrap();
        assert_eq!((master.name.as_str(), master.kind), ("Master", TrackKind::Main));
        assert_eq!(master.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["ReaLimit"]);
        assert_eq!(master.devices[0].plugin, None, "Cockos's own name no plug-in");
        let names: Vec<_> = set.tracks.iter().map(|t| (t.name.as_str(), t.kind, t.group.as_deref())).collect();
        assert_eq!(
            names,
            [("Drum Bus", TrackKind::Group, None), ("Kick", TrackKind::Audio, Some("Drum Bus")), ("Lead \"Hook\"", TrackKind::Midi, None)]
        );
        // A plug-in, renamed in REAPER and with a preset: the preset REAPER shows is its own name for it.
        let bus = &set.tracks[0];
        let glue = &bus.devices[0];
        assert_eq!(
            (glue.name.as_str(), glue.plugin.as_deref(), glue.preset.as_deref()),
            ("Pro-C 2", Some("VST3: Pro-C 2 (FabFilter)"), Some("Glue"))
        );
        assert_eq!(bus.plugins.as_deref(), Some(&["Pro-C 2".to_string()][..]));
        // Media, the project's own found in its folder; a section's inner source too; MIDI's file isn't a sample.
        let kick = &set.tracks[1];
        assert_eq!(kick.samples, ["/Projects/Song/Media/kick.wav", "/Samples/Kick 2.WAV"]);
        assert_eq!(kick.clips.arrangement, 2);
        let lead = &set.tracks[2];
        assert!(lead.samples.is_empty());
        assert_eq!(lead.frozen, Some(true));
        assert_eq!(lead.clips.arrangement, 1, "a frozen item isn't the track's");
        let devices: Vec<_> = lead.devices.iter().map(|d| (d.name.as_str(), d.role)).collect();
        assert_eq!(
            devices,
            [("Serum", DeviceRole::Instrument), ("Container", DeviceRole::Rack), ("MIDI/midi_transpose", DeviceRole::Midi)]
        );
        let space = &lead.devices[1];
        assert_eq!(
            (space.preset.as_deref(), space.inside.as_deref()),
            (Some("Space"), Some(&["Utility/volume".to_string(), "ReaVerbate".to_string()][..]))
        );
        assert_eq!(lead.plugins.as_deref(), Some(&["Serum".to_string()][..]));
    }

    #[test]
    fn words_not_in_utf8_are_read_as_near_as_they_come() {
        let mut text = b"<REAPER_PROJECT 0.1 \"7.82/win64\" 1 0\n  <TRACK {A}\n    NAME \"Caf".to_vec();
        text.extend_from_slice(&[0xE9, b' ', 0xFF]);
        text.extend_from_slice(b"\"\n  >\n>\n");
        let set = read_text(&text).unwrap();
        assert_eq!(set.tracks[0].name, "Caf\u{FFFD} \u{FFFD}");
    }

    #[test]
    fn a_file_thats_not_a_project_is_said_so_and_a_big_one_isnt_read() {
        assert!(read_text(b"<RENOISE_SONG>\n").is_none());
        assert!(read_text(b"").is_none());
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("Big.rpp");
        std::fs::write(&file, PROJECT).unwrap();
        let error = read_at_most(&file, &Signal::new(), 64).unwrap_err();
        assert!(error.to_string().contains("Big.rpp is bigger than Kumi reads"), "{error}");
        assert_eq!(read_project(&file, &Signal::new()).unwrap().tracks.len(), 3);
        std::fs::write(&file, "not a project").unwrap();
        assert!(read_project(&file, &Signal::new()).unwrap_err().to_string().contains("isn't a REAPER project"));
    }

    #[test]
    fn a_big_plug_in_state_doesnt_lose_the_project_and_what_is_kept_is_bounded() {
        // A line of a plug-in's state far past what's held, and a track's name and a preset past what's kept.
        let long = "A".repeat(MOST_LINE + 4096);
        let state = PROJECT.replace("bGltae5e7f4CAAAAAQAAAAAAAAACAAAAAAAAAAIAAAABAAAAAAAAAAIAAAAAAAAA", &long);
        assert_ne!(state, PROJECT);
        assert_eq!(read_text(state.as_bytes()).unwrap().tracks.len(), 3);
        let mut bytes = vec![];
        let lines = format!("{long}\nnext\n");
        let mut reader = lines.as_bytes();
        assert!(next_line(&mut reader, &mut bytes).unwrap());
        assert_eq!(bytes.len(), MOST_LINE);
        bytes.clear();
        assert!(next_line(&mut reader, &mut bytes).unwrap());
        assert_eq!(bytes, b"next\n");
        let named = format!(
            "<REAPER_PROJECT 0.1 \"7.82/macOS-arm64\" 1 0\n  <TRACK {{A}}\n    NAME \"{}é\"\n    <ITEM\n      <SOURCE WAVE\n        FILE \"/{}.wav\"\n      >\n    >\n  >\n>\n",
            "n".repeat(MOST_VALUE - 1),
            "f".repeat(MOST_VALUE),
        );
        let set = read_text(named.as_bytes()).unwrap();
        // Cut on a character, and a path too long to keep whole isn't kept.
        assert_eq!(set.tracks[0].name.len(), MOST_VALUE - 1);
        assert!(set.tracks[0].samples.is_empty());
        // More than a project holds, or deeper: it isn't read as one.
        let many = format!("<REAPER_PROJECT 0.1 \"7.82\" 1 0\n{}>\n", "  <TRACK {A}\n  >\n".repeat(MOST_KEPT + 2));
        assert!(read_text(many.as_bytes()).is_none());
        let deep = format!("<REAPER_PROJECT 0.1 \"7.82\" 1 0\n{}", "<A\n".repeat(MOST_DEPTH + 2));
        assert!(read_text(deep.as_bytes()).is_none());
    }

    /// The whole way: a project in a folder the producer named is learned through REAPER's library, leaving its copies, and
    /// `my_sets` in REAPER finds it by what's inside it and shows it whole; Live doesn't.
    #[tokio::test]
    async fn my_sets_in_reaper_finds_a_project_the_library_learned() {
        use kumi_library::{
            create_library, daws::Daws, daws::LibrarySources, sources::SourceOptions, LearnNowOptions, LibraryOptions, LibraryToolsOptions,
        };
        use kumi_types::daw::DawProfile;
        use serde_json::{json, Value};
        tokio::task::LocalSet::new()
            .run_until(async {
                let home = tempfile::tempdir().unwrap();
                let music = home.path().join("Projects");
                std::fs::create_dir_all(music.join("Backups")).unwrap();
                for name in ["Song.rpp", "Song (Kumi copy 2026-10-20 14.05.31 +0900).rpp", "Backups/Song.rpp", "Song-2026-10-19_183654.rpp"]
                {
                    std::fs::write(music.join(name), PROJECT).unwrap();
                }
                static OWN: [&'static dyn LibrarySources; 1] = [&crate::library::LIBRARY];
                let library = create_library(LibraryOptions {
                    daws: Daws(&OWN),
                    dir: home.path().join("library").to_string_lossy().into(),
                    folders: Some(vec![music.to_string_lossy().into()]),
                    sources: Some(SourceOptions {
                        home: Some(home.path().to_string_lossy().into()),
                        daw: Some(DawId::Reaper),
                        ..Default::default()
                    }),
                    fork: Some(false),
                    workers: Some(0),
                    find_sets: Some(false),
                    delay_ms: Some(0),
                    ..Default::default()
                });
                let learned = library.learn_now(LearnNowOptions::new(Signal::new())).await.unwrap().unwrap();
                assert_eq!(learned.sets.known, 1, "the project, not its copy, backup or auto-save");
                let my_sets = |profile: &'static DawProfile, input: Value| {
                    let tools = library.tools_for(profile, LibraryToolsOptions::default());
                    async move {
                        let tool = tools.iter().find(|tool| tool.name() == "my_sets").expect("my_sets");
                        let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
                        serde_json::from_str::<Value>(&result.text).unwrap_or(Value::String(result.text))
                    }
                };
                let found = my_sets(&DawProfile::REAPER, json!({"words": ["serum"]})).await;
                assert_eq!(found["sets"][0]["name"], "Song", "{found}");
                let song = my_sets(&DawProfile::REAPER, json!({"set": "song.rpp"})).await;
                assert_eq!(song["madeIn"], "REAPER 7.82", "{song}");
                let tracks = song["tracks"].as_array().unwrap();
                let track = |name: &str| tracks.iter().find(|track| track["name"] == name).unwrap_or_else(|| panic!("{name}: {song}"));
                assert_eq!(track("Drum Bus")["kind"], "group");
                assert_eq!(track("Kick")["kind"], "audio");
                assert_eq!(track("Kick")["group"], "Drum Bus");
                assert_eq!(track("Lead \"Hook\"").get("kind"), None, "REAPER's tracks have no MIDI kind");
                assert_eq!(song.get("scenes"), None);
                assert_eq!(my_sets(&DawProfile::ABLETON, json!({})).await["known"], 0, "a REAPER project isn't one of Live's Sets");
                library.close().await;
            })
            .await;
    }

    #[test]
    fn a_read_stopped_ends_with_aborted() {
        let mut text = PROJECT.as_bytes().to_vec();
        text.splice(PROJECT.find("  <TRACK").unwrap()..PROJECT.find("  <TRACK").unwrap(), b"  X\n".repeat(CHECK_EVERY * 2));
        let signal = Signal::new();
        signal.cancel();
        assert!(read_lines(&text[..], "Song", "/", &signal).unwrap_err().is_aborted());
    }

    #[test]
    fn copies_backups_and_auto_saves_arent_the_producers_projects() {
        for (path, skipped) in [
            ("/Music/Song/Song.rpp", false),
            ("/Music/Song/Song_001.rpp", false),
            ("/Music/Song/Song (Kumi copy 2026-10-20 14.05.31 +0900).rpp", true),
            ("/Music/Song/._Song.rpp", true),
            ("/Music/Song/Backups/Song.rpp", true),
            ("/Music/Song/autosaves/Song.rpp", true),
            ("/Music/Song/Song-2026-10-19_183654.RPP", true),
            ("/Music/Song/Song-2026-10-19_1836.rpp", true),
        ] {
            assert_eq!(skip(Path::new(path)), skipped, "{path}");
        }
    }
}
