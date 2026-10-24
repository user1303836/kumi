//! /map's data: the producer's sounds and songs as points on two maps, each point with its group (a sound's class, a
//! song's style), its name and folder, what of it plays, and what each action asks Kumi. Read afresh each time the map
//! opens; the terminal's map draws it.

use futures::{future::LocalBoxFuture, FutureExt};
use kumi_library::{
    map::{read_sounds_map, sounds_on_map_by_turns, MapSound},
    Library, SoundClass, CLASSES,
};
use kumi_listening::music::{
    hear::heard_first,
    learning::Music,
    store::{MapSong, MapStyle, MusicError, MusicStore},
    tool::tilde,
};
use std::{collections::HashMap, path::PathBuf, rc::Rc};

/// The two maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sounds,
    Songs,
}

/// Both maps, as read when /map opened.
#[derive(Clone, Debug)]
pub struct MapData {
    pub sounds: Layer,
    pub songs: Layer,
}

/// One map: its points, their groups, and the folders and tags the points name by number.
#[derive(Clone, Debug)]
pub struct Layer {
    pub kind: Kind,
    pub points: Vec<Point>,
    pub groups: Vec<Group>,
    pub places: Vec<String>,
    pub tags: Vec<String>,
    /// Why there's nothing on it yet, said to the producer.
    pub empty: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// What a page asks for it by: a sound's place in its layer, a song's id in music.db.
    pub id: i64,
    pub x: f32,
    pub y: f32,
    pub group: u16,
    pub name: String,
    /// Its folder, by its number in the layer's places.
    pub place: u32,
    /// What else the filter reads (a sound's class and kind, a song's genre tags), by its number in the layer's tags.
    pub tags: u32,
    pub path: String,
    /// What of it plays: from where, and for how long (seconds).
    pub from: f32,
    pub seconds: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// What the legend and the filter call it: a class ("kick"), or a style ("c1 · dark rainy").
    pub name: String,
    pub color: [u8; 3],
    pub count: usize,
    /// Points that stand for it (a style's own examples, else those nearest its middle), by their place in the layer.
    pub examples: Vec<u32>,
}

/// How many points stand for a group.
const EXAMPLES: usize = 3;
/// The longest stretch of a sound played, in seconds; and what plays of one whose length isn't known.
const LONGEST_SOUND: f64 = 30.;
const UNKNOWN_SOUND: f64 = 10.;

impl Layer {
    pub fn empty(kind: Kind, why: &str) -> Layer {
        Layer { kind, points: vec![], groups: vec![], places: vec![], tags: vec![], empty: Some(why.into()) }
    }
}

/// Each sound class's colour: drums warm, tones cool, voices and noises apart; a sound with no class grey.
pub fn class_color(class: Option<SoundClass>) -> [u8; 3] {
    match class {
        Some(SoundClass::Kick) => [0xff, 0x6b, 0x6b],
        Some(SoundClass::Snare) => [0xff, 0x9f, 0x43],
        Some(SoundClass::Clap) => [0xff, 0xd1, 0x66],
        Some(SoundClass::Hat) => [0xc5, 0xe4, 0x78],
        Some(SoundClass::Cymbal) => [0xff, 0xe3, 0xb3],
        Some(SoundClass::Tom) => [0xd9, 0x7b, 0x4f],
        Some(SoundClass::Perc) => [0xff, 0x8f, 0xb1],
        Some(SoundClass::Drums) => [0xe0, 0x5a, 0x47],
        Some(SoundClass::Bass) => [0xb3, 0x7b, 0xff],
        Some(SoundClass::Lead) => [0x5c, 0xb8, 0xff],
        Some(SoundClass::Pad) => [0x7a, 0xe0, 0xe0],
        Some(SoundClass::Keys) => [0x6e, 0xe6, 0xb0],
        Some(SoundClass::Pluck) => [0x3d, 0xd6, 0xc6],
        Some(SoundClass::Stab) => [0x8f, 0xa8, 0xff],
        Some(SoundClass::Synth) => [0xa5, 0x94, 0xf9],
        Some(SoundClass::Guitar) => [0xa8, 0xd8, 0x6a],
        Some(SoundClass::Strings) => [0x56, 0xc2, 0x7a],
        Some(SoundClass::Brass) => [0xf2, 0xb8, 0x4b],
        Some(SoundClass::Vocal) => [0xf0, 0x6e, 0xc8],
        Some(SoundClass::Fx) => [0xd0, 0xd6, 0xdf],
        Some(SoundClass::Noise) => [0x9a, 0xa3, 0xad],
        Some(SoundClass::Texture) => [0x8f, 0xb3, 0xc9],
        None => OTHER,
    }
}

/// What has no class or style.
const OTHER: [u8; 3] = [0x6b, 0x72, 0x7d];

/// A style's colour by its number: twelve hues, then the same lighter, then darker.
pub fn style_color(style: usize) -> [u8; 3] {
    const HUES: [[u8; 3]; 12] = [
        [0x5c, 0xb8, 0xff],
        [0xff, 0x9f, 0x43],
        [0x6e, 0xe6, 0xb0],
        [0xff, 0x6b, 0x9b],
        [0xc5, 0xe4, 0x78],
        [0xb3, 0x7b, 0xff],
        [0xff, 0xd1, 0x66],
        [0x3d, 0xd6, 0xc6],
        [0xf0, 0x6e, 0xc8],
        [0x8f, 0xa8, 0xff],
        [0xe0, 0x8a, 0x5a],
        [0x9a, 0xd0, 0xe8],
    ];
    let (toward, share) = match style / HUES.len() % 3 {
        0 => (0., 0.),
        1 => (255., 0.35),
        _ => (0., 0.3),
    };
    HUES[style % HUES.len()].map(|c| (c as f64 + (toward - c as f64) * share).round() as u8)
}

/// Names and folders by number: each said once, however many points share it.
#[derive(Default)]
struct Kept {
    numbers: HashMap<String, u32>,
    all: Vec<String>,
}

impl Kept {
    fn number(&mut self, text: &str) -> u32 {
        if let Some(number) = self.numbers.get(text) {
            return *number;
        }
        let number = self.all.len() as u32;
        self.numbers.insert(text.to_string(), number);
        self.all.push(text.to_string());
        number
    }
}

/// How many points each group has.
fn counts(points: &[Point], groups: usize) -> Vec<usize> {
    let mut counts = vec![0; groups];
    for point in points {
        counts[point.group as usize] += 1;
    }
    counts
}

/// The points of each group nearest its middle.
fn middles(points: &[Point], groups: usize) -> Vec<Vec<u32>> {
    let mut sums = vec![(0f64, 0f64, 0usize); groups];
    for point in points {
        let sum = &mut sums[point.group as usize];
        *sum = (sum.0 + point.x as f64, sum.1 + point.y as f64, sum.2 + 1);
    }
    let mut nearest: Vec<Vec<(f64, u32)>> = vec![vec![]; groups];
    for (at, point) in points.iter().enumerate() {
        let (x, y, count) = sums[point.group as usize];
        let (x, y) = (x / count as f64, y / count as f64);
        let distance = (point.x as f64 - x).powi(2) + (point.y as f64 - y).powi(2);
        let kept = &mut nearest[point.group as usize];
        if kept.len() < EXAMPLES || distance < kept[EXAMPLES - 1].0 {
            kept.push((distance, at as u32));
            kept.sort_by(|a, b| a.0.total_cmp(&b.0));
            kept.truncate(EXAMPLES);
        }
    }
    nearest.into_iter().map(|kept| kept.into_iter().map(|(_, at)| at).collect()).collect()
}

/// The sounds' map: grouped by class (in the classes' order, then those with none).
pub fn sounds_layer(sounds: Vec<MapSound>) -> Layer {
    let (mut places, mut tags) = (Kept::default(), Kept::default());
    let group_of = |class: Option<SoundClass>| class.and_then(|class| CLASSES.iter().position(|c| *c == class)).unwrap_or(CLASSES.len());
    let points: Vec<Point> = sounds
        .into_iter()
        .enumerate()
        .map(|(at, sound)| {
            let words = [sound.class.map(SoundClass::as_str), sound.kind.map(|kind| kind.as_str())];
            Point {
                id: at as i64,
                x: sound.x as f32,
                y: sound.y as f32,
                group: group_of(sound.class) as u16,
                place: places.number(&sound.place),
                tags: tags.number(&words.into_iter().flatten().collect::<Vec<_>>().join(" ")),
                from: 0.,
                seconds: sound.seconds.unwrap_or(UNKNOWN_SOUND).clamp(0.05, LONGEST_SOUND) as f32,
                name: sound.name,
                path: sound.path,
            }
        })
        .collect();
    let (examples, counts) = (middles(&points, CLASSES.len() + 1), counts(&points, CLASSES.len() + 1));
    let groups = CLASSES
        .iter()
        .map(|class| (class.as_str(), Some(*class)))
        .chain([("other", None)])
        .zip(examples.into_iter().zip(counts))
        .map(|((name, class), (examples, count))| Group { name: name.into(), color: class_color(class), count, examples })
        .collect();
    let empty = points.is_empty().then(|| "None of the sounds on Kumi's map are in this library now.".to_string());
    Layer { kind: Kind::Sounds, points, groups, places: places.all, tags: tags.all, empty }
}

/// The songs' map: grouped by style (c1 first, then the songs in none), each style named by its words where the
/// text half gave it some, else by its commonest genres.
pub fn songs_layer(songs: Vec<MapSong>, styles: Vec<MapStyle>) -> Layer {
    let (mut places, mut tags) = (Kept::default(), Kept::default());
    let other = styles.len();
    let points: Vec<Point> = songs
        .into_iter()
        .map(|song| {
            let folder = song.path.rfind(['/', '\\']).map_or("", |end| &song.path[..end]);
            let (from, seconds) = heard_first(song.seconds.unwrap_or(60.));
            Point {
                id: song.id,
                x: song.x as f32,
                y: song.y as f32,
                group: song.cluster.filter(|cluster| *cluster < other).unwrap_or(other) as u16,
                place: places.number(&tilde(folder)),
                tags: tags.number(song.genre.as_deref().unwrap_or("")),
                from: from as f32,
                seconds: seconds as f32,
                name: song.name,
                path: song.path,
            }
        })
        .collect();
    let at: HashMap<i64, u32> = points.iter().enumerate().map(|(at, point)| (point.id, at as u32)).collect();
    let (middle, counts) = (middles(&points, other + 1), counts(&points, other + 1));
    let mut groups: Vec<Group> = styles
        .iter()
        .enumerate()
        .map(|(number, style)| {
            let words = if style.names.is_empty() { style.genres.join(", ") } else { style.names.join(" ") };
            let examples: Vec<u32> = style.examples.iter().filter_map(|id| at.get(id).copied()).take(EXAMPLES).collect();
            Group {
                name: if words.is_empty() { format!("c{}", number + 1) } else { format!("c{} · {words}", number + 1) },
                color: style_color(number),
                count: counts[number],
                examples: if examples.is_empty() { middle[number].clone() } else { examples },
            }
        })
        .collect();
    groups.push(Group { name: "other".into(), color: OTHER, count: counts[other], examples: middle[other].clone() });
    let empty = points.is_empty().then(|| {
        "None of your songs are on a map yet: Kumi lays them out once it has heard them (/music says how far it's got).".to_string()
    });
    Layer { kind: Kind::Songs, points, groups, places: places.all, tags: tags.all, empty }
}

/// What the map offers to ask Kumi about a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    More,
    Reference,
    Grow,
}

pub const ACTIONS: [(Action, &str); 3] = [
    (Action::More, "Find more like this"),
    (Action::Reference, "Use it as the reference"),
    (Action::Grow, "Grow it: a match run aimed at it"),
];

/// What an action asks Kumi, in the producer's words: put in the input box for them to send.
pub fn asked(kind: Kind, point: &Point, action: Action) -> String {
    let (path, name) = (tilde(&point.path), &point.name);
    match (kind, action) {
        (Kind::Sounds, Action::More) => format!("Find sounds like this one: {path}"),
        (Kind::Sounds, Action::Reference) => format!("Use this sound as the reference: {path}"),
        (Kind::Sounds, Action::Grow) => format!("Make a sound like this one, with a match run aimed at it: {path}"),
        (Kind::Songs, Action::More) => format!("Find songs in my music like “{name}”: {path}"),
        (Kind::Songs, Action::Reference) => format!("Use “{name}” as the reference: {path}"),
        (Kind::Songs, Action::Grow) => format!("Make a sound like “{name}”, with a match run aimed at it: {path}"),
    }
}

/// What the browser's page calls an action.
fn action_called(action: Action) -> &'static str {
    match action {
        Action::More => "more",
        Action::Reference => "reference",
        Action::Grow => "grow",
    }
}

/// The map as `/map open` serves it to the browser: read once as it opens, so the page's ids name the same points
/// until it closes. A sound's id is `sounds:<its place on the map>`, a song's `songs:<its id in music.db>`.
pub struct Page {
    data: MapData,
    songs: HashMap<i64, usize>,
}

#[derive(serde::Serialize)]
struct PageMap<'a> {
    layers: [PageLayer<'a>; 2],
    actions: Vec<PageAction>,
}

#[derive(serde::Serialize)]
struct PageLayer<'a> {
    kind: &'static str,
    title: &'static str,
    empty: Option<&'a str>,
    groups: Vec<PageGroup<'a>>,
    places: &'a [String],
    tags: &'a [String],
    points: PagePoints<'a>,
}

#[derive(serde::Serialize)]
struct PageGroup<'a> {
    name: &'a str,
    color: String,
    count: usize,
}

/// The points column by column: 300,000 of them stay a few megabytes.
#[derive(serde::Serialize)]
struct PagePoints<'a> {
    id: Vec<i64>,
    x: Vec<f64>,
    y: Vec<f64>,
    group: Vec<u16>,
    name: Vec<&'a str>,
    place: Vec<u32>,
    tags: Vec<u32>,
}

#[derive(serde::Serialize)]
struct PageAction {
    action: &'static str,
    label: &'static str,
}

fn page_layer<'a>(layer: &'a Layer, title: &'static str) -> PageLayer<'a> {
    let points = &layer.points;
    // Five decimals, as the layout keeps a sound's place: far finer than the room between points.
    let short = |value: f32| (value as f64 * 1e5).round() / 1e5;
    PageLayer {
        kind: match layer.kind {
            Kind::Sounds => "sounds",
            Kind::Songs => "songs",
        },
        title,
        empty: layer.empty.as_deref(),
        groups: layer
            .groups
            .iter()
            .map(|group| PageGroup {
                name: &group.name,
                color: format!("#{:02x}{:02x}{:02x}", group.color[0], group.color[1], group.color[2]),
                count: group.count,
            })
            .collect(),
        places: &layer.places,
        tags: &layer.tags,
        points: PagePoints {
            id: points.iter().map(|point| point.id).collect(),
            x: points.iter().map(|point| short(point.x)).collect(),
            y: points.iter().map(|point| short(point.y)).collect(),
            group: points.iter().map(|point| point.group).collect(),
            name: points.iter().map(|point| point.name.as_str()).collect(),
            place: points.iter().map(|point| point.place).collect(),
            tags: points.iter().map(|point| point.tags).collect(),
        },
    }
}

impl Page {
    pub fn new(data: MapData) -> Page {
        let songs = data.songs.points.iter().enumerate().map(|(at, point)| (point.id, at)).collect();
        Page { data, songs }
    }

    /// What the page draws (JSON): each map's groups in their colours, the folders and tags its points name by number,
    /// and its points (x and y in [0, 1], y up), with the actions the page offers.
    pub fn points(&self) -> String {
        let map = PageMap {
            layers: [page_layer(&self.data.sounds, "Sounds"), page_layer(&self.data.songs, "Songs")],
            actions: ACTIONS.iter().map(|(action, label)| PageAction { action: action_called(*action), label }).collect(),
        };
        serde_json::to_string(&map).unwrap_or_default()
    }

    /// A point by the id the page asks for it by.
    pub fn point(&self, id: &str) -> Option<(Kind, &Point)> {
        let (kind, number) = id.split_once(':')?;
        let number: i64 = number.parse().ok()?;
        match kind {
            "sounds" => self.data.sounds.points.get(usize::try_from(number).ok()?).map(|point| (Kind::Sounds, point)),
            "songs" => self.songs.get(&number).map(|at| (Kind::Songs, &self.data.songs.points[*at])),
            _ => None,
        }
    }

    /// What an action chosen on the page asks Kumi, for the input box: none for an action or a point it doesn't have.
    pub fn asked(&self, action: &str, id: &str) -> Option<String> {
        let (action, _) = ACTIONS.iter().find(|(each, _)| action_called(*each) == action)?;
        let (kind, point) = self.point(id)?;
        Some(asked(kind, point, *action))
    }
}

/// How /map reads its data, each time it opens.
pub type LoadMap = Rc<dyn Fn() -> LocalBoxFuture<'static, MapData>>;

/// The map from this computer: the library's sounds (as this session's DAW is offered them) where the layout put
/// them, and the songs and styles in music.db.
pub fn loader(library: Rc<Library>, library_dir: PathBuf, music: Rc<Music>) -> LoadMap {
    Rc::new(move || {
        let (library, dir, music) = (library.clone(), library_dir.clone(), music.clone());
        async move { MapData { sounds: sounds(&library, dir).await, songs: songs(&music).await } }.boxed_local()
    })
}

async fn sounds(library: &Library, dir: PathBuf) -> Layer {
    let Some(map) = tokio::task::spawn_blocking(move || read_sounds_map(&dir)).await.ok().flatten() else {
        return Layer::empty(Kind::Sounds, "Kumi hasn't put your sounds on a map yet: it does once it has learned your library.");
    };
    // Joined with the library's index a slice at a time (the index lives on Kumi's thread), and laid out off it: a big
    // library's map doesn't hold up the keys and the screen.
    match library.sound_index_in(Some(crate::daw::current().id())).await {
        Ok(index) => {
            let sounds = sounds_on_map_by_turns(&map, &index).await;
            tokio::task::spawn_blocking(move || sounds_layer(sounds)).await.unwrap_or_else(|_| sounds_layer(vec![]))
        }
        Err(error) => Layer::empty(Kind::Sounds, &format!("Kumi couldn't read your library: {error}")),
    }
}

async fn songs(music: &Music) -> Layer {
    let file = kumi_listening::music::database(&music.dir());
    // Read and laid out off Kumi's thread.
    let read = tokio::task::spawn_blocking(move || -> Result<Layer, MusicError> {
        let (songs, styles): (Vec<MapSong>, Vec<MapStyle>) = match MusicStore::open_read(&file)? {
            Some(store) => store.map()?,
            None => (vec![], vec![]),
        };
        Ok(songs_layer(songs, styles))
    });
    match read.await {
        Ok(Ok(layer)) => layer,
        Ok(Err(error)) => Layer::empty(Kind::Songs, &format!("Kumi couldn't read your music: {error}")),
        Err(_) => songs_layer(vec![], vec![]),
    }
}
