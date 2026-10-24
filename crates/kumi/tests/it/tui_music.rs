//! /music in the app: the producer's music shown (a folder found before they add one, how learning goes, and that
//! names reach the model), a folder added, learning paused, and all of it forgotten after a confirm.
#[path = "../support/tui_app.rs"]
mod support;
use async_trait::async_trait;
use kumi::tui::app::{music_lines, parse_music, MusicAsked};
use kumi_common::abort::Signal;
use kumi_listening::{
    embed::Say,
    music::{
        folders::Found,
        hear::Style,
        learning::{create_music, MusicOptions, MusicState, MusicStatus},
        vectors::DIMS,
    },
};
use std::{path::Path, rc::Rc};
use support::*;

/// A style model of the test's own: the scan never reaches for Kumi's, which it would fetch (the runtime, its words
/// model) over the network.
struct Plain;

#[async_trait(?Send)]
impl Style for Plain {
    fn model(&self) -> &str {
        "test-model"
    }
    async fn embed(&self, _: &Path, _: &[f64], _: Option<f64>, _: Say<'_>, _: &Signal) -> Result<(Vec<f32>, String), String> {
        Ok((vec![1. / (DIMS as f32).sqrt(); DIMS], "test-model".into()))
    }
}

#[test]
fn music_words_are_read_in_any_case() {
    assert_eq!(parse_music(""), MusicAsked::Show);
    assert_eq!(parse_music(" ADD ~/Music "), MusicAsked::Add("~/Music".into()));
    assert_eq!(parse_music("remove /Volumes/Drive/Music"), MusicAsked::Remove("/Volumes/Drive/Music".into()));
    assert_eq!(
        (parse_music("pause"), parse_music("Resume"), parse_music("forget")),
        (MusicAsked::Pause, MusicAsked::Resume, MusicAsked::Forget)
    );
    assert_eq!(parse_music("add"), MusicAsked::Unknown);
    assert_eq!(parse_music("forget everything"), MusicAsked::Unknown);
}

#[test]
fn music_lines_say_the_folders_the_songs_and_how_learning_goes() {
    let status = |state: MusicState| MusicStatus {
        state,
        folders: vec!["/Users/someone/Music".into()],
        songs: 2_400,
        light: 1_200,
        full: 300,
        skipped: 12,
        cloud: 0,
        learned_at: Some(1_000),
        said: None,
    };
    let lines = music_lines(&status(MusicState::Learning), true, false, None, 1_000.);
    assert_eq!(
        lines[1..],
        ["Songs: 2,400 (1,200 heard lightly, 300 in full, 12 skipped)".to_string(), "Now: learning, in the background".into()]
    );
    assert_eq!(music_lines(&status(MusicState::Paused), true, true, None, 1_000.).last().unwrap(), "Now: paused (/music resume goes on)");
    assert_eq!(
        music_lines(&status(MusicState::Paused), true, false, None, 1_000.).last().unwrap(),
        "Now: waiting while Kumi listens or plays"
    );
    assert_eq!(
        music_lines(&status(MusicState::Done), true, false, None, 3_601_000.).last().unwrap(),
        "Now: up to date, learned 1 hour ago (Kumi looks for new songs every half hour)"
    );
    let off = MusicStatus {
        state: MusicState::Off,
        folders: vec![],
        songs: 0,
        light: 0,
        full: 0,
        skipped: 0,
        cloud: 0,
        learned_at: None,
        said: None,
    };
    let found = Found { folder: "/somewhere/Music".into(), songs: 2_391, ..Default::default() };
    assert_eq!(
        music_lines(&off, false, false, Some(&found), 0.),
        [
            "Folders: none",
            "Found: /somewhere/Music (about 2,400 songs): /music add /somewhere/Music learns your taste from it",
            "Now: off (Kumi asks before it learns your music)"
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn music_is_shown_added_paused_and_forgotten() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let songs = folder.path().join("Songs");
            std::fs::create_dir_all(songs.join("Artist/Album")).unwrap();
            std::fs::write(songs.join("Artist/Album/01 One.mp3"), b"tagless").unwrap();
            let kept = folder.path().join("kumi/music");
            let mut options = MusicOptions::new(kept.clone(), None, Default::default());
            (options.places, options.fork, options.delay_ms) = (Some(vec![songs.clone()]), false, 3_600_000);
            options.style = Some(Rc::new(Plain) as Rc<dyn Style>);
            let music = create_music(options);
            let h = Harness::with(160, 40, Rc::new(Control::default()), |options| options.music = Some(music.clone()));
            h.start().await;
            h.connect();
            h.type_text("/mus").await;
            h.has("Your own music, which Kumi learns your taste from");
            h.type_text("\x15/music\r").await;
            h.has("Your music");
            h.has("(1 song): /music add");
            h.has("Now: off (Kumi asks before it learns your music)");
            h.has("Song and artist names reach the model when Kumi uses your taste.");
            let path = songs.to_string_lossy().into_owned();
            h.type_text(&format!("/music add {path}\r")).await;
            h.has("Kumi learns your music in");
            assert_eq!(music.folders(), [path]);
            assert!(music.agreed() && music.offered());
            h.type_text("/music pause\r").await;
            h.has("Learning your music is paused: /music resume goes on.");
            assert!(music.held());
            h.type_text("/music forget\r").await;
            h.has("Forget what Kumi learned of your music?");
            h.type_text("\r").await;
            for _ in 0..200 {
                if !music.agreed() && !kept.exists() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            h.has("Forgot your music.");
            assert!(!music.agreed() && !kept.exists());
            h.close().await;
        })
        .await;
}
