//! Renoise's songs read without opening Renoise, for `my_sets`: its tracks with their devices in order, its sends and
//! master, its instruments with their samples or plug-ins, and its tempo, signature and length. A song is a zip archive
//! of `Song.xml` beside its samples, which can be gigabytes: only `Song.xml` is read, as it streams out of the archive,
//! and a pattern's lines (most of the file) are passed over.
//!
//! The elements read are those of RenoiseSong's schema (`Resources/Schemas/RenoiseSong67.xsd` in Renoise), and of the older
//! forms a song keeps when it's only been opened since (samples and plug-ins directly in an instrument, the sequence's
//! patterns directly in the sequence).

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::DeflateDecoder;
use kumi_common::abort::Signal;
use kumi_daw_own::changes::is_kumi_copy;
use kumi_library::daws::ProjectFormat;
use kumi_library::sets::{DeviceRole, SetDevice, SetSummary, SetTrack, TrackKind};
use kumi_library::xml::{decode, scan_xml_stream, ScanOptions, TagHandler, XmlError};
use kumi_types::errors::RuntimeError;

/// Renoise's songs, as its library gives them to Kumi's.
pub static FORMAT: ProjectFormat = ProjectFormat { extension: ".xrns", skip: copy_or_backup, read: read_song };

/// Where Renoise keeps its own backups of a song, in a folder beside it (the string is in its binary, 3.5.4).
const BACKUPS: &str = "_Backups";
/// Where Kumi kept its copies of a song before they went beside it.
const OLD_COPIES: &str = "Kumi copies";

/// The copies and backups of a song, which aren't the producer's songs.
fn copy_or_backup(path: &Path) -> bool {
    let folder = path.parent().and_then(Path::file_name).map(|name| name.to_string_lossy());
    folder.is_some_and(|folder| folder.eq_ignore_ascii_case(BACKUPS) || folder.eq_ignore_ascii_case(OLD_COPIES))
        || path.file_name().is_some_and(|name| is_kumi_copy(&name.to_string_lossy()))
}

/// The most of a song's samples named under its instrument: enough to find it by one, in a line a model reads.
const MOST_SAMPLES: usize = 32;
/// The most of what's inside a Doofer or a Splitter named, as a Live rack's.
const MOST_INSIDE: usize = 16;
/// How much a song may hold of what's kept (tracks, devices, instruments, samples named, patterns, sequence slots). Renoise's limits are far
/// below it (255 instruments, 1000 patterns and slots): more is a damaged or hostile file, whose every read would pay for it.
const MOST_KEPT: usize = 100_000;
/// How much of `Song.xml` is read: what's kept is bounded above, and nothing else is held (a plug-in's state, an impulse).
const MOST_XML: u64 = 4 * 1024 * 1024 * 1024;

/// The end of a zip archive holds where its directory is; its comment, at most this long, may follow.
const END_RECORD: usize = 22;
const MOST_COMMENT: usize = 65_535;
/// The most of a directory read: a hundred thousand samples' names.
const MOST_DIRECTORY: u32 = 32 * 1024 * 1024;
const END: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const CENTRAL: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const LOCAL: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const CENTRAL_RECORD: usize = 46;
const LOCAL_RECORD: usize = 30;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
const ENCRYPTED: u16 = 0x0001;

fn not_a_song() -> RuntimeError {
    RuntimeError::plain("That isn't a Renoise song.")
}

fn failed(error: io::Error) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Where a zip archive's directory is and how long, from the record at its end (an archive over 4 GB has its own form,
/// which reads as no archive).
fn directory_of(file: &mut File) -> io::Result<Option<(u64, u32)>> {
    let length = file.seek(SeekFrom::End(0))?;
    let tail = length.min((END_RECORD + MOST_COMMENT) as u64);
    file.seek(SeekFrom::Start(length - tail))?;
    let mut bytes = vec![0; tail as usize];
    file.read_exact(&mut bytes)?;
    let Some(at) = (0..bytes.len().saturating_sub(END_RECORD - 1)).rev().find(|at| bytes[*at..].starts_with(&END)) else { return Ok(None) };
    let (size, offset) = (u32_at(&bytes, at + 12), u32_at(&bytes, at + 16));
    Ok((size != u32::MAX && offset != u32::MAX).then_some((u64::from(offset), size)))
}

/// `Song.xml` in the archive, by its directory: how it's packed, and where its local record is.
fn song_entry(file: &mut File) -> Result<(u16, u64, u32), RuntimeError> {
    let (offset, size) = directory_of(file).map_err(failed)?.ok_or_else(not_a_song)?;
    if size > MOST_DIRECTORY {
        return Err(not_a_song());
    }
    file.seek(SeekFrom::Start(offset)).map_err(failed)?;
    let mut directory = vec![0; size as usize];
    file.read_exact(&mut directory).map_err(|_| not_a_song())?;
    let mut at = 0;
    while directory.len() >= at + CENTRAL_RECORD && directory[at..].starts_with(&CENTRAL) {
        let (flags, method, packed) = (u16_at(&directory, at + 8), u16_at(&directory, at + 10), u32_at(&directory, at + 20));
        let (name, extra, comment) =
            (usize::from(u16_at(&directory, at + 28)), usize::from(u16_at(&directory, at + 30)), usize::from(u16_at(&directory, at + 32)));
        let local = u32_at(&directory, at + 42);
        let name_at = at + CENTRAL_RECORD;
        if directory.get(name_at..name_at + name) == Some(b"Song.xml".as_slice()) {
            if flags & ENCRYPTED != 0 || packed == u32::MAX || local == u32::MAX {
                return Err(not_a_song());
            }
            return Ok((method, u64::from(local), packed));
        }
        at = name_at + name + extra + comment;
    }
    Err(not_a_song())
}

fn read_song(path: &Path, signal: &Signal) -> Result<SetSummary, RuntimeError> {
    let mut file = File::open(path).map_err(failed)?;
    let (method, local, packed) = song_entry(&mut file)?;
    file.seek(SeekFrom::Start(local)).map_err(failed)?;
    let mut record = [0; LOCAL_RECORD];
    if file.read_exact(&mut record).is_err() || !record.starts_with(&LOCAL) {
        return Err(not_a_song());
    }
    // The local record's own lengths: its extra field needn't be the directory's.
    let skip = i64::from(u16_at(&record, 26)) + i64::from(u16_at(&record, 28));
    file.seek(SeekFrom::Current(skip)).map_err(failed)?;
    let packed = BufReader::new(file).take(u64::from(packed));
    let mut song = Song::default();
    let options = ScanOptions { signal: Some(signal.clone()), max_bytes: Some(MOST_XML) };
    let scanned = match method {
        STORED => scan_xml_stream(packed, &mut song, options),
        DEFLATED => scan_xml_stream(DeflateDecoder::new(packed), &mut song, options),
        _ => return Err(not_a_song()),
    };
    match scanned {
        Err(XmlError::Aborted(_)) => return Err(RuntimeError::Aborted),
        Err(error) => return Err(RuntimeError::plain(error.to_string())),
        Ok(()) if !song.saw_song || song.kept > MOST_KEPT => return Err(not_a_song()),
        Ok(()) => {}
    }
    let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(song.summary(name))
}

/// A device's name in Renoise, by its element, where that isn't the element's own words (as Renoise 3.5.4 names those it
/// still makes, and an older song's by its newer form's name).
fn device_name(tag: &str) -> String {
    let named = match tag {
        "Chorus2Device" => "Chorus",
        "Comb2Device" | "CombDevice" => "Comb Filter",
        "CrossoverDevice" => "#Multiband Send",
        "DcOffsetDevice" => "DC Offset",
        "Distortion2Device" => "Distortion",
        "Eq5Device" => "EQ 5",
        "Eq10Device" => "EQ 10",
        "Filter1Device" | "Filter2Device" | "Filter3Device" => "Filter",
        "Flanger2Device" => "Flanger",
        "FormulaMetaDevice" => "*Formula",
        "Gate2Device" => "Gate",
        "HydraDevice" => "*Hydra",
        "InstrumentAutomationDevice" => "*Instr. Automation",
        "InstrumentMacroDevice" => "*Instr. Macros",
        "KeyTrackingDevice" => "*Key Tracker",
        "LfoDevice" => "*LFO",
        "LineInDevice" => "#Line Input",
        "Lofi2Device" | "LofiDevice" => "LofiMat",
        "MetaMixerDevice" => "*Meta Mixer",
        "MidiCCDevice" => "*MIDI CC",
        "MidiControlDevice" => "*Instr. MIDI Control",
        "MixerEqDevice" => "Mixer EQ",
        "MultitapDevice" => "Multitap Delay",
        "NotePadDevice" => "Notepad",
        "Phaser2Device" => "Phaser",
        "Reverb2Device" | "Reverb3Device" => "mpReverb",
        "RingMod2Device" | "RingModDevice" => "RingMod",
        "SendDevice" => "#Send",
        "SideChainDevice" => "#Sidechain",
        "SignalFollowerDevice" => "*Signal Follower",
        "VelocityDevice" => "*Velocity Tracker",
        "XYPadDevice" => "*XY Pad",
        other => return words_of(other.strip_suffix("Device").unwrap_or(other)),
    };
    named.into()
}

/// "BusCompressor": "Bus Compressor".
fn words_of(tag: &str) -> String {
    let mut words = String::with_capacity(tag.len() + 2);
    for (at, letter) in tag.char_indices() {
        if at > 0 && letter.is_ascii_uppercase() {
            words.push(' ');
        }
        words.push(letter);
    }
    words
}

/// Every track's first device: its volume and panning, which Kumi doesn't list.
fn is_mixer(tag: &str) -> bool {
    tag.ends_with("TrackMixerDevice")
}

/// A track as it's read, with how deep it sits in groups.
struct ReadTrack {
    tag: String,
    level: usize,
    track: SetTrack,
}

/// A plug-in's type and names, as a song has them: "AU", "AU: Apple: DLSMusicDevice" and "DLSMusicDevice".
#[derive(Default)]
struct ReadPlugin {
    kind: Option<String>,
    display: Option<String>,
    short: Option<String>,
}

impl ReadPlugin {
    fn set(&mut self, field: &str, value: Option<String>) {
        match field {
            "PluginType" => self.kind = value,
            "PluginDisplayName" => self.display = value,
            "PluginShortDisplayName" => self.short = value,
            _ => {}
        }
    }
    fn name(&mut self) -> Option<String> {
        let display = self.display.take().map(|name| name.rsplit(": ").next().unwrap_or_default().to_owned());
        self.short.take().or(display).filter(|name| !name.is_empty())
    }
}

/// A device as it's read: what the producer named it, its preset, and a plug-in's type and name.
#[derive(Default)]
struct ReadDevice {
    name: String,
    custom: Option<String>,
    preset: Option<String>,
    plugin: ReadPlugin,
    inside: Vec<String>,
}

impl ReadDevice {
    fn finished(mut self) -> SetDevice {
        let name = self.plugin.name().unwrap_or(self.name);
        let preset = self.custom.or(self.preset.filter(|preset| preset != "Init"));
        SetDevice {
            name,
            role: DeviceRole::Audio,
            plugin: self.plugin.kind,
            preset,
            inside: (!self.inside.is_empty()).then_some(self.inside),
        }
    }
}

/// What's read of a song, as its tags stream past.
#[derive(Default)]
struct Song {
    saw_song: bool,
    done: bool,
    kept: usize,
    /// The elements open below the song, up to one not read ...
    path: Vec<String>,
    /// ... and how deep inside that one the stream is.
    passing: usize,
    text: String,
    tempo: Option<f64>,
    lines_per_beat: Option<f64>,
    numerator: Option<String>,
    denominator: Option<String>,
    instruments: Vec<ReadInstrument>,
    tracks: Vec<ReadTrack>,
    device: Option<ReadDevice>,
    pattern_lines: Vec<f64>,
    sequence: Vec<usize>,
}

#[derive(Default)]
struct ReadInstrument {
    name: String,
    plugin: ReadPlugin,
    samples: Vec<String>,
    sample_count: usize,
}

/// The most of one value kept: a name, a number.
const MOST_VALUE: usize = 1024;

/// Whether the song's element `name` inside `path` is read; all else is passed over whole.
fn read_inside(path: &[&str], name: &str) -> bool {
    let plugin = matches!(name, "PluginType" | "PluginDisplayName" | "PluginShortDisplayName");
    match path {
        [] => matches!(name, "GlobalSongData" | "Instruments" | "Tracks" | "PatternPool" | "PatternSequence"),
        ["GlobalSongData"] => matches!(name, "BeatsPerMin" | "LinesPerBeat" | "SignatureNumerator" | "SignatureDenominator"),
        ["Instruments"] => name == "Instrument",
        ["Instruments", "Instrument"] => matches!(name, "Name" | "PluginGenerator" | "PluginProperties" | "SampleGenerator" | "Samples"),
        ["Instruments", "Instrument", "PluginGenerator" | "PluginProperties"] => name == "PluginDevice",
        ["Instruments", "Instrument", "PluginGenerator" | "PluginProperties", "PluginDevice"] => plugin,
        ["Instruments", "Instrument", "SampleGenerator"] => name == "Samples",
        ["Instruments", "Instrument", "Samples"] | ["Instruments", "Instrument", "SampleGenerator", "Samples"] => name == "Sample",
        ["Instruments", "Instrument", "Samples", "Sample"] | ["Instruments", "Instrument", "SampleGenerator", "Samples", "Sample"] => {
            name == "Name"
        }
        ["Tracks"] => matches!(name, "SequencerTrack" | "SequencerGroupTrack" | "SequencerSendTrack" | "SequencerMasterTrack"),
        ["Tracks", _] => matches!(name, "Name" | "GroupNestingLevel" | "FilterDevices"),
        ["Tracks", _, "FilterDevices"] => name == "Devices",
        ["Tracks", _, "FilterDevices", "Devices"] => !is_mixer(name),
        ["Tracks", _, "FilterDevices", "Devices", _] => {
            plugin || matches!(name, "CustomDeviceName" | "SelectedPresetName" | "DeviceChain" | "DeviceChain0" | "DeviceChain1")
        }
        // A Doofer's or a Splitter's chains: what's inside, by name.
        ["Tracks", _, "FilterDevices", "Devices", _, "DeviceChain" | "DeviceChain0" | "DeviceChain1"] => name == "Devices",
        ["Tracks", _, "FilterDevices", "Devices", _, _, "Devices"] => true,
        ["Tracks", _, "FilterDevices", "Devices", _, _, "Devices", _] => name == "PluginShortDisplayName",
        ["PatternPool"] => name == "Patterns",
        ["PatternPool", "Patterns"] => name == "Pattern",
        ["PatternPool", "Patterns", "Pattern"] => name == "NumberOfLines",
        ["PatternSequence"] => matches!(name, "SequenceEntries" | "PatternSequence"),
        ["PatternSequence", "SequenceEntries"] => name == "SequenceEntry",
        ["PatternSequence", "SequenceEntries", "SequenceEntry"] | ["PatternSequence", "PatternSequence"] => name == "Pattern",
        _ => false,
    }
}

impl TagHandler for Song {
    fn open(&mut self, name: &str, _attrs: &str, _empty: bool) {
        if self.passing > 0 {
            self.passing += 1;
            return;
        }
        if !self.saw_song {
            self.saw_song = name == "RenoiseSong";
            self.done = !self.saw_song;
            return;
        }
        let path: Vec<&str> = self.path.iter().map(String::as_str).collect();
        if !read_inside(&path, name) {
            self.passing = 1;
            return;
        }
        if matches!(path.as_slice(), ["Instruments"] | ["Tracks"] | ["Tracks", _, "FilterDevices", "Devices"] | ["PatternPool", "Patterns"])
        {
            self.kept += 1;
            self.done |= self.kept > MOST_KEPT;
        }
        match path.as_slice() {
            ["Instruments"] => self.instruments.push(ReadInstrument::default()),
            ["Tracks"] => {
                let kind = match name {
                    "SequencerGroupTrack" => TrackKind::Group,
                    "SequencerSendTrack" => TrackKind::Return,
                    "SequencerMasterTrack" => TrackKind::Main,
                    // A sequencer track plays notes, on whichever instruments they name.
                    _ => TrackKind::Midi,
                };
                self.tracks.push(ReadTrack { tag: name.into(), level: 0, track: SetTrack { kind, ..Default::default() } });
            }
            ["Tracks", _, "FilterDevices", "Devices"] => {
                self.device = Some(ReadDevice { name: device_name(name), ..Default::default() });
            }
            ["Tracks", _, "FilterDevices", "Devices", _, _, "Devices"] => {
                if let Some(device) = self.device.as_mut().filter(|device| device.inside.len() < MOST_INSIDE) {
                    device.inside.push(device_name(name));
                }
            }
            ["PatternPool", "Patterns"] => self.pattern_lines.push(64.),
            _ => {}
        }
        self.path.push(name.into());
        self.text.clear();
    }

    fn close(&mut self, _name: &str) {
        if self.passing > 0 {
            self.passing -= 1;
            return;
        }
        let Some(closed) = self.path.pop() else { return };
        let value = decode(self.text.trim());
        self.text.clear();
        let path: Vec<String> = std::mem::take(&mut self.path);
        {
            let at: Vec<&str> = path.iter().map(String::as_str).collect();
            self.closed(&at, &closed, value);
        }
        self.path = path;
    }

    fn text(&mut self, text: &str) {
        if self.passing == 0 && self.text.len() < MOST_VALUE {
            self.text.push_str(text);
        }
    }

    fn stop(&mut self) -> bool {
        self.done
    }
}

impl Song {
    fn closed(&mut self, path: &[&str], name: &str, value: String) {
        let number = || value.parse::<f64>().ok().filter(|number| number.is_finite());
        let named = || Some(value.clone()).filter(|value| !value.is_empty());
        match (path, name) {
            (["GlobalSongData"], "BeatsPerMin") => self.tempo = number(),
            (["GlobalSongData"], "LinesPerBeat") => self.lines_per_beat = number(),
            (["GlobalSongData"], "SignatureNumerator") => self.numerator = named(),
            (["GlobalSongData"], "SignatureDenominator") => self.denominator = named(),
            (["Instruments", "Instrument"], "Name") => {
                if let Some(instrument) = self.instruments.last_mut() {
                    instrument.name = value;
                }
            }
            (["Instruments", "Instrument", _, "PluginDevice"], field) => {
                if let Some(instrument) = self.instruments.last_mut() {
                    instrument.plugin.set(field, named());
                }
            }
            ([.., "Sample"], "Name") => {
                if let Some(instrument) = self.instruments.last_mut() {
                    instrument.sample_count += 1;
                    if instrument.samples.len() < MOST_SAMPLES && !value.is_empty() {
                        instrument.samples.push(value);
                        self.kept += 1;
                        self.done |= self.kept > MOST_KEPT;
                    }
                }
            }
            (["Tracks", _], "Name") => {
                if let Some(track) = self.tracks.last_mut() {
                    track.track.name = value;
                }
            }
            (["Tracks", _], "GroupNestingLevel") => {
                if let Some(track) = self.tracks.last_mut() {
                    track.level = number().unwrap_or(0.) as usize;
                }
            }
            (["Tracks", _, "FilterDevices", "Devices"], _) => {
                if let (Some(device), Some(track)) = (self.device.take(), self.tracks.last_mut()) {
                    track.track.devices.push(device.finished());
                }
            }
            (["Tracks", _, "FilterDevices", "Devices", _], field) => {
                let Some(device) = &mut self.device else { return };
                match field {
                    "CustomDeviceName" => device.custom = named(),
                    "SelectedPresetName" => device.preset = named(),
                    field => device.plugin.set(field, named()),
                }
            }
            (["Tracks", _, "FilterDevices", "Devices", _, _, "Devices", _], "PluginShortDisplayName") => {
                if let (Some(device), Some(plugin)) = (&mut self.device, named()) {
                    if device.inside.len() < MOST_INSIDE {
                        if let Some(last) = device.inside.last_mut() {
                            *last = plugin;
                        }
                    }
                }
            }
            (["PatternPool", "Patterns", "Pattern"], "NumberOfLines") => {
                if let (Some(lines), Some(number)) = (self.pattern_lines.last_mut(), number()) {
                    *lines = number;
                }
            }
            ([.., "SequenceEntry"] | ["PatternSequence", "PatternSequence"], "Pattern") => {
                if let Some(pattern) = number().filter(|pattern| *pattern >= 0.) {
                    self.sequence.push(pattern as usize);
                    self.kept += 1;
                    self.done |= self.kept > MOST_KEPT;
                }
            }
            // All that's read comes before what's after the sequence (the pattern coder's, the mixer's view).
            ([], "PatternSequence") => self.done = true,
            _ => {}
        }
    }

    fn summary(self, name: String) -> SetSummary {
        let mut set = SetSummary { name, tempo: self.tempo, made_in: Some("Renoise".into()), ..Default::default() };
        if let (Some(numerator), Some(denominator)) = (&self.numerator, &self.denominator) {
            set.signature = Some(format!("{numerator}/{denominator}"));
        }
        // Lines at the song's own lines per beat: a pattern's ZL commands may change it as it plays.
        let lines: f64 = self.sequence.iter().filter_map(|pattern| self.pattern_lines.get(*pattern)).sum();
        set.arrangement_beats = Some(lines / self.lines_per_beat.filter(|lpb| *lpb > 0.).unwrap_or(4.)).filter(|beats| *beats > 0.);
        let instruments: Vec<_> = self.instruments.into_iter().enumerate().filter_map(|(index, read)| read.finished(index)).collect();
        set.instruments = (!instruments.is_empty()).then_some(instruments);
        // A group comes after its tracks, each a level deeper than it: from the end, the group of each level is known.
        let mut groups: Vec<String> = vec![];
        let mut tracks = self.tracks;
        for read in tracks.iter_mut().rev() {
            if read.level > 0 {
                read.track.group = groups.get(read.level - 1).cloned();
            }
            if read.tag == "SequencerGroupTrack" {
                groups.truncate(read.level);
                groups.push(read.track.name.clone());
            }
        }
        for read in tracks {
            match read.track.kind {
                TrackKind::Return => set.returns.push(read.track),
                TrackKind::Main => set.main = Some(read.track),
                _ => set.tracks.push(read.track),
            }
        }
        set
    }
}

impl ReadInstrument {
    /// What the song's instrument is, unless it's an empty slot.
    fn finished(mut self, index: usize) -> Option<SetDevice> {
        let plugin = self.plugin.name();
        if self.name.is_empty() && plugin.is_none() && self.sample_count == 0 {
            return None;
        }
        // Renoise numbers its instruments in hex, from 00.
        let name = if self.name.is_empty() { format!("Instrument {index:02X}") } else { self.name };
        let inside: Vec<_> = plugin.into_iter().chain(self.samples).collect();
        Some(SetDevice {
            name,
            role: DeviceRole::Instrument,
            plugin: self.plugin.kind,
            preset: None,
            inside: (!inside.is_empty()).then_some(inside),
        })
    }
}
