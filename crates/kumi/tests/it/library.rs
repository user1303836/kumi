use kumi::{
    library::{progress_line, run_library, LibraryIo},
    tui::tty::TtyOutput,
};
use kumi_common::{abort::Signal, js::number::round};
use kumi_runtime::{
    library::{
        create_library,
        learn::{Counts, LearnPhase, LearnProgress},
        sources::SourceOptions,
        LibraryOptions,
    },
    system::Env,
    KUMI,
};
use std::{cell::RefCell, fs, io::Write, path::Path, rc::Rc};
#[derive(Default)]
struct Out(RefCell<String>);
impl TtyOutput for Out {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        None
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, s: &str) {
        self.0.borrow_mut().push_str(s);
    }
}
fn put(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn tone(hz: f64, seconds: f64) -> Vec<u8> {
    let frames = round(44100. * seconds) as usize;
    let len = (frames * 2) as u32;
    let mut bytes = vec![];
    bytes.extend(b"RIFF");
    bytes.extend((36 + len).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(44100u32.to_le_bytes());
    bytes.extend(88200u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(len.to_le_bytes());
    for i in 0..frames {
        bytes.extend(
            (round(20000. * (2. * std::f64::consts::PI * hz * i as f64 / 44100.).sin() * (-(i as f64) / 44100. / 0.1).exp()) as i16)
                .to_le_bytes(),
        );
    }
    bytes
}
#[tokio::test]
async fn library_command_original_source_scenario_inspects_and_rebuilds_real_files() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let crate_dir = home.path().join("Crate");
            put(crate_dir.join("Kick Round.wav"), tone(55., 0.4));
            put(crate_dir.join("Snare Crack.wav"), tone(200., 0.2));
            let mut gzip = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
            gzip.write_all(br#"<?xml version="1.0"?><Ableton><Operator Id="0"><Annotation Value="" /></Operator></Ableton>"#).unwrap();
            put(crate_dir.join("Presets/Bass.adv"), gzip.finish().unwrap());
            let env: Env = [
                ("KUMI_LIBRARY_DIR".into(), home.path().join("library").display().to_string()),
                ("KUMI_SETTINGS_FILE".into(), home.path().join("settings.json").display().to_string()),
                ("KUMI_PROJECTS_DIR".into(), home.path().join("projects").display().to_string()),
            ]
            .into();
            let library = create_library(LibraryOptions {
                dir: env["KUMI_LIBRARY_DIR"].clone(),
                folders: Some(vec![crate_dir.display().to_string()]),
                sources: Some(SourceOptions {
                    home: Some(home.path().display().to_string()),
                    platform: Some("darwin".into()),
                    applications: Some(home.path().join("Applications").display().to_string()),
                    ..Default::default()
                }),
                fork: Some(false),
                workers: Some(0),
                find_sets: Some(false),
                ..Default::default()
            });
            let out = Rc::new(Out::default());
            let mut io = LibraryIo::new(out.clone(), env.clone());
            io.library = Some(library.clone());
            assert_eq!(run_library(io.clone()).await.unwrap(), 0);
            assert!(out.0.borrow().contains("Not learned yet. Kumi learns it by itself in the background while it runs,"));
            assert!(out.0.borrow().contains("Where Kumi looks\n  Crate"));
            out.0.borrow_mut().clear();
            io.rebuild = true;
            assert_eq!(run_library(io.clone()).await.unwrap(), 0);
            let text = out.0.borrow().clone();
            assert!(text.starts_with("Learning your library again, from the start."));
            assert!(regex::Regex::new(r"Learned 2 sounds, 1 preset and 0 Sets in \d+ seconds\.").unwrap().is_match(&text), "{text}");
            assert!(text.contains("Sounds    2\n  Presets   1\n  Sets      0\n  Learned   just now"), "{text}");
            assert!(text.contains(&format!("{} library --rebuild learns everything again.", *KUMI)));
            // Same operational contract when another Kumi owns the lock, and when interrupted.
            #[cfg(unix)]
            {
                let file = Path::new(&env["KUMI_LIBRARY_DIR"]).join("learning.lock");
                put(&file, serde_json::json!({"pid":unsafe{libc::getppid()},"at":kumi_common::time::now_ms()}).to_string());
                out.0.borrow_mut().clear();
                assert_eq!(run_library(io.clone()).await.unwrap(), 1);
                assert!(out.0.borrow().contains("another window right now"));
                fs::remove_file(file).unwrap();
            }
            let signal = Signal::new();
            signal.cancel();
            io.signal = Some(signal);
            out.0.borrow_mut().clear();
            assert_eq!(run_library(io).await.unwrap(), 1);
            assert!(out.0.borrow().contains("Stopped. What Kumi learned so far is kept."));
            library.close().await;
        })
        .await;
}
#[test]
fn progress_wording_grouped_counts_and_all_phases() {
    let mut p = LearnProgress {
        phase: LearnPhase::Looking,
        at: Some("Crate".into()),
        sounds: Counts { known: 0, done: 1204, todo: 8311 },
        presets: Counts { known: 0, done: 2, todo: 3 },
        sets: Counts { known: 0, done: 1, todo: 4 },
        ..Default::default()
    };
    assert_eq!(progress_line(&p), "Looking through your folders (Crate)…");
    for (phase, want) in [
        (LearnPhase::Presets, "Presets: 2 of 3"),
        (LearnPhase::Sets, "Sets: 1 of 4"),
        (LearnPhase::Sounds, "Sounds: 1,204 of 8,311 (Crate)"),
        (LearnPhase::Tidying, "Tidying up…"),
        (LearnPhase::Done, "Done."),
    ] {
        p.phase = phase;
        assert_eq!(progress_line(&p), want);
    }
}
