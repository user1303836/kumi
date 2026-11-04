//! /music in the app: the words it reads, and the producer's music shown (a folder found before they add one, how
//! learning goes, and that names reach the model).
use kumi::tui::app::{music_lines, parse_music, MusicAsked};
use kumi_listening::music::{
    folders::Found,
    learning::{MusicState, MusicStatus},
};

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
