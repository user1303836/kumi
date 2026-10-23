//! /map in the app: the producer's sounds and songs drawn full screen (braille, an image under the names where the
//! terminal shows images, a list on a console that shows neither), moved around with the keys, a point played through
//! Kumi's player, and an action that puts what it asks in the input box. Screens are kept as text in
//! tests/goldens/tui/map; `KUMI_BLESS=1` writes them again. `KUMI_MAP_SHOTS=<folder>` saves the map as a PNG and as
//! braille text there, to look at.
#[path = "../support/tui_app.rs"]
mod support;
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
    let data = fixture(500, 120);
    let mut shots: Vec<(String, Vec<u8>)> = vec![];
    for (name, zoom) in [("map", 0), ("map-zoomed", 5)] {
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
