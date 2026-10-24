//! /map in the app: the producer's sounds and songs drawn full screen (braille, an image under the names where the
//! terminal shows images, a list on a console that shows neither), moved around with the keys, a point played through
//! Kumi's player, and an action that puts what it asks in the input box. Screens are kept as text in
//! tests/goldens/tui/map; `KUMI_BLESS=1` writes them again. `KUMI_MAP_SHOTS=<folder>` saves the map as a PNG and as
//! braille text there, to look at.
#[path = "../support/tui_app.rs"]
mod support;
use crate::map_server::{ask, get, post};
use futures::FutureExt;
use kumi::map::{songs_layer, sounds_layer, LoadMap, MapData};
use kumi::tui::map::MapView;
use kumi::tui::pictures::{kitty::DIACRITICS, Graphics};
use kumi::tui::screen::Screen;
use kumi::tui::style::palette;
use kumi_library::{map::MapSound, SoundClass, SoundKind};
use kumi_listening::music::store::{MapSong, MapStyle};
use kumi_media::audio::player::{Format, Output, Player, PlayerError};
use std::{cell::RefCell, rc::Rc};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn golden(name: &str, actual: &str) {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/tui/map").join(name);
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&file)
        .unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()))
        .replace("\r\n", "\n");
    assert!(actual == expected, "{} changed: check it, then run this test with KUMI_BLESS=1\n{actual}", file.display());
}

/// The screen as text, trailing spaces trimmed; an image's placeholder cells as ▒.
fn text(lines: &[String]) -> String {
    let cell = |c: char| if c == '\u{10EEEE}' { Some('▒') } else { (!DIACRITICS.contains(&c)).then_some(c) };
    lines.iter().map(|line| line.chars().filter_map(cell).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n") + "\n"
}

/// The same numbers every run.
struct Numbers(u32);
impl Numbers {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.0 >> 8) as f64 / (1u32 << 24) as f64
    }
    /// About `spread` round `middle`: the mean of three.
    fn near(&mut self, middle: f64, spread: f64) -> f64 {
        (middle + (self.next() + self.next() + self.next() - 1.5) * spread).clamp(0., 1.)
    }
}

/// A library and a music collection as the layout places them: a cloud of sounds for each of six classes, and songs
/// in three styles (two named by their words, one by its genre).
fn fixture(per_class: usize, per_style: usize) -> MapData {
    let mut numbers = Numbers(7);
    let classes = [
        (SoundClass::Kick, 0.22, 0.32, "Kick"),
        (SoundClass::Snare, 0.42, 0.62, "Snare"),
        (SoundClass::Hat, 0.66, 0.8, "Hat"),
        (SoundClass::Bass, 0.78, 0.36, "Bass"),
        (SoundClass::Pad, 0.5, 0.18, "Pad"),
        (SoundClass::Vocal, 0.16, 0.74, "Vox"),
    ];
    let mut sounds = vec![];
    for (class, x, y, word) in classes {
        for n in 1..=per_class {
            sounds.push(MapSound {
                path: format!("/Samples/{word}s/{word} {n:02}.wav"),
                x: numbers.near(x, 0.16),
                y: numbers.near(y, 0.16),
                name: format!("{word} {n:02}"),
                place: format!("Samples / {word}s"),
                class: Some(class),
                kind: Some(SoundKind::OneShot),
                seconds: Some(0.3 + n as f64 % 7. * 0.1),
            });
        }
    }
    let styles =
        [((0.3, 0.6), vec!["dark", "rainy"], "Dubstep"), ((0.7, 0.62), vec!["warm", "dusty"], "House"), ((0.5, 0.25), vec![], "Techno")];
    let mut songs = vec![];
    for (style, ((x, y), _, genre)) in styles.iter().enumerate() {
        for n in 1..=per_style {
            let id = (style * per_style + n) as i64;
            songs.push(MapSong {
                id,
                x: numbers.near(*x, 0.2),
                y: numbers.near(*y, 0.2),
                cluster: Some(style),
                path: format!("/Music/Artist {style}/Song {n:02}.flac"),
                name: format!("Artist {style} – Song {n:02}"),
                genre: Some(genre.to_string()),
                seconds: Some(240.),
            });
        }
    }
    let styles = styles
        .iter()
        .enumerate()
        .map(|(style, (_, names, genre))| MapStyle {
            songs: per_style,
            examples: vec![(style * per_style + 1) as i64],
            genres: vec![genre.to_string()],
            names: names.iter().map(|name| name.to_string()).collect(),
        })
        .collect();
    MapData { sounds: sounds_layer(sounds), songs: songs_layer(songs, styles) }
}

fn loader(data: MapData) -> LoadMap {
    Rc::new(move || {
        let data = data.clone();
        async move { data }.boxed_local()
    })
}

fn braille(lines: &[String]) -> bool {
    lines.iter().any(|line| line.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)))
}

/// The row under the map that names the point chosen.
fn chosen(h: &Harness) -> String {
    h.screen()[37].clone()
}

/// Where the point chosen is marked: its row and column.
fn marker(lines: &[String]) -> (usize, usize) {
    lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| line.chars().position(|c| c == '◉').map(|column| (row, column)))
        .expect("the point chosen")
}

case!(the_map_opens_full_screen_in_braille_and_esc_brings_the_conversation_back, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(loader(fixture(12, 8))));
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    let lines = h.screen();
    assert!(braille(&lines), "the points, in braille");
    golden("braille.160x40.txt", &text(&lines));
    // Tab: the songs, by style.
    h.type_text("\t").await;
    h.wait_for("c1 · dark rainy").await;
    golden("braille-songs.160x40.txt", &text(&h.screen()));
    h.type_text("\x1b").await;
    h.wait_until_hidden("Songs 24").await;
    h.close().await;
});

case!(an_arrow_moves_to_a_point_that_way_and_zooming_in_names_the_points, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(loader(fixture(12, 8))));
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    let (first, at) = (chosen(&h), marker(&h.screen()));
    h.type_text("\x1b[C").await;
    let right = marker(&h.screen());
    assert!(chosen(&h) != first && right.1 >= at.1, "another point, to the right: {at:?} → {right:?}");
    h.type_text("\x1b[A").await;
    let up = marker(&h.screen());
    assert!(up.0 <= right.0, "up: {right:?} → {up:?}");
    // Zoomed in, the points near it have their names beside them.
    for _ in 0..6 {
        h.type_text("+").await;
    }
    h.wait_for("×11.4").await;
    let lines = h.screen();
    let named = lines[2..37].iter().filter(|line| line.contains(" 0") || line.contains(" 1")).count();
    assert!(named >= 1, "names beside the points:\n{}", lines.join("\n"));
    golden("zoomed.160x40.txt", &text(&lines));
    // The filter keeps what has its words.
    h.type_text("/snare\r").await;
    h.wait_for("/ snare: 12 shown").await;
    h.close().await;
});

case!(on_a_terminal_with_images_the_points_are_one_image_under_the_names, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| {
        o.graphics = Some(Graphics::Kitty);
        o.map = Some(loader(fixture(12, 8)));
    });
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    let lines = h.screen();
    assert!(!braille(&lines));
    golden("kitty.160x40.txt", &text(&lines));
    let written = h.output.text.borrow().clone();
    let upload = written.find("\x1b_Ga=T,U=1,i=").expect("an upload");
    assert!(written[upload..].starts_with("\x1b_Ga=T,U=1,i=1,f=32,s=1264,v=560,c=158,r=35,"), "{}", &written[upload..upload + 60]);
    assert!(written[upload..].contains('\u{10EEEE}'), "placeholders after it");
    // Moving to another point draws no new image; zooming does, and takes the old one down.
    h.type_text("\x1b[C").await;
    assert_eq!(h.output.text.borrow().matches("\x1b_Ga=T,").count(), 1);
    h.type_text("+").await;
    h.wait_for("×1.5").await;
    let written = h.output.text.borrow().clone();
    assert_eq!(written.matches("\x1b_Ga=T,").count(), 2);
    assert!(written.contains("\x1b_Ga=d,d=I,i=1,"), "the first image taken down");
    h.type_text("\x1b").await;
    h.wait_until_hidden("Sounds 72").await;
    assert!(h.output.text.borrow().contains("\x1b_Ga=d,d=I,i=2,"), "closing takes the map's image down");
    h.close().await;
});

case!(a_console_without_pictures_lists_the_groups_with_counts_and_examples, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| {
        o.graphics = Some(Graphics::Words);
        o.map = Some(loader(fixture(12, 8)));
    });
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    golden("words.160x40.txt", &text(&h.screen()));
    h.type_text("\t").await;
    h.wait_for("c1 · dark rainy").await;
    golden("words-songs.160x40.txt", &text(&h.screen()));
    h.close().await;
});

case!(enter_offers_what_to_ask_and_an_action_fills_the_input_box_without_sending, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(loader(fixture(12, 8))));
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    h.type_text("\r").await;
    h.wait_for("Ask Kumi:").await;
    h.has("1 Find more like this");
    h.type_text("1").await;
    h.wait_for("Find sounds like this one: /Samples/").await;
    assert!(!h.screen().iter().any(|line| line.contains("Sounds 72")), "the map closed");
    assert!(!h.calls().iter().any(|call| call.starts_with("submit")), "nothing sent: {:?}", h.calls());
    // On the songs' map, the song by name; esc in the actions only closes them.
    h.type_text("\x15/map\r").await;
    h.wait_for("Sounds 72").await;
    h.type_text("\t").await;
    h.type_text("\r").await;
    h.wait_for("Ask Kumi:").await;
    h.type_text("\x1b").await;
    h.wait_until_hidden("Ask Kumi:").await;
    h.has("Songs 24");
    h.type_text("\r").await;
    h.type_text("\x1b[C").await;
    h.type_text("\r").await;
    h.wait_for("Use “Artist").await;
    h.close().await;
});

/// The port and the token's path of the browser's map, from the link Kumi opened.
fn served_at(url: &str) -> (u16, String) {
    let url: url::Url = url.parse().unwrap();
    (url.port().unwrap(), url.path().to_string())
}

/// The map's link, from what the browser was opened with: a link that works once (so the browser's command line,
/// which on Linux any account reads, has no token), which goes on to the map's page, and after that finds nothing.
async fn linked(opened: &str) -> String {
    let (port, once) = served_at(opened);
    assert!(opened.starts_with("http://127.0.0.1:") && once.starts_with("/once/"), "{opened}");
    let host = format!("127.0.0.1:{port}");
    let (status, head, _) = ask(port, &get(&once, &host, "")).await;
    assert_eq!(status, 302, "{head}");
    let page = head.lines().find_map(|line| line.strip_prefix("Location: ")).expect("where it goes on to");
    assert!(!opened.contains(page.trim_matches('/')), "no token in what the browser was opened with");
    assert_eq!(ask(port, &get(&once, &host, "")).await.0, 404, "once only");
    format!("http://{host}{page}")
}

case!(map_open_serves_the_map_to_the_browser_whose_actions_fill_the_input_box, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(loader(fixture(12, 8))));
    h.start().await;
    h.connect();
    h.type_text("/map open\r").await;
    h.wait_for("The map is open in your browser").await;
    let opened = h.browsed.borrow().clone();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let link = linked(&opened[0]).await;
    h.has(&link);
    let (port, home) = served_at(&link);
    let host = format!("127.0.0.1:{port}");
    let (status, _, body) = ask(port, &get(&format!("{home}points"), &host, "")).await;
    let map: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((status, map["layers"][0]["points"]["x"].as_array().unwrap().len()), (200, 72));
    // An action chosen there lands in the input box, unsent; with the terminal's map open, that closes so it shows.
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    let more = r#"{"action":"more","id":"sounds:0"}"#;
    assert_eq!(ask(port, &post(&format!("{home}action"), &host, &format!("http://{host}"), more)).await.0, 204);
    h.wait_for("Find sounds like this one: /Samples/Kicks/Kick 01.wav").await;
    assert!(!h.screen().iter().any(|line| line.contains("Sounds 72")), "the terminal's map closed");
    assert!(!h.calls().iter().any(|call| call.starts_with("submit")), "nothing sent: {:?}", h.calls());
    let song = r#"{"action":"reference","id":"songs:9"}"#;
    assert_eq!(ask(port, &post(&format!("{home}action"), &host, &format!("http://{host}"), song)).await.0, 204);
    h.wait_for("Use “Artist 1 – Song 01” as the reference: /Music/Artist 1/Song 01.flac").await;
    // A second /map open serves afresh, and the first stops answering.
    h.type_text("\x15/map open\r").await;
    for _ in 0..500 {
        if h.browsed.borrow().len() > 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let opened = h.browsed.borrow()[1].clone();
    let again = linked(&opened).await;
    assert_ne!(again, link);
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err(), "the first closed");
    // /map close: nothing answers there.
    let (port, _) = served_at(&again);
    h.type_text("/map close\r").await;
    h.wait_for("The map in your browser is closed.").await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err(), "closed");
    h.type_text("/map close\r").await;
    h.wait_for("The map isn't open in your browser: /map open opens it.").await;
    h.type_text("/map wander\r").await;
    h.wait_for("/map open shows it in your browser, and /map close closes that.").await;
    h.close().await;
});

case!(over_ssh_map_open_says_how_to_reach_the_map_and_opens_no_browser_there, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| {
        o.map = Some(loader(fixture(2, 1)));
        o.remote = true;
    });
    h.start().await;
    h.connect();
    h.type_text("/map open\r").await;
    h.wait_for("over SSH").await;
    assert!(h.browsed.borrow().is_empty(), "no browser on the computer Kumi runs on");
    // The notice wraps beside the panes: its words, each whole.
    let said = h.screen().join(" ");
    let words: Vec<&str> = said.split_whitespace().collect();
    let url = words.iter().find(|word| word.starts_with("http://127.0.0.1:")).expect("the map's link");
    let (port, _) = served_at(url);
    assert!(words.contains(&"ssh") && words.contains(&"-L"), "{said}");
    assert!(words.contains(&format!("{port}:127.0.0.1:{port}").as_str()), "the same port here as there: {said}");
    h.close().await;
});

/// An output that keeps what it's given.
struct Kept {
    started: Rc<RefCell<Vec<usize>>>,
    playing: Rc<RefCell<bool>>,
}
impl Output for Kept {
    fn format(&self) -> Format {
        Format { rate: 48_000, channels: 2 }
    }
    fn start(&self, samples: Vec<f32>) -> Result<(), PlayerError> {
        self.started.borrow_mut().push(samples.len());
        *self.playing.borrow_mut() = true;
        Ok(())
    }
    fn stop(&self) {
        *self.playing.borrow_mut() = false;
    }
    fn playing(&self) -> bool {
        *self.playing.borrow()
    }
}

case!(space_plays_the_point_chosen_and_again_stops_it, async {
    let dir = tempfile::tempdir().unwrap();
    let mut data = fixture(2, 1);
    for point in data.sounds.points.iter_mut() {
        let file = dir.path().join(format!("{}.wav", point.name));
        let samples: Vec<f32> = (0..24_000).map(|n| (n as f32 * 0.05).sin() * 0.2).collect();
        std::fs::write(&file, kumi_media::audio::decode::float_wav(&[samples], 48_000)).unwrap();
        point.path = file.to_str().unwrap().to_string();
    }
    let (started, playing) = (Rc::new(RefCell::new(vec![])), Rc::new(RefCell::new(false)));
    let opener = {
        let (started, playing) = (started.clone(), playing.clone());
        Box::new(move || Ok(Box::new(Kept { started: started.clone(), playing: playing.clone() }) as Box<dyn Output>))
    };
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| {
        o.map = Some(loader(data));
        o.player = Some(Rc::new(Player::with_opener(opener)));
    });
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 12").await;
    h.type_text(" ").await;
    h.wait_for("▶ playing").await;
    // Half a second of it (or its whole length, shorter), in both channels.
    let length = started.borrow()[0];
    assert!(length > 0 && length <= 48_000, "{length}");
    h.type_text(" ").await;
    h.wait_until_hidden("▶ playing").await;
    assert!(!*playing.borrow(), "stopped");
    h.close().await;
});

#[test]
fn the_map_drawn_for_a_look() {
    // Zoomed out, 12,000 sounds are density; zoomed in, points.
    let data = fixture(2_000, 120);
    let mut shots: Vec<(String, Vec<u8>)> = vec![];
    for (name, zoom) in [("map", 0), ("map-zoomed", 4)] {
        let mut image = MapView::new(Graphics::Kitty);
        image.loaded(data.clone());
        let mut cells = MapView::new(Graphics::Cells);
        cells.loaded(data.clone());
        for _ in 0..zoom {
            image.on_text("+");
            cells.on_text("+");
        }
        // On the terminal's ground, as it looks there.
        let mut png = None;
        image.draw(&mut Screen::new(160, 45), &mut |_, raster, _, _| {
            let mut shown = raster.clone();
            for pixel in shown.rgba.chunks_mut(4) {
                let a = pixel[3] as f64 / 255.;
                for c in 0..3 {
                    pixel[c] = (pixel[c] as f64 * a + palette::GROUND[c] as f64 * (1. - a)).round() as u8;
                }
                pixel[3] = 255;
            }
            png = Some(shown.png());
            1
        });
        let mut screen = Screen::new(160, 45);
        cells.draw(&mut screen, &mut |_, _, _, _| unreachable!("cells draw no image"));
        let lines = screen.lines();
        assert!(braille(&lines));
        shots.push((format!("{name}-kitty.png"), png.expect("an image")));
        shots.push((format!("{name}-braille.txt"), (lines.join("\n") + "\n").into_bytes()));
    }
    if let Some(folder) = std::env::var_os("KUMI_MAP_SHOTS") {
        for (name, bytes) in shots {
            std::fs::write(std::path::Path::new(&folder).join(name), bytes).unwrap();
        }
    }
}

/// How long the map takes to answer a key at 300,000 points, in the build at hand: run by hand
/// (`--run-ignored only --no-capture`); it says, and checks nothing. An image goes up once per picture, as the app
/// sends it.
#[test]
#[ignore]
fn how_long_a_key_takes_at_300_000_points() {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    let data = fixture(50_000, 1);
    eprintln!("300,000 sounds laid out for the map in {:?}", started.elapsed());
    for graphics in [Graphics::Kitty, Graphics::Cells] {
        let mut view = MapView::new(graphics);
        view.loaded(data.clone());
        let mut shown = String::new();
        // The frame, and how much of it was the image going up.
        let mut frame = |view: &mut MapView| {
            let (started, mut sent) = (Instant::now(), Duration::ZERO);
            view.draw(&mut Screen::new(200, 50), &mut |key, raster, cols, rows| {
                if key != shown {
                    let started = Instant::now();
                    std::hint::black_box(kumi::tui::pictures::kitty::transmit(1, raster, cols, rows));
                    (sent, shown) = (started.elapsed(), key.to_string());
                }
                1
            });
            (started.elapsed(), sent)
        };
        let first = frame(&mut view);
        let started = Instant::now();
        view.on_key("right", false);
        let (_, sent) = frame(&mut view);
        let moved = (started.elapsed(), sent);
        let started = Instant::now();
        view.on_text("+");
        let (_, sent) = frame(&mut view);
        let zoomed = (started.elapsed(), sent);
        eprintln!("{graphics:?} (frame, of it the upload): first {first:?}, an arrow {moved:?}, a zoom {zoomed:?}");
    }
}

/// The answer to a request, ending on a question with options.
fn asks_which(h: &Harness) {
    h.emit(serde_json::json!({"type":"text","text":"Which track should get the reverb?\n\n1. Bubbles\n2. Drums"}));
    h.emit(serde_json::json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(serde_json::json!({"type":"state","state":"idle"}));
}

case!(a_question_kumi_asks_while_the_map_is_open_shows_in_its_place_and_the_map_comes_back_after, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(loader(fixture(12, 8))));
    h.start().await;
    h.connect();
    h.type_text("add a reverb\r").await;
    h.emit(serde_json::json!({"type":"state","state":"running"}));
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    // The answer ends asking which: its options show in the map's place, and take the keys.
    asks_which(&h);
    h.wait_for("Your answer").await;
    assert!(!has(&h.screen(), "Sounds 72"), "the map gives way");
    h.type_text("2").await;
    h.type_text("\r").await;
    h.wait_for_call("submit:Drums").await;
    // Answered: the map's back.
    h.wait_for("Sounds 72").await;
    h.close().await;
});

case!(an_action_from_the_browser_while_kumi_offers_answers_takes_their_place_and_enter_sends_it, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(loader(fixture(12, 8))));
    h.start().await;
    h.connect();
    h.type_text("/map open\r").await;
    h.wait_for("The map is open in your browser").await;
    let opened = h.browsed.borrow()[0].clone();
    let (port, home) = served_at(&linked(&opened).await);
    let host = format!("127.0.0.1:{port}");
    h.type_text("add a reverb\r").await;
    h.emit(serde_json::json!({"type":"state","state":"running"}));
    asks_which(&h);
    h.wait_for("Your answer").await;
    let more = r#"{"action":"more","id":"sounds:0"}"#;
    assert_eq!(ask(port, &post(&format!("{home}action"), &host, &format!("http://{host}"), more)).await.0, 204);
    h.wait_for("Find sounds like this one: /Samples/Kicks/Kick 01.wav").await;
    h.wait_until_hidden("Your answer").await;
    h.type_text("\r").await;
    h.wait_for_call("submit:Find sounds like this one: /Samples/Kicks/Kick 01.wav").await;
    h.close().await;
});

case!(a_jobs_answer_waits_while_the_producer_is_in_the_map, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(loader(fixture(12, 8))));
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 72").await;
    let job = |state: &str| serde_json::json!({"type":"job","id":1,"kind":"worker","title":"Burial vocal chops","state":state,"whenDone":"continue","startedAt":1,"steps":0,"sources":0,"line":"Done.","endedAt":2});
    h.emit(job("running"));
    h.emit(job("done"));
    delay(100).await;
    assert!(!h.calls().iter().any(|call| call.starts_with("carry-on")), "not while they're in the map: {:?}", h.calls());
    h.type_text("\x1b").await;
    h.wait_for_call("carry-on:1").await;
    h.close().await;
});

case!(map_close_while_the_browsers_map_is_still_read_opens_none, async {
    let read = Rc::new(tokio::sync::Notify::new());
    let load: LoadMap = {
        let (read, data) = (read.clone(), fixture(2, 1));
        Rc::new(move || {
            let (read, data) = (read.clone(), data.clone());
            async move {
                read.notified().await;
                data
            }
            .boxed_local()
        })
    };
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.map = Some(load));
    h.start().await;
    h.connect();
    h.type_text("/map open\r").await;
    h.wait_for("Kumi is reading your map for the browser").await;
    h.type_text("/map close\r").await;
    h.wait_for("The map won't open in your browser after all.").await;
    read.notify_one();
    delay(200).await;
    assert!(h.browsed.borrow().is_empty(), "no browser opened: {:?}", h.browsed.borrow());
    h.type_text("/map close\r").await;
    h.wait_for("The map isn't open in your browser: /map open opens it.").await;
    h.close().await;
});

/// An output that plays for as long as what it's given lasts, and `tail` more (the last block, a fade).
struct Timed {
    ends: Rc<std::cell::Cell<Option<std::time::Instant>>>,
    tail: std::time::Duration,
}
impl Output for Timed {
    fn format(&self) -> Format {
        Format { rate: 48_000, channels: 2 }
    }
    fn start(&self, samples: Vec<f32>) -> Result<(), PlayerError> {
        let lasts = std::time::Duration::from_secs_f64(samples.len() as f64 / 96_000.) + self.tail;
        self.ends.set(Some(std::time::Instant::now() + lasts));
        Ok(())
    }
    fn stop(&self) {
        self.ends.set(None);
    }
    fn playing(&self) -> bool {
        self.ends.get().is_some_and(|end| std::time::Instant::now() < end)
    }
}

/// The map, its sounds real files of a quarter second, played through a Timed output.
fn timed(dir: &std::path::Path, tail: std::time::Duration) -> Harness {
    let mut data = fixture(2, 1);
    for point in data.sounds.points.iter_mut() {
        let file = dir.join(format!("{}.wav", point.name));
        let samples: Vec<f32> = (0..12_000).map(|n| (n as f32 * 0.05).sin() * 0.2).collect();
        std::fs::write(&file, kumi_media::audio::decode::float_wav(&[samples], 48_000)).unwrap();
        point.path = file.to_str().unwrap().to_string();
        point.seconds = 0.25;
    }
    for point in data.songs.points.iter_mut() {
        point.path = dir.join("not there.wav").to_str().unwrap().to_string();
    }
    Harness::with(160, 40, Rc::new(Control::default()), move |o| {
        o.map = Some(loader(data));
        let ends = Rc::new(std::cell::Cell::new(None));
        o.player =
            Some(Rc::new(Player::with_opener(Box::new(move || Ok(Box::new(Timed { ends: ends.clone(), tail }) as Box<dyn Output>)))));
    })
}

case!(the_playing_mark_goes_once_the_output_has_played_the_last_of_it, async {
    let dir = tempfile::tempdir().unwrap();
    // The output still plays a moment after the sound's length: the mark waits for it, and goes.
    let h = timed(dir.path(), std::time::Duration::from_millis(300));
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 12").await;
    h.type_text(" ").await;
    h.wait_for("▶ playing").await;
    h.wait_until_hidden("▶ playing").await;
    h.close().await;
});

case!(a_play_that_ended_says_nothing_over_what_a_later_one_said, async {
    let dir = tempfile::tempdir().unwrap();
    let h = timed(dir.path(), std::time::Duration::ZERO);
    h.start().await;
    h.connect();
    h.type_text("/map\r").await;
    h.wait_for("Sounds 12").await;
    // A sound plays; meanwhile a song (a file that isn't there) can't be played, and says why.
    h.type_text(" ").await;
    h.wait_for("▶ playing").await;
    h.type_text("\t").await;
    h.type_text(" ").await;
    h.wait_for("There's no file there.").await;
    // The sound's play has ended since: what the song said stays.
    delay(600).await;
    h.has("There's no file there.");
    h.close().await;
});

case!(the_browsers_map_closes_when_the_music_it_shows_is_forgotten_or_its_folder_goes, async {
    let folder = tempfile::tempdir().unwrap();
    let songs = folder.path().join("Songs");
    std::fs::create_dir_all(&songs).unwrap();
    let songs = songs.to_string_lossy().into_owned();
    let mut options =
        kumi_listening::music::learning::MusicOptions::new(folder.path().join("kumi/music"), Some(vec![songs.clone()]), Default::default());
    (options.fork, options.delay_ms) = (false, 3_600_000);
    let music = kumi_listening::music::learning::create_music(options);
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| {
        o.map = Some(loader(fixture(2, 1)));
        o.music = Some(music.clone());
    });
    h.start().await;
    h.connect();
    let served = |h: &Harness, at: usize| served_at(&h.browsed.borrow()[at]).0;
    h.type_text("/map open\r").await;
    h.wait_for("The map is open in your browser").await;
    let port = served(&h, 0);
    // The folder taken away: the page, which lists its songs, closes.
    h.type_text(&format!("/music remove {songs}\r")).await;
    h.wait_for("The map in your browser closed: it showed your music as it was").await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err(), "closed");
    // Forgotten: the same.
    h.type_text("/map open\r").await;
    for _ in 0..500 {
        if h.browsed.borrow().len() > 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let port = served(&h, 1);
    h.type_text("/music forget\r").await;
    h.wait_for("Forget my music").await;
    h.type_text("\r").await;
    h.wait_for("Forgot your music.").await;
    h.wait_for("The map in your browser closed").await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err(), "closed");
    h.close().await;
});
