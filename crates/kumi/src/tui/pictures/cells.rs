//! A picture in character cells, for terminals without graphics: half-blocks (two pixels a cell, in colour) for
//! areas such as notes, braille (eight dots a cell) for curves.

use super::super::style::{Rgb, Style};
use super::super::wrap::Span;
use super::raster::Raster;
use super::scene::{Scene, Shape};

/// The scene in half-blocks, `cols` wide and `rows` tall: each cell's top and bottom pixel; clear ones are `ground`.
pub fn half_blocks(scene: &Scene, cols: usize, rows: usize, ground: Rgb) -> Vec<Vec<Span>> {
    let raster = Raster::draw(scene, cols as u32, (rows * 2) as u32);
    let pixel = |x: usize, y: usize| {
        let [r, g, b, a] = raster.at(x as u32, y as u32);
        let a = a as f64 / 255.;
        let mix = |c: u8, under: u8| (c as f64 * a + under as f64 * (1. - a)).round() as u8;
        [mix(r, ground[0]), mix(g, ground[1]), mix(b, ground[2])]
    };
    (0..rows)
        .map(|row| {
            (0..cols)
                .map(|col| {
                    let (top, bottom) = (pixel(col, row * 2), pixel(col, row * 2 + 1));
                    if top == ground && bottom == ground {
                        Span::styled(" ", Style { bg: Some(ground), ..Default::default() })
                    } else {
                        Span::styled("▀", Style { fg: Some(top), bg: Some(bottom), ..Default::default() })
                    }
                })
                .collect()
        })
        .collect()
}

/// The scene's lines and dots in braille, `cols` wide and `rows` tall, a dot a disc of braille's dots as wide as it is
/// (one under a dot's width): a cell takes the colour of the last line through it (what's there now over what came
/// before), or the colour most of its dots have.
pub fn braille(scene: &Scene, cols: usize, rows: usize) -> Vec<Vec<Span>> {
    let (width, height) = (cols * 2, rows * 4);
    let mut dots = vec![0u8; cols * rows];
    let mut colors: Vec<Option<Rgb>> = vec![None; cols * rows];
    // Each cell's dots by colour, in the order the colours came.
    let mut counted: Vec<Vec<(Rgb, u32)>> = vec![vec![]; cols * rows];
    let at = |(x, y): (f64, f64)| ((x * (width - 1) as f64).round() as i64, ((1. - y) * (height - 1) as f64).round() as i64);
    let cell_of = |x: i64, y: i64| {
        (x >= 0 && y >= 0 && x < width as i64 && y < height as i64)
            .then(|| (y as usize / 4 * cols + x as usize / 2, DOT[x as usize % 2][y as usize % 4]))
    };
    for shape in &scene.shapes {
        if let Shape::Dot { x, y, radius, color, .. } = shape {
            let (x, y) = at((*x, *y));
            // Its radius in braille's dots (a scene gives it in thousandths of the picture's height), about the dot it's on.
            let reach = radius / 1000. * height as f64;
            let most = reach.floor().max(0.) as i64;
            for dy in -most..=most {
                for dx in -most..=most {
                    if (dx * dx + dy * dy) as f64 > reach * reach && (dx, dy) != (0, 0) {
                        continue;
                    }
                    let Some((cell, dot)) = cell_of(x + dx, y + dy) else { continue };
                    dots[cell] |= dot;
                    match counted[cell].iter_mut().find(|(seen, _)| seen == color) {
                        Some((_, count)) => *count += 1,
                        None => counted[cell].push((*color, 1)),
                    }
                }
            }
            continue;
        }
        let Shape::Line { points, color, .. } = shape else { continue };
        let mut mark = |x: i64, y: i64| {
            if let Some((cell, dot)) = cell_of(x, y) {
                dots[cell] |= dot;
                colors[cell] = Some(*color);
            }
        };
        let pixels: Vec<(i64, i64)> = points.iter().map(|p| at(*p)).collect();
        match pixels.as_slice() {
            [only] => mark(only.0, only.1),
            many => {
                for pair in many.windows(2) {
                    for (x, y) in segment(pair[0], pair[1]) {
                        mark(x, y);
                    }
                }
            }
        }
    }
    for (cell, counts) in counted.iter().enumerate() {
        // The most common; of as many, the one that came last.
        if let Some((color, _)) = counts.iter().max_by_key(|(_, count)| *count) {
            colors[cell] = Some(*color);
        }
    }
    (0..rows)
        .map(|row| {
            (0..cols)
                .map(|col| {
                    let cell = row * cols + col;
                    match colors[cell] {
                        Some(color) if dots[cell] != 0 => {
                            Span::styled(char::from_u32(0x2800 + dots[cell] as u32).unwrap().to_string(), Style::fg(color))
                        }
                        _ => Span::styled(" ", Style::default()),
                    }
                })
                .collect()
        })
        .collect()
}

/// The bit of each dot in a braille cell, by column and row.
const DOT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// The grid points from `a` to `b` (Bresenham's line).
fn segment(a: (i64, i64), b: (i64, i64)) -> Vec<(i64, i64)> {
    let (mut x, mut y) = a;
    let (dx, dy) = ((b.0 - a.0).abs(), -(b.1 - a.1).abs());
    let (sx, sy) = (if a.0 < b.0 { 1 } else { -1 }, if a.1 < b.1 { 1 } else { -1 });
    let mut error = dx + dy;
    let mut points = vec![(x, y)];
    while (x, y) != b {
        let twice = 2 * error;
        if twice >= dy {
            error += dy;
            x += sx;
        }
        if twice <= dx {
            error += dx;
            y += sy;
        }
        points.push((x, y));
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rows: &[Vec<Span>]) -> Vec<String> {
        rows.iter().map(|row| row.iter().map(|span| span.text.as_str()).collect()).collect()
    }

    #[test]
    fn a_flat_line_is_a_row_of_braille_dots_in_its_colour() {
        let scene =
            Scene { shapes: vec![Shape::Line { points: vec![(0., 0.), (1., 0.)], color: [1, 2, 3], alpha: 1., width: 10. }], lines: true };
        let rows = braille(&scene, 4, 2);
        assert_eq!(text(&rows), ["    ", "⣀⣀⣀⣀"]);
        assert_eq!(rows[1][0].style.fg, Some([1, 2, 3]));
    }

    #[test]
    fn what_is_there_now_colours_a_cell_over_what_came_before() {
        let line = |y: f64, color: Rgb| Shape::Line { points: vec![(0., y), (1., y)], color, alpha: 1., width: 10. };
        let scene = Scene { shapes: vec![line(1., [9, 9, 9]), line(0.9, [1, 1, 1])], lines: true };
        let rows = braille(&scene, 2, 1);
        assert_eq!(rows[0][0].style.fg, Some([1, 1, 1]));
    }

    #[test]
    fn dots_mark_their_braille_dot_and_a_cell_takes_their_commonest_colour() {
        let dot = |x: f64, y: f64, color: Rgb| Shape::Dot { x, y, radius: 5., color, alpha: 1. };
        let (red, blue) = ([200, 0, 0], [0, 0, 200]);
        // 4 × 4 dots across two cells: the left cell has two red and one blue, the right one blue then one red.
        let scene = Scene {
            shapes: vec![dot(0., 1., red), dot(0., 0., blue), dot(1. / 3., 0., red), dot(1., 1., blue), dot(1., 0., red)],
            lines: false,
        };
        let rows = braille(&scene, 2, 1);
        assert_eq!(text(&rows), ["\u{28c1}\u{2888}"]);
        assert_eq!(rows[0][0].style.fg, Some(red), "two red to one blue");
        assert_eq!(rows[0][1].style.fg, Some(red), "as many of each: the one that came last");
        assert_eq!(text(&braille(&Scene { shapes: vec![dot(2., 2., red)], lines: false }, 2, 1)), ["  "], "off the picture: nothing");
    }

    #[test]
    fn a_dot_a_few_of_brailles_dots_wide_is_a_disc_of_them_about_it() {
        // 8 × 8 dots (4 × 2 cells); a dot of radius 2 dots (250 thousandths of 8) at the dot (2, 3) from the top left.
        let scene =
            Scene { shapes: vec![Shape::Dot { x: 2. / 7., y: 1. - 3. / 7., radius: 250., color: [9, 9, 9], alpha: 1. }], lines: false };
        let rows = braille(&scene, 4, 2);
        let lit: u32 =
            rows.iter().flatten().flat_map(|span| span.text.chars()).map(|c| (c as u32).saturating_sub(0x2800).count_ones()).sum();
        assert_eq!(lit, 13, "the dot, four a dot away, four diagonally, four two away: {:?}", text(&rows));
        assert_eq!(text(&rows), ["\u{28e0}\u{28e6}\u{2840} ", "\u{2808}\u{280b}  "]);
    }

    #[test]
    fn half_blocks_show_top_and_bottom_pixels() {
        let scene = Scene { shapes: vec![Shape::Rect { x0: 0., y0: 0., x1: 1., y1: 0.5, color: [200, 0, 0], alpha: 1. }], lines: false };
        let rows = half_blocks(&scene, 2, 1, [0, 0, 0]);
        assert_eq!(text(&rows), ["▀▀"]);
        assert_eq!((rows[0][0].style.fg, rows[0][0].style.bg), (Some([0, 0, 0]), Some([200, 0, 0])), "the bottom half is filled");
        let empty = half_blocks(&Scene { shapes: vec![], lines: false }, 2, 1, [5, 5, 5]);
        assert_eq!(text(&empty), ["  "]);
    }
}
