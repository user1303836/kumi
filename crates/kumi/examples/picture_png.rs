//! Draws change pictures as PNGs, to check them by eye without a screenshot.
//!
//! `cargo run -p kumi --example picture_png -- <folder> [<file>]`: the file holds change records or pictures as JSON
//! (an array, or a saved conversation with its `changes`); without one, a sample of each kind is drawn. Each picture
//! is written at the size HISTORY shows it (60 × 8 cells), as Kitty's image and as cells (half-blocks as pixels).

use kumi::tui::pictures::{cells, raster::Raster, scene_of};
use kumi_types::contracts::{ChangePicture, ClipNote};
use kumi_types::picture::{Dynamics, EqBand, EqShape};
use serde_json::Value;

fn samples() -> Vec<(String, ChangePicture)> {
    let band = |shape, freq, gain_db, q| EqBand { shape, freq, gain_db, q, on: true, slope: None };
    let cut = |freq| EqBand { slope: Some(48.), ..band(EqShape::LowCut, freq, 0., 0.71) };
    let note = |pitch: f64, start: f64, duration: f64| ClipNote { pitch, start, duration, velocity: 100. };
    vec![
        (
            "eq".into(),
            ChangePicture::Eq {
                before: vec![cut(30.), band(EqShape::Bell, 250., 0., 1.), band(EqShape::HighShelf, 8000., 0., 0.71)],
                after: vec![cut(60.), band(EqShape::Bell, 250., -4.5, 1.4), band(EqShape::HighShelf, 8000., 3., 0.71)],
                values: vec![],
            },
        ),
        (
            "dynamics".into(),
            ChangePicture::Dynamics {
                before: Dynamics { threshold_db: -12., ratio: 2., knee_db: 6., makeup_db: 0. },
                after: Dynamics { threshold_db: -24., ratio: 4., knee_db: 6., makeup_db: 4. },
                values: vec![],
            },
        ),
        (
            "notes".into(),
            ChangePicture::Notes {
                length: 8.,
                before: vec![note(60., 0., 1.), note(63., 1., 1.), note(67., 2., 2.), note(70., 4., 1.)],
                after: vec![note(60., 0., 1.), note(64., 1., 1.), note(67., 2., 2.), note(72., 5., 1.5), note(55., 6., 2.)],
                partial: false,
            },
        ),
        (
            "automation".into(),
            ChangePicture::Automation {
                parameter: "Filter Freq".into(),
                min: 0.,
                max: 1.,
                before: vec![[0., 0.3], [8., 0.3]],
                after: vec![[0., 0.3], [2., 0.9], [4., 0.2], [6., 0.7], [8., 0.3]],
                words: None,
            },
        ),
    ]
}

fn pictures_in(value: &Value) -> Vec<(String, ChangePicture)> {
    let items = value.get("changes").and_then(Value::as_array).or_else(|| value.as_array()).cloned().unwrap_or_default();
    items
        .iter()
        .enumerate()
        .filter_map(|(at, item)| {
            let picture = item.get("picture").unwrap_or(item);
            let name = item.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("p{at}"));
            serde_json::from_value(picture.clone()).ok().map(|picture| (name, picture))
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(folder) = args.first() else {
        eprintln!("usage: picture_png <folder> [<file of pictures or records>]");
        std::process::exit(2);
    };
    let pictures = match args.get(1) {
        Some(file) => pictures_in(&serde_json::from_str(&std::fs::read_to_string(file).expect("read the file")).expect("JSON")),
        None => samples(),
    };
    std::fs::create_dir_all(folder).expect("make the folder");
    let (cols, rows) = (60usize, 8usize);
    for (name, picture) in pictures {
        let Some(scene) = scene_of(&picture) else {
            println!("{name}: values or levels (bars, no image)");
            continue;
        };
        let image = Raster::draw(&scene, (cols * 8) as u32, (rows * 16) as u32);
        std::fs::write(format!("{folder}/{name}-image.png"), image.png()).expect("write");
        // The cells as pixels: each half-block's two colours.
        let blocks = cells::half_blocks(&scene, cols, rows, [0x14, 0x16, 0x1a]);
        let mut pixels = Raster::new(cols as u32, (rows * 2) as u32);
        for (row, line) in blocks.iter().enumerate() {
            for (col, span) in line.iter().enumerate() {
                for (half, color) in [(0, span.style.fg.or(span.style.bg)), (1, span.style.bg)] {
                    let [r, g, b] = color.unwrap_or([0x14, 0x16, 0x1a]);
                    let at = ((row * 2 + half) * cols + col) * 4;
                    pixels.rgba[at..at + 4].copy_from_slice(&[r, g, b, 255]);
                }
            }
        }
        std::fs::write(format!("{folder}/{name}-cells.png"), pixels.png()).expect("write");
        println!("{name}: {folder}/{name}-image.png, {folder}/{name}-cells.png");
    }
}
