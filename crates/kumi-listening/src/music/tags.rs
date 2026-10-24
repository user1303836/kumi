//! A song's names: its tags (ID3, Vorbis comments, MP4's atoms, RIFF INFO), read with lofty, or, where it has none, its
//! path under its music folder (`Artist/Album/01 Title.ext`).

use lofty::{
    config::{ParseOptions, ParsingMode},
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{Accessor, ItemKey, Tag},
};
use std::path::Path;

/// Where a song's names came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NamesFrom {
    Tags,
    #[default]
    Path,
}

impl NamesFrom {
    pub fn as_str(self) -> &'static str {
        match self {
            NamesFrom::Tags => "tags",
            NamesFrom::Path => "path",
        }
    }
    pub fn parse(text: &str) -> NamesFrom {
        if text == "tags" {
            NamesFrom::Tags
        } else {
            NamesFrom::Path
        }
    }
}

/// What Kumi knows of a song before it's heard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub track: Option<i64>,
    /// Its length in seconds, when the file says.
    pub seconds: Option<f64>,
    pub names_from: NamesFrom,
}

/// The longest name kept: a tag can hold a whole liner note.
const LONGEST: usize = 200;

/// A song's tags, its path's names filling what they leave out; a song with no artist or title in its tags (or that
/// lofty can't read) is named by its path alone.
pub fn read(file: &Path, folder: &Path) -> Tags {
    let path = from_path(file, folder);
    let Some(tags) = read_tags(file) else { return path };
    if tags.artist.is_none() && tags.title.is_none() {
        return Tags { genre: tags.genre, year: tags.year, seconds: tags.seconds, track: tags.track.or(path.track), ..path };
    }
    Tags {
        artist: tags.artist.or(tags.album_artist.clone()).or(path.artist),
        album: tags.album.or(path.album),
        title: tags.title.or(path.title),
        track: tags.track.or(path.track),
        ..tags
    }
}

/// The tags alone, None when the file can't be read as one lofty knows.
fn read_tags(file: &Path) -> Option<Tags> {
    let options = ParseOptions::new().read_cover_art(false).parsing_mode(ParsingMode::Relaxed);
    let tagged = Probe::open(file).ok()?.options(options).guess_file_type().ok()?.read().ok()?;
    let seconds = Some(tagged.properties().duration().as_secs_f64()).filter(|seconds| *seconds > 0.);
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Some(Tags { seconds, names_from: NamesFrom::Tags, ..Default::default() });
    };
    Some(Tags {
        artist: tag.artist().and_then(|text| clean(&text)),
        album_artist: tag.get_string(ItemKey::AlbumArtist).and_then(clean),
        album: tag.album().and_then(|text| clean(&text)),
        title: tag.title().and_then(|text| clean(&text)),
        genre: tag.genre().and_then(|text| genre(&text)),
        year: year(tag),
        track: tag.track().filter(|track| *track > 0).map(i64::from),
        seconds,
        names_from: NamesFrom::Tags,
    })
}

/// The year it came out: its date's year, or the first four digits of a year written as text.
fn year(tag: &Tag) -> Option<i64> {
    let valid = |year: i64| (1000..=9999).contains(&year).then_some(year);
    if let Some(year) = tag.date().and_then(|date| valid(date.year as i64)) {
        return Some(year);
    }
    [ItemKey::Year, ItemKey::RecordingDate, ItemKey::ReleaseDate, ItemKey::OriginalReleaseDate]
        .into_iter()
        .filter_map(|key| tag.get_string(key))
        .find_map(|text| text.trim().get(..4).and_then(|digits| digits.parse().ok()).and_then(valid))
}

/// A genre as words: ID3's numbered genres (`(17)`, `17`, `(17)Rock`) by their names, several joined with "; ".
fn genre(text: &str) -> Option<String> {
    let named = |part: &str| -> Option<String> {
        let part = part.trim();
        let (number, rest) = match part.strip_prefix('(').and_then(|rest| rest.split_once(')')) {
            Some((number, rest)) => (number, rest),
            None => (part, ""),
        };
        match number.parse::<usize>().ok().and_then(|n| lofty::id3::v1::GENRES.get(n)) {
            Some(name) if rest.trim().is_empty() => Some(name.to_string()),
            Some(_) => clean(rest),
            None => clean(part),
        }
    };
    let parts: Vec<String> = text.split(['\0', ';']).filter_map(named).collect();
    let mut kept: Vec<String> = vec![];
    for part in parts {
        if !kept.iter().any(|known| known.eq_ignore_ascii_case(&part)) {
            kept.push(part);
        }
    }
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// A name as kept: no control characters, single spaces, at most `LONGEST` characters; None when nothing's left.
pub fn clean(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|word| !word.is_empty()).collect();
    let joined = words.join(" ");
    let kept: String = joined.chars().take(LONGEST).collect();
    (!kept.is_empty()).then_some(kept)
}

/// Folder names that name no artist or album: iTunes' and Music's own.
const NAMELESS: [&str; 4] = ["unknown artist", "unknown album", "compilations", "various artists"];

/// A song's names from its path under its folder: `Artist/Album/01 Title.ext`, `Artist/Title.ext`,
/// `Album/Artist - Title.ext` or `Artist - Title.ext`.
pub fn from_path(file: &Path, folder: &Path) -> Tags {
    let relative = file.strip_prefix(folder).unwrap_or(file);
    let parts: Vec<&str> = relative.components().filter_map(|part| part.as_os_str().to_str()).collect();
    let folders: Vec<&str> = parts.iter().take(parts.len().saturating_sub(1)).copied().collect();
    let named = |name: &str| clean(name).filter(|name| !NAMELESS.contains(&name.to_lowercase().as_str()));
    let stem = file.file_stem().and_then(|stem| stem.to_str()).unwrap_or("");
    let (track, rest) = numbered(stem);
    let (mut artist, mut album) = match folders[..] {
        [] => (None, None),
        [only] => (named(only), None),
        [.., artist, album] => (named(artist), named(album)),
    };
    let mut title = clean(rest);
    // "Artist - Title" in the file's own name: then a lone folder is its album.
    if let Some((by, name)) = rest.split_once(" - ").and_then(|(by, name)| Some((clean(by)?, clean(name)?))) {
        if folders.len() < 2 {
            album = if folders.len() == 1 { artist.take() } else { None };
            artist = Some(by);
            title = Some(name);
        }
    }
    Tags { artist, album, title: title.or_else(|| clean(stem)), track, names_from: NamesFrom::Path, ..Default::default() }
}

/// A file name's track number and the rest: `01 Title`, `01 - Title`, `01. Title`, `1-01 Title` (disc and track).
fn numbered(stem: &str) -> (Option<i64>, &str) {
    let digits = |text: &str| text.chars().take_while(char::is_ascii_digit).count();
    let mut at = digits(stem);
    if at == 0 || at > 3 {
        return (None, stem);
    }
    let mut number = &stem[..at];
    // A disc's number before the track's.
    if stem[at..].starts_with('-') {
        let more = digits(&stem[at + 1..]);
        if (1..=3).contains(&more) {
            number = &stem[at + 1..at + 1 + more];
            at += 1 + more;
        }
    }
    let rest = stem[at..].trim_start_matches(['.', ' ', '-', '_', ')']).trim_start();
    if rest.is_empty() || at == stem.len() || !stem[at..].starts_with(['.', ' ', '-', '_', ')']) {
        return (None, stem);
    }
    (number.parse().ok(), rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn named(path: &str) -> (Option<String>, Option<String>, Option<String>, Option<i64>) {
        let tags = from_path(&PathBuf::from("/m").join(path), Path::new("/m"));
        (tags.artist, tags.album, tags.title, tags.track)
    }

    fn some(text: &str) -> Option<String> {
        Some(text.into())
    }

    #[test]
    fn paths_name_artist_album_and_title() {
        assert_eq!(named("Burial/Untrue/02 Archangel.mp3"), (some("Burial"), some("Untrue"), some("Archangel"), Some(2)));
        assert_eq!(named("Burial/Untrue/1-02 Archangel.flac"), (some("Burial"), some("Untrue"), some("Archangel"), Some(2)));
        assert_eq!(named("Burial/Archangel.mp3"), (some("Burial"), None, some("Archangel"), None));
        assert_eq!(named("Untrue/Burial - Archangel.mp3"), (some("Burial"), some("Untrue"), some("Archangel"), None));
        assert_eq!(named("Burial - Archangel.mp3"), (some("Burial"), None, some("Archangel"), None));
        assert_eq!(named("Unknown Artist/Unknown Album/03. Track.wav"), (None, None, some("Track"), Some(3)));
        // A number that's the name, not a track.
        assert_eq!(named("Aphex Twin/Selected/4.mp3"), (some("Aphex Twin"), some("Selected"), some("4"), None));
        assert_eq!(named("Artist/Album/2000 Light Years.mp3"), (some("Artist"), some("Album"), some("2000 Light Years"), None));
    }

    #[test]
    fn genres_read_as_words() {
        assert_eq!(genre("(17)").as_deref(), Some("Rock"));
        assert_eq!(genre("17").as_deref(), Some("Rock"));
        assert_eq!(genre("(52)Electronic").as_deref(), Some("Electronic"));
        assert_eq!(genre("Dubstep;dubstep; Ambient").as_deref(), Some("Dubstep; Ambient"));
        assert_eq!(genre("  ").as_deref(), None);
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean(" Burial\u{0}  Untrue \n").as_deref(), Some("Burial Untrue"));
        assert_eq!(clean("\u{7}"), None);
        assert_eq!(clean(&"x".repeat(500)).map(|name| name.len()), Some(LONGEST));
    }
}
