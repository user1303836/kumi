//! /music: the producer's own music, which Kumi learns their taste from. Its folders (and, before they add one, the
//! one Kumi found), its songs and how far they're heard, pausing and going on, adding and removing a folder, and
//! forgetting all of it.
use super::*;
use kumi_common::time::since;
use kumi_listening::music::{
    folders::{self, Found},
    learning::{Music, MusicState, MusicStatus, FORGETTING},
    tool::{about, grouped, tilde},
};

/// What /music was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MusicAsked {
    Show,
    Add(String),
    Remove(String),
    Pause,
    Resume,
    Forget,
    Unknown,
}

/// /music's words, read in any case.
pub fn parse_music(words: &str) -> MusicAsked {
    let words = words.trim();
    let (verb, rest) = words.split_once(char::is_whitespace).map_or((words, ""), |(verb, rest)| (verb, rest.trim()));
    match (verb.to_lowercase().as_str(), rest.is_empty()) {
        ("", _) => MusicAsked::Show,
        ("add", false) => MusicAsked::Add(rest.into()),
        ("remove", false) => MusicAsked::Remove(rest.into()),
        ("pause", true) => MusicAsked::Pause,
        ("resume", true) => MusicAsked::Resume,
        ("forget", true) => MusicAsked::Forget,
        _ => MusicAsked::Unknown,
    }
}

/// The line under /music's lines: what it does, and what reaches the model.
pub const MUSIC_FOOTER: &str = "/music add <folder> or /music remove <folder> · /music pause or resume · /music forget deletes what Kumi learned of your music. Song and artist names reach the model when Kumi uses your taste.";

/// /music's lines: the folders (or the one found), the songs, and how learning goes.
pub fn music_lines(status: &MusicStatus, agreed: bool, held: bool, found: Option<&Found>, now: f64) -> Vec<String> {
    let mut lines = vec![];
    if status.folders.is_empty() {
        lines.push("Folders: none".into());
    } else {
        lines.extend(status.folders.iter().map(|folder| format!("Folder: {}", tilde(folder))));
    }
    if let Some(found) = found.filter(|_| status.folders.is_empty()) {
        lines.push(format!(
            "Found: {} ({} {}): /music add {} learns your taste from it",
            tilde(&found.folder),
            about(found.songs, found.capped),
            if found.songs == 1 { "song" } else { "songs" },
            tilde(&found.folder)
        ));
    }
    if status.songs > 0 {
        lines.push(format!(
            "Songs: {} ({} heard lightly, {} in full, {} skipped)",
            grouped(status.songs),
            grouped(status.light),
            grouped(status.full),
            grouped(status.skipped)
        ));
    }
    if status.cloud > 0 {
        lines.push(format!("Only in the cloud: {} (heard once they're on this computer)", grouped(status.cloud)));
    }
    lines.push(match status.state {
        MusicState::Off if !agreed => "Now: off (Kumi asks before it learns your music)".into(),
        MusicState::Off => "Now: off, with no folder to learn from".into(),
        MusicState::Learning => "Now: learning, in the background".into(),
        MusicState::Paused if held => "Now: paused (/music resume goes on)".into(),
        MusicState::Paused => "Now: waiting while Kumi listens or plays".into(),
        MusicState::Elsewhere => "Now: another Kumi is learning it".into(),
        MusicState::Done => match status.learned_at {
            Some(at) => format!("Now: up to date, learned {} (Kumi looks for new songs every half hour)", since(at as f64, now)),
            None => "Now: about to start".into(),
        },
    });
    if let Some(said) = &status.said {
        lines.push(format!("Last word: {said}"));
    }
    lines
}

impl TuiApp {
    /// /music and its words.
    pub(super) async fn music_command(&self, music: Rc<Music>, words: &str) {
        match parse_music(words) {
            MusicAsked::Show => {
                let found = match music.folders().is_empty() {
                    true => {
                        let (places, skip) = (music.places(), music.skipping());
                        tokio::task::spawn_blocking(move || folders::found_in(places, &skip())).await.ok().flatten()
                    }
                    false => None,
                };
                let items = music_lines(&music.status(), music.agreed(), music.held(), found.as_ref(), kumi_common::time::now_ms() as f64);
                self.0.state.borrow_mut().transcript.add(Entry::News {
                    title: "Your music".into(),
                    items,
                    footer: Some(MUSIC_FOOTER.into()),
                });
                self.0.scheduler.request();
            }
            MusicAsked::Add(folder) => {
                let Some(path) = kumi_library::sources::expand_folder(&folder, &kumi_library::sources::homedir())
                    .filter(|path| std::path::Path::new(path).is_dir())
                else {
                    self.notice(&format!("There's no folder {} on this computer.", self.clean(&folder, 200)), NoticeTone::Warn);
                    return;
                };
                let mut wanted = music.folders();
                if wanted.iter().any(|known| {
                    *known == folder
                        || kumi_library::sources::expand_folder(known, &kumi_library::sources::homedir()).as_deref() == Some(&path)
                }) {
                    self.notice(&format!("Kumi learns from {} already.", tilde(&path)), NoticeTone::Info);
                    return;
                }
                wanted.push(folder);
                match music.learn(wanted) {
                    Ok(()) => self.notice(
                        &format!("Kumi learns your music in {} too, in the background (/music shows how far it's got).", tilde(&path)),
                        NoticeTone::Info,
                    ),
                    Err(why) if why == FORGETTING => self.notice(&why, NoticeTone::Warn),
                    Err(why) => self.notice(&format!("Kumi couldn't keep that in its settings: {why}"), NoticeTone::Warn),
                }
            }
            MusicAsked::Remove(folder) => {
                let home = kumi_library::sources::homedir();
                let path = kumi_library::sources::expand_folder(&folder, &home);
                let known = music.folders();
                let kept: Vec<String> = known
                    .iter()
                    .filter(|known| **known != folder && kumi_library::sources::expand_folder(known, &home) != path)
                    .cloned()
                    .collect();
                if kept.len() == known.len() {
                    self.notice(
                        &format!("Kumi doesn't learn from {}: /music shows the folders it does.", self.clean(&folder, 200)),
                        NoticeTone::Warn,
                    );
                    return;
                }
                match music.set_folders(Some(kept)) {
                    Ok(()) => {
                        self.notice(
                            &format!("Kumi no longer learns from {}; what it heard there is set aside as gone.", self.clean(&folder, 200)),
                            NoticeTone::Info,
                        );
                        self.music_changed_under_map_page();
                    }
                    Err(why) => self.notice(&format!("Kumi couldn't keep that in its settings: {why}"), NoticeTone::Warn),
                }
            }
            MusicAsked::Pause => {
                music.hold(true);
                self.notice("Learning your music is paused: /music resume goes on.", NoticeTone::Info);
            }
            MusicAsked::Resume => {
                music.hold(false);
                self.notice("Learning your music again, in the background.", NoticeTone::Info);
            }
            MusicAsked::Forget => {
                let items = vec![
                    PickerItem {
                        detail: Some("its songs, their style vectors and the map".into()),
                        ..PickerItem::new("Forget my music", "forget")
                    },
                    PickerItem::new("Keep it", "keep"),
                ];
                let title = "Forget what Kumi learned of your music? It stops learning until you add a folder again.";
                self.pick(Picker::new(title, items), move |app, item| {
                    let music = music.clone();
                    async move {
                        app.close_panel();
                        if item.value.as_deref() == Some("forget") {
                            match music.forget().await {
                                Ok(()) => app.notice(
                                    "Forgot your music. Kumi learns none of it until you add a folder (/music add).",
                                    NoticeTone::Info,
                                ),
                                Err(why) => app.notice(&why, NoticeTone::Warn),
                            }
                            app.music_changed_under_map_page();
                        }
                        Ok(())
                    }
                });
            }
            MusicAsked::Unknown => {
                self.notice("/music shows your music; /music add <folder>, remove <folder>, pause, resume or forget.", NoticeTone::Warn);
            }
        }
    }
}
