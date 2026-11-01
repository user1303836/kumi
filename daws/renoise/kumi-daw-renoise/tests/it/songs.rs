//! Renoise's songs as `my_sets` reads them: zip archives written here as Renoise writes them, with a `Song.xml` in the form
//! of its schema (the parts Kumi reads, and enough of the rest to pass over).

use std::io::Write;
use std::path::Path;

use flate2::{write::DeflateEncoder, Compression};
use kumi_common::abort::Signal;
use kumi_daw_renoise::songs::FORMAT;
use kumi_library::{
    create_library,
    daws::{Daws, LibrarySources},
    sets::{DeviceRole, SetSummary, TrackKind},
    sources::SourceOptions,
    LearnNowOptions, LibraryOptions, LibraryToolsOptions,
};
use kumi_types::daw::{DawId, DawProfile};
use serde_json::{json, Value};

/// A zip archive of these entries: each (name, contents), packed or stored, and with its sizes after its data or not.
fn archive(entries: &[(&str, &[u8], bool, bool)]) -> Vec<u8> {
    let (mut out, mut directory) = (vec![], vec![]);
    for (name, contents, packed, descriptor) in entries {
        let data = if *packed {
            let mut encoder = DeflateEncoder::new(vec![], Compression::default());
            encoder.write_all(contents).unwrap();
            encoder.finish().unwrap()
        } else {
            contents.to_vec()
        };
        let at = out.len() as u32;
        let flags: u16 = if *descriptor { 0x0008 } else { 0 };
        let method: u16 = if *packed { 8 } else { 0 };
        let local_sizes = |value: usize| if *descriptor { 0u32 } else { value as u32 };
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0]);
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0; 8]); // time, date, CRC: not read.
        out.extend_from_slice(&local_sizes(data.len()).to_le_bytes());
        out.extend_from_slice(&local_sizes(contents.len()).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b"xyz"); // An extra field only the local record has.
        out.extend_from_slice(&data);
        directory.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0]);
        directory.extend_from_slice(&flags.to_le_bytes());
        directory.extend_from_slice(&method.to_le_bytes());
        directory.extend_from_slice(&[0; 8]);
        directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
        directory.extend_from_slice(&(contents.len() as u32).to_le_bytes());
        directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
        directory.extend_from_slice(&[0; 12]);
        directory.extend_from_slice(&at.to_le_bytes());
        directory.extend_from_slice(name.as_bytes());
    }
    let (start, count) = (out.len() as u32, entries.len() as u16);
    out.extend_from_slice(&directory);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

const SONG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RenoiseSong doc_version="67">
  <GlobalSongData>
    <BeatsPerMin>127.5</BeatsPerMin>
    <LinesPerBeat>8</LinesPerBeat>
    <SignatureNumerator>7</SignatureNumerator>
    <SignatureDenominator>8</SignatureDenominator>
    <ShuffleAmounts><ShuffleAmount>0</ShuffleAmount></ShuffleAmounts>
    <SongName>What Renoise calls it</SongName>
  </GlobalSongData>
  <Instruments>
    <Instrument>
      <Name>Bass &amp; Sub</Name>
      <MacroMappings><Macro><Name>Not a sample</Name></Macro></MacroMappings>
      <PluginGenerator type="InstrumentPluginGenerator"/>
      <SampleGenerator>
        <Phrases><Phrase><Name>Not a sample either</Name></Phrase></Phrases>
        <Samples>
          <Sample><Name>SINE-C1</Name></Sample>
          <Sample><Name>SINE-C2</Name></Sample>
        </Samples>
      </SampleGenerator>
    </Instrument>
    <Instrument>
      <Name></Name>
      <SampleGenerator/>
    </Instrument>
    <Instrument>
      <Name>Pads</Name>
      <PluginGenerator>
        <PluginDevice type="AudioPluginDevice">
          <PluginType>VST3</PluginType>
          <PluginIdentifier>abc123</PluginIdentifier>
          <PluginDisplayName>VST3: Xfer Records: Serum</PluginDisplayName>
          <PluginShortDisplayName>Serum</PluginShortDisplayName>
        </PluginDevice>
      </PluginGenerator>
    </Instrument>
    <Instrument>
      <Samples><Sample><Name>From an older song</Name></Sample></Samples>
    </Instrument>
  </Instruments>
  <Tracks>
    <SequencerTrack type="SequencerTrack">
      <Name>Kick</Name>
      <GroupNestingLevel>2</GroupNestingLevel>
      <FilterDevices type="TrackDeviceChain">
        <Devices>
          <TrackMixerDevice type="TrackMixerDevice">
            <CustomDeviceName>Mixer</CustomDeviceName>
          </TrackMixerDevice>
          <BusCompressorDevice type="BusCompressorDevice">
            <SelectedPresetName>Init</SelectedPresetName>
            <Threshold><Value>-12</Value></Threshold>
          </BusCompressorDevice>
          <DooferDevice type="DooferDevice">
            <CustomDeviceName>Saturation</CustomDeviceName>
            <DeviceChain>
              <Devices>
                <DistortionDevice type="DistortionDevice"><CustomDeviceName>Not this name</CustomDeviceName></DistortionDevice>
                <AudioPluginDevice type="AudioPluginDevice"><PluginDisplayName>AU: Maker: Warm</PluginDisplayName><PluginShortDisplayName>Warm</PluginShortDisplayName></AudioPluginDevice>
              </Devices>
            </DeviceChain>
          </DooferDevice>
          <AudioPluginDevice type="AudioPluginDevice">
            <SelectedPresetName>Big Room</SelectedPresetName>
            <PluginType>AU</PluginType>
            <PluginDisplayName>AU: Maker: Shimmer</PluginDisplayName>
          </AudioPluginDevice>
        </Devices>
      </FilterDevices>
    </SequencerTrack>
    <SequencerTrack type="SequencerTrack">
      <Name>Hat</Name>
      <GroupNestingLevel>1</GroupNestingLevel>
    </SequencerTrack>
    <SequencerGroupTrack type="SequencerGroupTrack">
      <Name>Inner</Name>
      <GroupNestingLevel>1</GroupNestingLevel>
    </SequencerGroupTrack>
    <SequencerGroupTrack type="SequencerGroupTrack">
      <Name>Drums</Name>
      <GroupNestingLevel>0</GroupNestingLevel>
    </SequencerGroupTrack>
    <SequencerTrack type="SequencerTrack">
      <Name>Lead</Name>
      <GroupNestingLevel>0</GroupNestingLevel>
    </SequencerTrack>
    <SequencerMasterTrack type="SequencerMasterTrack">
      <Name>Mst</Name>
      <FilterDevices><Devices>
        <MasterTrackMixerDevice type="MasterTrackMixerDevice"/>
        <MaximizerDevice type="MaximizerDevice"/>
      </Devices></FilterDevices>
    </SequencerMasterTrack>
    <SequencerSendTrack type="SequencerSendTrack">
      <Name>S01</Name>
      <FilterDevices><Devices>
        <SendTrackMixerDevice type="SendTrackMixerDevice"/>
        <Eq5Device type="Eq5Device"/>
      </Devices></FilterDevices>
    </SequencerSendTrack>
  </Tracks>
  <PatternPool>
    <Patterns>
      <Pattern>
        <NumberOfLines>32</NumberOfLines>
        <Tracks><PatternTrack><Lines><Line><NoteColumns><NoteColumn><Note>C-4</Note></NoteColumn></NoteColumns></Line></Lines></PatternTrack></Tracks>
      </Pattern>
      <Pattern>
        <NumberOfLines>128</NumberOfLines>
      </Pattern>
      <Pattern>
      </Pattern>
    </Patterns>
  </PatternPool>
  <PatternSequence>
    <SequenceEntries>
      <SequenceEntry><Pattern>0</Pattern><IsSectionStart>true</IsSectionStart></SequenceEntry>
      <SequenceEntry><Pattern>1</Pattern></SequenceEntry>
      <SequenceEntry><Pattern>0</Pattern></SequenceEntry>
      <SequenceEntry><Pattern>2</Pattern></SequenceEntry>
    </SequenceEntries>
  </PatternSequence>
</RenoiseSong>"#;

fn read(bytes: &[u8]) -> Result<SetSummary, String> {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("Night Drive.xrns");
    std::fs::write(&path, bytes).unwrap();
    (FORMAT.read)(&path, &Signal::new()).map_err(|error| error.to_string())
}

#[test]
fn a_song_reads_as_its_tracks_devices_instruments_and_length() {
    let set = read(&archive(&[("Song.xml", SONG.as_bytes(), true, false)])).unwrap();
    assert_eq!(set.name, "Night Drive", "by its file, as a Live Set is");
    assert_eq!((set.tempo, set.signature.as_deref(), set.made_in.as_deref()), (Some(127.5), Some("7/8"), Some("Renoise")));
    assert_eq!(set.arrangement_beats, Some((32. + 128. + 32. + 64.) / 8.), "the sequence's patterns, at the song's lines per beat");
    let names = |tracks: &[kumi_library::sets::SetTrack]| tracks.iter().map(|track| track.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&set.tracks), ["Kick", "Hat", "Inner", "Drums", "Lead"]);
    assert_eq!(names(&set.returns), ["S01"]);
    assert_eq!(set.main.as_ref().map(|main| (main.name.as_str(), main.kind)), Some(("Mst", TrackKind::Main)));
    let kinds: Vec<_> = set.tracks.iter().map(|track| track.kind).collect();
    assert_eq!(kinds, [TrackKind::Midi, TrackKind::Midi, TrackKind::Group, TrackKind::Group, TrackKind::Midi]);
    assert_eq!(set.returns[0].kind, TrackKind::Return);
    let groups: Vec<_> = set.tracks.iter().map(|track| track.group.as_deref()).collect();
    assert_eq!(groups, [Some("Inner"), Some("Drums"), Some("Drums"), None, None], "a group comes after its tracks");
    // The mixer isn't listed; a device with the producer's name for it, what's inside it, and a plug-in.
    let devices = &set.tracks[0].devices;
    assert_eq!(devices.len(), 3);
    assert_eq!((devices[0].name.as_str(), devices[0].preset.as_deref()), ("Bus Compressor", None), "Init is no preset");
    assert_eq!((devices[1].name.as_str(), devices[1].preset.as_deref()), ("Doofer", Some("Saturation")));
    assert_eq!(devices[1].inside.as_deref(), Some(["Distortion".to_owned(), "Warm".to_owned()].as_slice()));
    assert_eq!(
        (devices[2].name.as_str(), devices[2].plugin.as_deref(), devices[2].preset.as_deref()),
        ("Shimmer", Some("AU"), Some("Big Room")),
        "without its short name, the long one's end"
    );
    assert_eq!(set.main.as_ref().unwrap().devices[0].name, "Maximizer");
    assert_eq!(set.returns[0].devices[0].name, "EQ 5");
    // Instruments with their samples or plug-in, and without the empty slot; one with no name by its number.
    let instruments = set.instruments.as_ref().unwrap();
    let rows: Vec<_> =
        instruments.iter().map(|one| (one.name.as_str(), one.plugin.as_deref(), one.inside.clone().unwrap_or_default())).collect();
    assert_eq!(
        rows,
        [
            ("Bass & Sub", None, vec!["SINE-C1".to_owned(), "SINE-C2".to_owned()]),
            ("Pads", Some("VST3"), vec!["Serum".to_owned()]),
            ("Instrument 03", None, vec!["From an older song".to_owned()]),
        ]
    );
    assert!(instruments.iter().all(|one| one.role == DeviceRole::Instrument));
}

#[test]
fn song_xml_is_found_by_the_archives_directory_however_the_archive_was_written() {
    let sample = vec![7u8; 4096];
    for entries in [
        // Stored; after the samples, as a producer who zipped the song again has it; with its sizes after its data.
        vec![("Song.xml", SONG.as_bytes(), false, false)],
        vec![
            ("SampleData/Instrument00 (Bass)/Sample00 (Sine).flac", sample.as_slice(), false, false),
            ("Song.xml", SONG.as_bytes(), true, false),
        ],
        vec![("Song.xml", SONG.as_bytes(), true, true), ("SampleData/x.flac", sample.as_slice(), false, false)],
    ] {
        assert_eq!(read(&archive(&entries)).unwrap().tempo, Some(127.5));
    }
    // A comment after the archive's end.
    let mut commented = archive(&[("Song.xml", SONG.as_bytes(), true, false)]);
    let length = commented.len();
    commented[length - 2..].copy_from_slice(&5u16.to_le_bytes());
    commented.extend_from_slice(b"hello");
    assert_eq!(read(&commented).unwrap().tempo, Some(127.5));
}

#[test]
fn only_a_songs_archive_reads_and_a_stop_ends_it() {
    let not_one = "That isn't a Renoise song.";
    assert_eq!(read(b"just text, long enough to have been a file worth trying to read here").unwrap_err(), not_one);
    assert_eq!(read(&[]).unwrap_err(), not_one);
    assert_eq!(read(&archive(&[("Instrument.xml", SONG.as_bytes(), true, false)])).unwrap_err(), not_one, "no Song.xml");
    assert_eq!(read(&archive(&[("Song.xml", b"<LiveSet></LiveSet>", true, false)])).unwrap_err(), not_one, "another document");
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("Song.xrns");
    std::fs::write(&path, archive(&[("Song.xml", SONG.as_bytes(), true, false)])).unwrap();
    let signal = Signal::new();
    signal.cancel();
    assert!(matches!((FORMAT.read)(&path, &signal), Err(kumi_types::errors::RuntimeError::Aborted)));
    assert!((FORMAT.read)(&home.path().join("Gone.xrns"), &Signal::new()).is_err());
}

#[test]
fn a_songs_copies_and_backups_arent_the_producers_songs() {
    let skip = |path: &str| (FORMAT.skip)(Path::new(path));
    assert!(!skip("/Music/Night Drive.xrns"));
    assert!(!skip("/Music/My Backups/Night Drive.xrns"));
    assert!(skip("/Music/_Backups/Night Drive.xrns"), "Renoise's own");
    assert!(skip("/Music/Kumi copies/Night Drive 2026-10-19 02.06.06.xrns"), "where Kumi kept its copies");
    assert!(skip("/Music/Night Drive (Kumi copy 2026-10-20 14.05.31 +0900).xrns"), "beside the song, where it keeps them now");
    assert!(!skip("/Music/Night Drive (Kumi copy 2026-10-20 14.05.31 +0900) final.xrns"), "a copy the producer named is theirs");
}

/// The whole way: a song in a folder the producer named is learned through Renoise's library, and `my_sets` in Renoise
/// finds it by what's inside it and shows it whole; Live doesn't.
#[tokio::test]
async fn my_sets_in_renoise_finds_a_song_the_library_learned() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let music = home.path().join("Songs");
            std::fs::create_dir_all(&music).unwrap();
            std::fs::write(music.join("Night Drive.xrns"), archive(&[("Song.xml", SONG.as_bytes(), true, false)])).unwrap();
            std::fs::write(
                music.join("Night Drive (Kumi copy 2026-10-20 14.05.31 +0900).xrns"),
                archive(&[("Song.xml", SONG.as_bytes(), true, false)]),
            )
            .unwrap();
            static OWN: [&'static dyn LibrarySources; 1] = [&kumi_daw_renoise::library::LIBRARY];
            let library = create_library(LibraryOptions {
                daws: Daws(&OWN),
                dir: home.path().join("library").to_string_lossy().into(),
                folders: Some(vec![music.to_string_lossy().into()]),
                sources: Some(SourceOptions {
                    home: Some(home.path().to_string_lossy().into()),
                    daw: Some(DawId::Renoise),
                    ..Default::default()
                }),
                fork: Some(false),
                workers: Some(0),
                find_sets: Some(false),
                delay_ms: Some(0),
                ..Default::default()
            });
            let learned = library.learn_now(LearnNowOptions::new(Signal::new())).await.unwrap().unwrap();
            assert_eq!(learned.sets.known, 1, "the song, not its copy");
            let my_sets = |profile: &'static DawProfile, input: Value| {
                let tools = library.tools_for(profile, LibraryToolsOptions::default());
                async move {
                    let tool = tools.iter().find(|tool| tool.name() == "my_sets").expect("my_sets");
                    let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
                    serde_json::from_str::<Value>(&result.text).unwrap_or(Value::String(result.text))
                }
            };
            let found = my_sets(&DawProfile::RENOISE, json!({"words": ["sine-c2"]})).await;
            assert_eq!(found["sets"][0]["name"], "Night Drive", "{found}");
            assert!(found["sets"][0]["why"].as_str().unwrap().contains("instrument “Bass & Sub” has SINE-C2"), "{found}");
            let song = my_sets(&DawProfile::RENOISE, json!({"set": "night drive.xrns"})).await;
            assert_eq!((&song["madeIn"], &song["arrangement"]), (&json!("Renoise"), &json!("5 bars")), "{song}");
            assert_eq!(song["instruments"][0], "Bass & Sub: SINE-C1, SINE-C2");
            assert_eq!(song["instruments"][1], "Pads [VST3]: Serum");
            assert_eq!(song["tracks"][0]["devices"][1], "Saturation (Doofer): Distortion, Warm");
            assert_eq!(song["tracks"][0]["devices"][2], "Big Room (Shimmer) [AU]");
            assert_eq!(song["tracks"][0]["group"], "Inner");
            assert_eq!(song["tracks"][0].get("clips"), None);
            assert_eq!(song.get("scenes"), None);
            assert_eq!(my_sets(&DawProfile::ABLETON, json!({})).await["known"], 0, "a Renoise song isn't one of Live's Sets");
            library.close().await;
        })
        .await;
}
