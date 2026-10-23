//! REAPER's and Renoise's own libraries: found where their Kumi bridges are, learned by each DAW's layout, and offered
//! only in that DAW (its presets in find_presets, its sounds in find_sounds). Every folder is a temporary one.
#[path = "../support/library.rs"]
mod fixture;
use async_trait::async_trait;
use fixture::{put, wav};
use kumi::daw::registry::library_sources;
use kumi_common::abort::Signal;
use kumi_library::{
    create_library,
    features::MeasureOptions,
    learn::{learn, library_logs, LearnOptions, LearnPlan, PresetEntry, SetEntry, SoundEntry},
    search::SoundIndex,
    sources::{Source, SourceKind, SourceOptions},
    tools::{library_tools_for, LearningState, LibraryAccess, LibraryToolsOptions},
    LibraryOptions,
};
use kumi_types::{daw::DawProfile, errors::RuntimeError};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, path::Path, rc::Rc};

/// A stored (unpacked) zip holding these files, as an `.xrni` is.
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in files {
        let at = out.len() as u32;
        let header = |signature: &[u8], central: bool| {
            let mut h = signature.to_vec();
            if central {
                h.extend([20, 0]);
            }
            h.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend([0, 0, 0, 0]);
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend([0, 0]);
            if central {
                // (comment length, disk, internal and external attributes)
                h.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                h.extend(at.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(header(b"PK\x03\x04", false));
        out.extend(*data);
        central.extend(header(b"PK\x01\x02", true));
    }
    let start = out.len() as u32;
    out.extend(&central);
    out.extend(b"PK\x05\x06\0\0\0\0");
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend([0, 0]);
    out
}

/// A home with Live's User Library, a REAPER resource folder and Renoise's libraries, each DAW with Kumi's bridge in it
/// when `bridges`.
struct Home {
    home: tempfile::TempDir,
    dir: tempfile::TempDir,
    resource: std::path::PathBuf,
}

impl Home {
    fn new(bridges: bool) -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = home.path();
        let resource = root.join("REAPER Lab");
        let renoise = root.join("Library/Application Support/Renoise/V3.5.4/Library");
        let user = root.join("Documents/Renoise/User Library");
        // Live's own preset: never in REAPER or Renoise.
        put(
            &root.join("Music/Ableton/User Library/Presets/Audio Effects/Vox Chain.adv"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Ableton MajorVersion=\"5\">\n  <Compressor2>\n  </Compressor2>\n</Ableton>\n",
        );
        // REAPER: chains, templates, JSFX (one a library without desc), a preset bank.
        put(
            &resource.join("FXChains/Vocals/Lead Vox.RfxChain"),
            "BYPASS 0 0 0\n<VST \"VST: ReaEQ (Cockos)\" reaeq.vst.dylib 0 \"\" 1919247729<5653547265716572656165710000000000000000>\nZXE=\n>\nBYPASS 0 0 0\n<VST \"VST: ReaComp (Cockos)\" reacomp.vst.dylib 0 \"\" 1919247213<56535472636d7072656163>\nY20=\n>\n<JS utility/volume \"\"\n0 -\n>\n",
        );
        put(
            &resource.join("TrackTemplates/Drum Bus.RTrackTemplate"),
            "<TRACK\n  NAME Kick\n  <FXCHAIN\n    <VST \"VST: ReaGate (Cockos)\" reagate.vst.dylib 0 \"\" 1\n    >\n  >\n>\n<TRACK\n  NAME \"Snare Top\"\n>\n",
        );
        put(
            &resource.join("ProjectTemplates/Mix Start.RPP"),
            "<REAPER_PROJECT 0.1 \"7.82\"\n  <TRACK\n    NAME Bass\n  >\n  <TRACK\n    NAME Keys\n  >\n>\n",
        );
        put(
            &resource.join("Effects/midi/midi_choke"),
            "// Copyright\ndesc:MIDI Choke\n//tags: MIDI processing\n//author: Cockos\n\nslider1:1<1,16,1>MIDI Channel\n@init\n",
        );
        put(&resource.join("Effects/lib/helper"), "// a library other effects import, with no desc of its own\nfunction f() ( 1; );\n");
        put(&resource.join("Effects/lib/notes.txt"), "not an effect, and long enough to be looked at by the walk at all");
        put(
            &resource.join("presets/vst-reacomp.ini"),
            "[General]\nNbPresets=2\n\n[Preset0]\nData=00\nLen=1\nName=stock - Rock Snare\n\n[Preset1]\nData=00\nLen=1\nName=stock - Vocal Leveler\n",
        );
        put(&resource.join("Scripts/not a preset.RfxChain"), "<VST \"VST: ReaEQ (Cockos)\" x 0 \"\" 1\n>\n");
        // Renoise: every kind of library file, a sample, and a song (not learned here).
        put(
            &renoise.join("Effect Presets/Compressor/Drums Hard.xrdp"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FilterDevicePreset doc_version=\"0\">\n  <DeviceSlot type=\"CompressorDevice\">\n    <Ratio><Value>10</Value></Ratio>\n  </DeviceSlot>\n</FilterDevicePreset>\n",
        );
        put(
            &renoise.join("Effect Chains/Bass/Amp Wide.xrnt"),
            "<?xml version=\"1.0\"?>\n<RenoiseDeviceChain doc_version=\"21\">\n<Devices>\n<LfoDevice type=\"LfoDevice\">\n<Amplitude><Value>1</Value></Amplitude>\n</LfoDevice>\n<DooferDevice type=\"DooferDevice\">\n<Devices>\n<DelayDevice type=\"DelayDevice\"/>\n</Devices>\n</DooferDevice>\n<Eq10Device type=\"Eq10Device\">\n</Eq10Device>\n</Devices>\n</RenoiseDeviceChain>\n",
        );
        put(
            &renoise.join("Instruments/Drums/Tight Kit.xrni"),
            zip(&[
                ("Instrument.xml", b"<RenoiseInstrument doc_version=\"34\"/>".as_slice()),
                ("SampleData/Sample00 (Kick Deep).flac", b"fLaC".as_slice()),
                ("SampleData/Sample01 (Snare).flac", b"fLaC".as_slice()),
            ]),
        );
        put(
            &renoise.join("Multi-Samples/Piano/Piano.sfz"),
            "<group>\n<region> sample=Piano C3.flac pitch_keycenter=48\n<region> sample=Piano C4.flac pitch_keycenter=60\n",
        );
        put(
            &renoise.join("Phrases/Arp/Up.xrnz"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<InstrumentPhrase doc_version=\"1\">\n  <SelectedPresetName>Arp - Up (16ths)</SelectedPresetName>\n</InstrumentPhrase>\n",
        );
        put(
            &renoise.join("Phrase Scripts/Euclid Rhythm.lua"),
            "-- Euclidean rhythms\n-- (three pulses in eight)\nreturn pattern { unit = \"1/16\", pulse = pulse.euclidean(3, 8) }\n",
        );
        put(
            &renoise.join("Modulation Sets/Filters/Old Filters.xrno"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<SampleModulationSet doc_version=\"2\">\n  <Name>Set 01</Name>\n</SampleModulationSet>\n",
        );
        put(&renoise.join("Samples/Drums/Renoise Kick.wav"), wav(&[fixture::kick(55., 0.2)], 44100));
        put(&renoise.join("Songs/Demo.xrns"), zip(&[("Song.xml", b"<RenoiseSong/>".as_slice())]));
        put(
            &user.join("Effect Presets/EQ 10/Drums Hard EQ.xrdp"),
            "<?xml version=\"1.0\"?>\n<FilterDevicePreset doc_version=\"0\">\n  <DeviceSlot type=\"Eq10Device\">\n  </DeviceSlot>\n</FilterDevicePreset>\n",
        );
        if bridges {
            put(&resource.join("UserPlugins/reaper_kumi.dylib"), "an extension");
            std::fs::create_dir_all(root.join("Library/Preferences/Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx")).unwrap();
        } else {
            std::fs::create_dir_all(root.join("Library/Preferences/Renoise/V3.5.4/Scripts/Tools")).unwrap();
        }
        Self { home, dir: tempfile::tempdir().unwrap(), resource }
    }

    fn options(&self) -> SourceOptions {
        let env: HashMap<String, String> =
            [("KUMI_REAPER_RESOURCE_DIR".to_string(), self.resource.to_string_lossy().into_owned())].into_iter().collect();
        SourceOptions {
            home: Some(self.home.path().to_string_lossy().into()),
            platform: Some("darwin".into()),
            applications: Some(self.home.path().join("Applications").to_string_lossy().into()),
            env: Some(env),
            ..Default::default()
        }
    }

    fn plan(&self) -> LearnPlan {
        LearnPlan {
            dir: self.dir.path().to_string_lossy().into(),
            sources: library_sources(&self.options()),
            set_folders: vec![],
            set_files: vec![],
            plugin_presets: vec![],
            workers: Some(0),
        }
    }
}

fn kinds_of(sources: &[Source]) -> Vec<(SourceKind, String)> {
    sources.iter().map(|s| (s.kind, s.label.clone())).collect()
}

#[test]
fn a_daws_library_is_a_source_only_where_kumis_bridge_is() {
    let with = Home::new(true);
    let sources = library_sources(&with.options());
    let kinds = kinds_of(&sources);
    assert!(kinds.contains(&(SourceKind::UserLibrary, "User Library".into())), "{kinds:?}");
    assert!(kinds.contains(&(SourceKind::Reaper, "REAPER".into())), "{kinds:?}");
    assert!(kinds.contains(&(SourceKind::Renoise, "Renoise Library".into())), "{kinds:?}");
    assert!(kinds.contains(&(SourceKind::Renoise, "Renoise User Library".into())), "{kinds:?}");
    let reaper = sources.iter().find(|s| s.kind == SourceKind::Reaper).unwrap();
    assert!(reaper.path.ends_with("REAPER Lab"), "{}", reaper.path);
    // Who sees what: a DAW's own library only there; the producer's folders everywhere; Live's when no DAW is known.
    use kumi_types::daw::DawId::*;
    let user = sources.iter().find(|s| s.kind == SourceKind::UserLibrary).unwrap();
    let renoise = sources.iter().find(|s| s.label == "Renoise Library").unwrap();
    assert!([None, Some(Ableton), Some(Reaper), Some(Renoise)].into_iter().all(|daw| user.offered_in(daw)));
    assert!(
        reaper.offered_in(Some(Reaper))
            && !reaper.offered_in(Some(Renoise))
            && !reaper.offered_in(Some(Ableton))
            && !reaper.offered_in(None)
    );
    assert!(renoise.offered_in(Some(Renoise)) && !renoise.offered_in(Some(Reaper)) && !renoise.offered_in(None));
    let pack = Source { path: "/p".into(), label: "Drum Essentials".into(), kind: SourceKind::Pack };
    assert!(pack.offered_in(None) && pack.offered_in(Some(Ableton)) && !pack.offered_in(Some(Reaper)));

    let without = Home::new(false);
    let kinds = kinds_of(&library_sources(&without.options()));
    assert!(!kinds.iter().any(|(kind, _)| matches!(kind, SourceKind::Reaper | SourceKind::Renoise)), "{kinds:?}");
}

#[test]
fn a_daws_library_inside_a_folder_the_producer_named_stays_its_own_source() {
    let home = Home::new(true);
    let mut options = home.options();
    options.folders = Some(vec![home.home.path().join("Documents").to_string_lossy().into()]);
    let sources = library_sources(&options);
    let kinds = kinds_of(&sources);
    assert!(kinds.contains(&(SourceKind::Folder, "Documents".into())), "{kinds:?}");
    assert!(kinds.contains(&(SourceKind::Renoise, "Renoise User Library".into())), "{kinds:?}");
}

/// The library as the tools read it: what was learned.
struct Learned {
    presets: Vec<PresetEntry>,
}

#[async_trait(?Send)]
impl LibraryAccess for Learned {
    async fn sounds(&self) -> Result<Rc<SoundIndex>, RuntimeError> {
        Ok(Rc::new(SoundIndex::new(Vec::<SoundEntry>::new(), &[])))
    }
    async fn presets(&self) -> Result<Vec<PresetEntry>, RuntimeError> {
        Ok(self.presets.clone())
    }
    async fn sets(&self) -> Result<Vec<SetEntry>, RuntimeError> {
        Ok(vec![])
    }
    fn learning(&self) -> LearningState {
        LearningState::default()
    }
    fn remember(&self, _: Vec<String>) {}
    async fn folders(&self) -> Vec<String> {
        vec![]
    }
    async fn measure(&self, _: String, _: MeasureOptions) -> Result<SoundEntry, RuntimeError> {
        Err(RuntimeError::plain("no"))
    }
}

async fn find_presets(access: &Rc<Learned>, profile: &DawProfile, input: Value) -> (Value, Value) {
    let tools = library_tools_for(profile, access.clone(), LibraryToolsOptions::default());
    let tool = tools.iter().find(|t| t.name() == "find_presets").unwrap_or_else(|| panic!("find_presets in {:?}", profile.id));
    let input: Map<String, Value> = input.as_object().cloned().unwrap();
    let result = tool.execute(input, Signal::new()).await.unwrap();
    (serde_json::from_str(&result.text).unwrap(), Value::Object(tool.input_schema()))
}

fn names(found: &Value) -> Vec<String> {
    found["presets"].as_array().unwrap().iter().map(|row| row["name"].as_str().unwrap().to_owned()).collect()
}

fn row<'a>(found: &'a Value, name: &str) -> &'a Value {
    found["presets"].as_array().unwrap().iter().find(|row| row["name"] == name).unwrap_or_else(|| panic!("{name} in {found}"))
}

#[tokio::test]
async fn reaper_and_renoise_presets_are_learned_by_their_layout_and_offered_only_in_their_daw() {
    let home = Home::new(true);
    learn(LearnOptions::new(home.plan(), Signal::new())).await.unwrap();
    let logs = library_logs(home.dir.path().to_str().unwrap());
    let presets: Vec<PresetEntry> = logs.presets.load().await.into_values().collect();
    let access = Rc::new(Learned { presets });

    // REAPER: its chains, templates, JSFX (not a library without desc) and preset banks; nobody else's.
    let (reaper, schema) = find_presets(&access, &DawProfile::REAPER, json!({"limit": 50})).await;
    let mut found = names(&reaper);
    found.sort();
    assert_eq!(found, ["Drum Bus", "Lead Vox", "MIDI Choke", "Mix Start", "reacomp presets"], "{reaper}");
    assert_eq!(
        schema["properties"]["kind"]["enum"],
        json!(["fx chain", "track template", "project template", "jsfx", "fx presets", "plug-in"])
    );
    let chain = row(&reaper, "Lead Vox");
    assert_eq!(chain["kind"], "fx chain");
    assert_eq!(chain["device"], "ReaEQ (Cockos)");
    assert_eq!(chain["inside"], json!(["ReaEQ (Cockos)", "ReaComp (Cockos)", "JS: utility/volume"]));
    assert!(chain["path"].as_str().unwrap().replace('\\', "/").ends_with("FXChains/Vocals/Lead Vox.RfxChain"));
    let template = row(&reaper, "Drum Bus");
    assert_eq!((template["kind"].as_str(), template["about"].as_str()), (Some("track template"), Some("2 tracks: Kick, Snare Top")));
    assert_eq!(template["inside"], json!(["ReaGate (Cockos)"]));
    assert_eq!(row(&reaper, "Mix Start")["about"], "2 tracks: Bass, Keys");
    let jsfx = row(&reaper, "MIDI Choke");
    assert_eq!((jsfx["kind"].as_str(), jsfx["add"].as_str()), (Some("jsfx"), Some("JS: midi/midi_choke")));
    assert_eq!(jsfx["about"], "MIDI processing (by Cockos)");
    let bank = row(&reaper, "reacomp presets");
    assert_eq!((bank["kind"].as_str(), bank["device"].as_str()), (Some("fx presets"), Some("reacomp")));
    assert_eq!(bank["presets"], json!(["stock - Rock Snare", "stock - Vocal Leveler"]));
    // A bank is found by its presets' names, a chain by what it holds.
    assert_eq!(names(&find_presets(&access, &DawProfile::REAPER, json!({"words": ["leveler"]})).await.0), ["reacomp presets"]);
    assert_eq!(names(&find_presets(&access, &DawProfile::REAPER, json!({"words": ["snare"]})).await.0), ["Drum Bus", "reacomp presets"]);
    assert_eq!(names(&find_presets(&access, &DawProfile::REAPER, json!({"device": "ReaComp"})).await.0), ["reacomp presets", "Lead Vox"]);
    assert_eq!(names(&find_presets(&access, &DawProfile::REAPER, json!({"kind": "jsfx"})).await.0), ["MIDI Choke"]);

    // Renoise: every library folder's kind, the User Library's first; nobody else's.
    let (renoise, schema) = find_presets(&access, &DawProfile::RENOISE, json!({"limit": 50})).await;
    let mut found = names(&renoise);
    found.sort();
    assert_eq!(found, ["Amp Wide", "Drums Hard", "Drums Hard EQ", "Euclid Rhythm", "Old Filters", "Piano", "Tight Kit", "Up"], "{renoise}");
    assert_eq!(
        schema["properties"]["kind"]["enum"],
        json!(["instrument", "multi-sample", "effect chain", "effect preset", "phrase", "phrase script", "modulation set"])
    );
    let kit = row(&renoise, "Tight Kit");
    assert_eq!((kit["kind"].as_str(), kit["about"].as_str()), (Some("instrument"), Some("2 samples: Kick Deep, Snare")));
    let preset = row(&renoise, "Drums Hard");
    assert_eq!((preset["kind"].as_str(), preset["device"].as_str()), (Some("effect preset"), Some("Compressor")));
    assert_eq!(row(&renoise, "Drums Hard EQ")["device"], "EQ 10");
    let chain = row(&renoise, "Amp Wide");
    assert_eq!((chain["kind"].as_str(), &chain["inside"]), (Some("effect chain"), &json!(["*LFO", "Doofer", "EQ 10"])));
    assert_eq!(
        (row(&renoise, "Piano")["kind"].as_str(), row(&renoise, "Piano")["about"].as_str()),
        (Some("multi-sample"), Some("2 zones"))
    );
    assert_eq!(row(&renoise, "Up")["about"], "Arp - Up (16ths)");
    assert_eq!(row(&renoise, "Euclid Rhythm")["about"], "Euclidean rhythms (three pulses in eight)");
    assert_eq!(row(&renoise, "Old Filters")["kind"], "modulation set");
    assert_eq!(
        names(&find_presets(&access, &DawProfile::RENOISE, json!({"words": ["drums", "hard"]})).await.0),
        ["Drums Hard EQ", "Drums Hard"]
    );

    // Live: its own preset only.
    assert_eq!(names(&find_presets(&access, &DawProfile::ABLETON, json!({})).await.0), ["Vox Chain"]);
}

#[tokio::test(flavor = "current_thread")]
async fn find_sounds_in_each_daw_holds_only_the_sounds_offered_there() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = Home::new(true);
            put(&home.home.path().join("Music/Ableton/User Library/Samples/My Kick.wav"), wav(&[fixture::kick(50., 0.2)], 44100));
            learn(LearnOptions::new(home.plan(), Signal::new())).await.unwrap();
            let library = create_library(LibraryOptions {
                daws: kumi::daw::registry::libraries(),
                dir: home.dir.path().to_string_lossy().into(),
                sources: Some(home.options()),
                fork: Some(false),
                delay_ms: Some(3_600_000),
                ..Default::default()
            });
            use kumi_types::daw::DawId::*;
            let names = |index: &SoundIndex| {
                let mut names: Vec<String> =
                    index.entries.iter().map(|e| Path::new(&e.path).file_stem().unwrap().to_string_lossy().into_owned()).collect();
                names.sort();
                names
            };
            assert_eq!(names(&library.sound_index_in(Some(Renoise)).await.unwrap()), ["My Kick", "Renoise Kick"]);
            assert_eq!(names(&library.sound_index_in(Some(Reaper)).await.unwrap()), ["My Kick"]);
            assert_eq!(names(&library.sound_index_in(Some(Ableton)).await.unwrap()), ["My Kick"]);
            assert_eq!(names(&library.sound_index().await.unwrap()), ["My Kick"]);
            library.close().await;
        })
        .await;
}

/// my_sets finds the producer's Live Sets (the library reads only .als): REAPER and Renoise aren't offered it as their own
/// projects or songs.
#[test]
fn my_sets_is_offered_only_where_kumi_reads_the_daws_projects() {
    let access = Rc::new(Learned { presets: vec![] });
    let offered = |profile: &DawProfile| {
        library_tools_for(profile, access.clone(), LibraryToolsOptions::default()).iter().any(|tool| tool.name() == "my_sets")
    };
    assert!(offered(&DawProfile::ABLETON));
    assert!(!offered(&DawProfile::REAPER));
    assert!(!offered(&DawProfile::RENOISE));
}

/// (#361 review K31) A DAW library inside a folder the producer named (or a Live Place): what the library's layout reads
/// is its own source's and offered only in its DAW; the rest there is still the outer folder's, learned and offered
/// everywhere, as before the library was a source of its own.
#[tokio::test(flavor = "current_thread")]
async fn a_daws_library_inside_a_producers_folder_leaves_its_other_sounds_to_that_folder() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = Home::new(true);
            let user = home.home.path().join("Documents/Renoise/User Library");
            put(&user.join("Kits/Kit Snare.wav"), wav(&[fixture::snare(0.2)], 44100));
            put(&user.join("Samples/User Clap.wav"), wav(&[fixture::snare(0.1)], 44100));
            put(&user.join("Loose Hat.wav"), wav(&[fixture::hat(0.1, 3)], 44100));
            let documents = home.home.path().join("Documents").to_string_lossy().into_owned();
            let mut plan = home.plan();
            plan.sources = library_sources(&SourceOptions { folders: Some(vec![documents.clone()]), ..home.options() });
            learn(LearnOptions::new(plan, Signal::new())).await.unwrap();
            let logs = library_logs(home.dir.path().to_str().unwrap());
            let learned: Vec<String> =
                logs.sounds.load().await.into_keys().map(|p| Path::new(&p).file_stem().unwrap().to_string_lossy().into_owned()).collect();
            for name in ["Kit Snare", "Loose Hat", "User Clap", "Renoise Kick"] {
                assert_eq!(learned.iter().filter(|n| *n == name).count(), 1, "{name} learned once: {learned:?}");
            }
            let library = create_library(LibraryOptions {
                daws: kumi::daw::registry::libraries(),
                dir: home.dir.path().to_string_lossy().into(),
                folders: Some(vec![documents]),
                sources: Some(home.options()),
                fork: Some(false),
                delay_ms: Some(3_600_000),
                ..Default::default()
            });
            use kumi_types::daw::DawId::*;
            let names = |index: &SoundIndex| {
                let mut names: Vec<String> =
                    index.entries.iter().map(|e| Path::new(&e.path).file_stem().unwrap().to_string_lossy().into_owned()).collect();
                names.sort();
                names
            };
            assert_eq!(
                names(&library.sound_index_in(Some(Renoise)).await.unwrap()),
                ["Kit Snare", "Loose Hat", "Renoise Kick", "User Clap"]
            );
            assert_eq!(names(&library.sound_index_in(Some(Reaper)).await.unwrap()), ["Kit Snare", "Loose Hat"]);
            assert_eq!(
                names(&library.sound_index_in(None).await.unwrap()),
                ["Kit Snare", "Loose Hat"],
                "Renoise's own samples aren't Live's"
            );
            library.close().await;
        })
        .await;
}

/// Every step find_presets names for loading a DAW's presets is one of that DAW's change steps.
#[test]
fn find_presets_names_only_steps_the_daw_has_and_lives_text_stays() {
    use kumi_daw_reaper::changes as reaper_changes;
    use kumi_daw_renoise::changes as renoise_changes;
    let access = Rc::new(Learned { presets: vec![] });
    let description = |profile: &DawProfile| {
        let tools = library_tools_for(profile, access.clone(), LibraryToolsOptions::default());
        tools.iter().find(|t| t.name() == "find_presets").map(|t| t.description().to_string()).unwrap()
    };
    for (profile, set, wanted) in [
        (&DawProfile::REAPER, &reaper_changes::CHANGES, &["fx_chain_load", "track_template_insert", "fx_add", "fx_preset"][..]),
        (
            &DawProfile::RENOISE,
            &renoise_changes::CHANGES,
            &[
                "instrument_load",
                "device_add",
                "device_preset_data",
                "device_chain_load",
                "phrase_load",
                "modset_load",
                "phrase_add",
                "phrase_script",
            ][..],
        ),
    ] {
        let text = description(profile);
        for step in wanted {
            assert!(text.contains(step), "{:?} names {step}: {text}", profile.id);
            assert!(set.kinds.iter().any(|kind| kind.tool == *step), "{step} is a step of {:?}", profile.id);
        }
    }
    let live = description(&DawProfile::ABLETON);
    assert!(live.contains("load_device takes as itemId") && !live.contains("device_preset_data") && !live.contains("fx_chain_load"));
}

/// A DAW's library text comes from its profile alone (review L5): a profile Kumi's library code has never heard of, with
/// presets in its capabilities and its own sample steps, preset kinds and loading words in its vocabulary, gets
/// find_presets in its words, its kinds offered, and find_sounds naming its sample steps.
#[test]
fn a_dawss_library_words_come_from_its_profile_alone() {
    use kumi_types::daw::{Capabilities, PromptVocab};
    const ANOTHER: DawProfile = DawProfile {
        id: None,
        caps: Capabilities { presets: true, ..Capabilities::NONE },
        vocab: PromptVocab {
            sample_tools: "clip_drop and the other sample steps",
            preset_loading: ". Load one with preset_drop (path)",
            preset_kinds: &["patch", "chain"],
            ..DawProfile::UNKNOWN.vocab
        },
        ..DawProfile::UNKNOWN
    };
    let access = Rc::new(Learned { presets: vec![] });
    let tools = library_tools_for(&ANOTHER, access, LibraryToolsOptions::default());
    let presets = tools.iter().find(|t| t.name() == "find_presets").expect("find_presets: the profile says it has presets");
    assert!(presets.description().contains("Load one with preset_drop (path)"), "{}", presets.description());
    assert_eq!(presets.input_schema()["properties"]["kind"]["enum"], json!(["patch", "chain"]));
    let sounds = tools.iter().find(|t| t.name() == "find_sounds").unwrap();
    let said = format!("{} {}", sounds.description(), Value::Object(sounds.input_schema()));
    assert!(said.contains("clip_drop and the other sample steps"), "{said}");
    // A DAW Kumi doesn't know: no presets, as before.
    let unknown = library_tools_for(&DawProfile::UNKNOWN, Rc::new(Learned { presets: vec![] }), LibraryToolsOptions::default());
    assert!(!unknown.iter().any(|t| t.name() == "find_presets"));
}
