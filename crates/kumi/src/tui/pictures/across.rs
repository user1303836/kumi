//! One image of a change across tracks: each track's existing scene in a panel of its own. Headings are terminal
//! text over transparent rows of the image, so names remain legible at the terminal's font size and in either theme.

use kumi_types::picture::PicturePart;

use super::{cells, fit, kitty, lines_of, scene_of, styled, Drawn, Graphics, Raster, Scene};
use crate::tui::style::{palette, Rgb};
use crate::tui::width::text_width;
use crate::tui::wrap::Span;

fn columns(parts: usize, cols: usize) -> usize {
    (cols / 24).clamp(1, 3).min(parts.max(1))
}

/// Enough space for each panel's heading and six rows of its curve, roll or lane, even in a narrow HISTORY.
pub(super) fn height(parts: &[PicturePart], cols: usize) -> usize {
    parts.len().div_ceil(columns(parts.len(), cols)) * 7
}

struct Panel {
    left: usize,
    right: usize,
    top: usize,
    bottom: usize,
    heading: bool,
}

fn panels(parts: usize, cols: usize, rows: usize) -> Vec<Panel> {
    let columns = columns(parts, cols);
    let tiers = parts.div_ceil(columns).max(1);
    (0..parts)
        .map(|at| {
            let (column, tier) = (at % columns, at / columns);
            let (top, bottom) = (tier * rows / tiers, (tier + 1) * rows / tiers);
            Panel {
                left: column * cols / columns,
                right: ((column + 1) * cols / columns).saturating_sub(usize::from(column + 1 < columns)),
                top,
                bottom,
                heading: bottom > top + 1,
            }
        })
        .collect()
}

/// The same panels in one scene, with room for their headings. Cells keep each panel's own braille/blocks choice.
pub(super) fn scene(parts: &[PicturePart], cols: usize, rows: usize) -> Option<Scene> {
    let mut combined = Scene { shapes: Vec::new(), lines: true };
    for (part, panel) in parts.iter().zip(panels(parts.len(), cols, rows)) {
        let top = panel.top + usize::from(panel.heading);
        if panel.bottom <= top || panel.right <= panel.left {
            continue;
        }
        let Some(child) = scene_of(&part.picture) else { continue };
        combined.lines &= child.lines;
        combined.shapes.extend(
            child
                .in_rect([
                    panel.left as f64 / cols as f64,
                    1. - panel.bottom as f64 / rows as f64,
                    panel.right as f64 / cols as f64,
                    1. - top as f64 / rows as f64,
                ])
                .shapes,
        );
    }
    (!combined.shapes.is_empty()).then_some(combined)
}

pub(super) fn draw(
    parts: &[PicturePart],
    picture: &kumi_types::contracts::ChangePicture,
    cols: usize,
    rows: usize,
    graphics: Graphics,
    id: u32,
    ground: Rgb,
) -> Drawn {
    let lines = lines_of(picture, cols);
    let Some(scene) = (rows > 0).then(|| scene(parts, cols, rows)).flatten() else {
        return Drawn { lines, ..Default::default() };
    };
    let theme = crate::tui::style::theme();
    let ground = theme.of(ground);
    let image = (graphics == Graphics::Kitty)
        .then(|| Raster::draw(&scene.in_theme(theme), (cols * 8).min(1600) as u32, (rows * 16).min(800) as u32));
    let mut drawn = vec![vec![styled(" ", palette::TEXT); cols]; rows];
    // Right to left: a multi-cell heading occupies one span, so replacing it must not shift the cell indexes
    // of another heading still to be placed on the same row.
    for (part, panel) in parts.iter().zip(panels(parts.len(), cols, rows)).rev() {
        let width = panel.right.saturating_sub(panel.left);
        let top = panel.top + usize::from(panel.heading);
        let tall = panel.bottom.saturating_sub(top);
        if width == 0 || tall == 0 {
            continue;
        }
        if panel.heading {
            let mut heading = fit(vec![styled(&part.name, palette::TEXT)], width);
            let used: usize = heading.iter().map(|span| text_width(&span.text) as usize).sum();
            heading.extend((used..width).map(|_| styled(" ", palette::TEXT)));
            // A heading may span several cells, so splice the cell range rather than indexing spans afterwards.
            drawn[panel.top].splice(panel.left..panel.right, heading);
        }
        let Some(child) = scene_of(&part.picture) else { continue };
        let child = child.in_theme(theme);
        let content: Vec<Vec<Span>> = if graphics == Graphics::Kitty {
            (top..panel.bottom)
                .map(|row| {
                    (panel.left..panel.right)
                        .map(|col| Span::styled(kitty::placeholder(row, col), crate::tui::style::Style::fg(kitty::id_color(id))))
                        .collect()
                })
                .collect()
        } else if child.lines {
            cells::braille(&child, width, tall)
        } else {
            cells::half_blocks(&child, width, tall, ground)
        };
        for (row, content) in (top..panel.bottom).zip(content) {
            drawn[row].splice(panel.left..panel.right, content);
        }
    }
    Drawn { rows: drawn, lines, image }
}
