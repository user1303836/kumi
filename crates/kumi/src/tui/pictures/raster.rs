//! A scene drawn into RGBA pixels, for terminal graphics: lines smoothed at their edges, boxes filled, on a clear
//! ground (the terminal's own shows through). Also a PNG of it, to check a picture by eye without a screenshot.

use std::io::Write;

use super::scene::{Scene, Shape};

/// RGBA pixels, row by row from the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    pub fn new(width: u32, height: u32) -> Raster {
        Raster { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }

    /// The scene at `width` × `height`.
    pub fn draw(scene: &Scene, width: u32, height: u32) -> Raster {
        let mut raster = Raster::new(width.max(1), height.max(1));
        raster.paint(scene);
        raster
    }

    /// The scene drawn over what's there.
    pub fn paint(&mut self, scene: &Scene) {
        for shape in &scene.shapes {
            // A dot is laid straight on, around itself only: a map draws thousands.
            if let Shape::Dot { x, y, radius, color, alpha } = shape {
                self.dot((*x, *y), radius / 1000. * self.height as f64, *color, *alpha);
                continue;
            }
            // Each other shape's cover is worked out whole, then laid on once: joins in a line aren't laid twice.
            let mut cover = vec![0f32; (self.width * self.height) as usize];
            match shape {
                Shape::Line { points, width, .. } => self.line(&mut cover, points, (width / 1000. * self.height as f64).max(1.)),
                Shape::Rect { x0, y0, x1, y1, .. } => self.rect(&mut cover, *x0, *y0, *x1, *y1),
                Shape::Dot { .. } => {}
            }
            let (color, alpha) = match shape {
                Shape::Line { color, alpha, .. } | Shape::Rect { color, alpha, .. } | Shape::Dot { color, alpha, .. } => (*color, *alpha),
            };
            self.lay(&cover, color, alpha);
        }
    }

    /// A soft light under what's drawn next: each of `cols` × `rows` cells' colour at its share (0 to 1), blended
    /// smoothly from one cell's middle to the next.
    pub fn glow(&mut self, cells: &[Option<([u8; 3], f64)>], cols: usize, rows: usize) {
        if cols == 0 || rows == 0 || cells.len() < cols * rows {
            return;
        }
        // A cell's light with its colour weighted by its share (so faint cells pull the colour less), and the share.
        let light = |col: isize, row: isize| -> [f64; 4] {
            let (col, row) = (col.clamp(0, cols as isize - 1) as usize, row.clamp(0, rows as isize - 1) as usize);
            match cells[row * cols + col] {
                Some((color, share)) => [color[0] as f64 * share, color[1] as f64 * share, color[2] as f64 * share, share],
                None => [0.; 4],
            }
        };
        let (cell_width, cell_height) = (self.width as f64 / cols as f64, self.height as f64 / rows as f64);
        for py in 0..self.height {
            let down = (py as f64 + 0.5) / cell_height - 0.5;
            let (row, ty) = (down.floor() as isize, down - down.floor());
            for px in 0..self.width {
                let across = (px as f64 + 0.5) / cell_width - 0.5;
                let (col, tx) = (across.floor() as isize, across - across.floor());
                let [a, b, c, d] = [light(col, row), light(col + 1, row), light(col, row + 1), light(col + 1, row + 1)];
                let mixed: [f64; 4] =
                    std::array::from_fn(|i| (a[i] * (1. - tx) + b[i] * tx) * (1. - ty) + (c[i] * (1. - tx) + d[i] * tx) * ty);
                if mixed[3] > 0.004 {
                    let color = [0, 1, 2].map(|i| (mixed[i] / mixed[3]).round().clamp(0., 255.) as u8);
                    self.blend((py * self.width + px) as usize, color, mixed[3]);
                }
            }
        }
    }

    /// The pixel at (x, y), x from the left, y from the top.
    pub fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    fn to_pixel(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (x * (self.width - 1) as f64, (1. - y) * (self.height - 1) as f64)
    }

    fn line(&self, cover: &mut [f32], points: &[(f64, f64)], thickness: f64) {
        let half = thickness / 2.;
        let points: Vec<(f64, f64)> = points.iter().map(|p| self.to_pixel(*p)).collect();
        let segments: Vec<((f64, f64), (f64, f64))> = match points.as_slice() {
            [only] => vec![(*only, *only)],
            many => many.windows(2).map(|pair| (pair[0], pair[1])).collect(),
        };
        for (a, b) in segments {
            let (left, right) = (a.0.min(b.0) - half - 1., a.0.max(b.0) + half + 1.);
            let (top, bottom) = (a.1.min(b.1) - half - 1., a.1.max(b.1) + half + 1.);
            for y in (top.floor().max(0.) as u32)..=(bottom.ceil().min((self.height - 1) as f64) as u32) {
                for x in (left.floor().max(0.) as u32)..=(right.ceil().min((self.width - 1) as f64) as u32) {
                    let distance = to_segment((x as f64, y as f64), a, b);
                    let covered = (half + 0.5 - distance).clamp(0., 1.) as f32;
                    let i = (y * self.width + x) as usize;
                    cover[i] = cover[i].max(covered);
                }
            }
        }
    }

    fn rect(&self, cover: &mut [f32], x0: f64, y0: f64, x1: f64, y1: f64) {
        let (w, h) = (self.width as f64, self.height as f64);
        let (left, right) = ((x0.min(x1) * w).floor(), (x0.max(x1) * w).ceil().max((x0.min(x1) * w).floor() + 1.));
        let (top, bottom) = (((1. - y0.max(y1)) * h).floor(), ((1. - y0.min(y1)) * h).ceil().max(((1. - y0.max(y1)) * h).floor() + 1.));
        for y in (top.max(0.) as u32)..(bottom.min(h) as u32) {
            for x in (left.max(0.) as u32)..(right.min(w) as u32) {
                cover[(y * self.width + x) as usize] = 1.;
            }
        }
    }

    /// A disc `radius` pixels round its centre, smoothed at its edge.
    fn dot(&mut self, at: (f64, f64), radius: f64, color: [u8; 3], alpha: f64) {
        let (x, y) = self.to_pixel(at);
        let radius = radius.max(0.5);
        let span = |middle: f64, size: u32| {
            let (low, high) = ((middle - radius - 1.).floor().max(0.), (middle + radius + 1.).ceil().min(size as f64 - 1.));
            (low as u32)..=(high.max(low - 1.) as u32)
        };
        if !(x.is_finite() && y.is_finite()) || x < -radius - 1. || y < -radius - 1. {
            return;
        }
        // Inside its edge a pixel is covered and outside it isn't: only the edge's pixels are measured.
        let (inside, outside) = ((radius - 0.5).max(0.).powi(2), (radius + 0.5).powi(2));
        for py in span(y, self.height) {
            for px in span(x, self.width) {
                let squared = (px as f64 - x).powi(2) + (py as f64 - y).powi(2);
                let covered = match squared {
                    s if s <= inside => 1.,
                    s if s >= outside => continue,
                    s => (radius + 0.5 - s.sqrt()).clamp(0., 1.),
                };
                self.blend((py * self.width + px) as usize, color, covered * alpha);
            }
        }
    }

    fn lay(&mut self, cover: &[f32], color: [u8; 3], alpha: f64) {
        for (i, covered) in cover.iter().enumerate() {
            self.blend(i, color, *covered as f64 * alpha);
        }
    }

    /// `color` laid over pixel `i` at `a` (0 to 1).
    fn blend(&mut self, i: usize, color: [u8; 3], a: f64) {
        if a <= 0. {
            return;
        }
        let pixel = &mut self.rgba[i * 4..i * 4 + 4];
        let under = pixel[3] as f64 / 255.;
        let out = a + under * (1. - a);
        for c in 0..3 {
            let mixed = (color[c] as f64 * a + pixel[c] as f64 * under * (1. - a)) / out.max(1e-9);
            pixel[c] = mixed.round().clamp(0., 255.) as u8;
        }
        pixel[3] = (out * 255.).round().clamp(0., 255.) as u8;
    }

    /// The pixels as a PNG file.
    pub fn png(&self) -> Vec<u8> {
        let mut rows = Vec::with_capacity((self.width * self.height * 4 + self.height) as usize);
        for row in self.rgba.chunks((self.width * 4) as usize) {
            rows.push(0);
            rows.extend_from_slice(row);
        }
        let mut deflate = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = deflate.write_all(&rows);
        let data = deflate.finish().unwrap_or_default();
        let mut header = Vec::new();
        header.extend_from_slice(&self.width.to_be_bytes());
        header.extend_from_slice(&self.height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, body) in [(*b"IHDR", header), (*b"IDAT", data), (*b"IEND", Vec::new())] {
            png.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let start = png.len();
            png.extend_from_slice(&kind);
            png.extend_from_slice(&body);
            let crc = crc32(&png[start..]);
            png.extend_from_slice(&crc.to_be_bytes());
        }
        png
    }
}

/// How far `p` is from the segment `a`–`b`.
fn to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0. { 0. } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0., 1.) };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_and_a_box_land_where_they_should() {
        let scene = Scene {
            shapes: vec![
                Shape::Line { points: vec![(0., 0.5), (1., 0.5)], color: [255, 0, 0], alpha: 1., width: 100. },
                Shape::Rect { x0: 0., y0: 0., x1: 0.25, y1: 0.25, color: [0, 0, 255], alpha: 1. },
            ],
            lines: false,
        };
        let raster = Raster::draw(&scene, 40, 20);
        let middle = raster.at(20, 10);
        assert_eq!(middle, [255, 0, 0, 255], "the line runs through the middle");
        assert_eq!(raster.at(20, 1)[3], 0, "clear away from it");
        assert_eq!(raster.at(2, 18), [0, 0, 255, 255], "the box is at the bottom left");
        assert_eq!(raster.at(30, 18)[3], 0);
    }

    #[test]
    fn a_dot_is_a_disc_where_it_should_be_and_nowhere_else() {
        let dot = |x: f64, y: f64, color: [u8; 3]| Shape::Dot { x, y, radius: 100., color, alpha: 1. };
        let scene = Scene { shapes: vec![dot(0.25, 0.75, [255, 0, 0]), dot(2., 0.5, [0, 255, 0]), dot(1., 0., [0, 0, 255])], lines: false };
        let raster = Raster::draw(&scene, 41, 21);
        // 2.1 pixels round (a tenth of 21), at (10, 5) from the top left.
        assert_eq!(raster.at(10, 5), [255, 0, 0, 255], "its middle");
        assert_eq!(raster.at(11, 5), [255, 0, 0, 255], "a pixel along");
        assert!((1..255).contains(&raster.at(12, 5)[3]), "its edge, smoothed: {:?}", raster.at(12, 5));
        assert_eq!(raster.at(14, 5)[3], 0, "four along: clear");
        assert_eq!(raster.at(40, 20), [0, 0, 255, 255], "a dot in the corner is drawn as far as it shows");
        assert!(raster.rgba.chunks(4).all(|pixel| pixel[1] == 0), "one off the picture draws nothing");
    }

    #[test]
    fn a_glow_fades_smoothly_from_a_lit_cell_to_one_that_isnt_and_lies_under_what_is_drawn_next() {
        // Two cells across, 40 pixels each: the left one lit red at half, the right one not.
        let mut raster = Raster::new(80, 10);
        raster.glow(&[Some(([200, 0, 0], 0.5)), None], 2, 1);
        assert_eq!(raster.at(10, 5), [200, 0, 0, 128], "the left cell's middle, and left of it");
        let fading: Vec<u8> = [30, 40, 50].iter().map(|x| raster.at(*x, 5)[3]).collect();
        assert!(fading[0] > fading[1] && fading[1] > fading[2] && fading[2] > 0, "{fading:?}");
        assert_eq!(raster.at(70, 5)[3], 0, "the right cell's middle: none");
        raster.paint(&Scene { shapes: vec![Shape::Dot { x: 0.125, y: 0.5, radius: 300., color: [0, 0, 255], alpha: 1. }], lines: false });
        assert_eq!(raster.at(10, 5), [0, 0, 255, 255], "a dot over it");
    }

    #[test]
    fn a_png_is_a_png() {
        let png = Raster::new(3, 2).png();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.ends_with(&[0xae, 0x42, 0x60, 0x82]), "IEND's CRC");
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }
}
