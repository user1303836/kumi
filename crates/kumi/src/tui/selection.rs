//! Text selected from the displayed cells. Keep that frame while selecting so streaming cannot
//! replace the words under the mouse. A selection stays inside the pane where the drag began.

use super::screen::{Rect, Screen};

pub(super) struct Selection {
    pub screen: Screen,
    area: Rect,
    anchor: (i32, i32),
    end: (i32, i32),
    pub dragging: bool,
    pub moved: bool,
}

impl Selection {
    pub fn new(screen: Screen, area: Rect, x: i32, y: i32) -> Self {
        let mut selection = Self { screen, area, anchor: (0, 0), end: (0, 0), dragging: true, moved: false };
        selection.anchor = selection.point(x, y);
        selection.end = selection.anchor;
        selection
    }

    fn point(&self, x: i32, y: i32) -> (i32, i32) {
        let y = y.clamp(self.area.y, self.area.y + self.area.height - 1);
        let mut x = x.clamp(self.area.x, self.area.x + self.area.width - 1);
        if x > self.area.x && self.screen.widths[(y * self.screen.width + x) as usize] == 0 {
            x -= 1;
        }
        (y, x)
    }

    pub fn update(&mut self, x: i32, y: i32) {
        self.end = self.point(x, y);
        self.moved |= self.end != self.anchor;
    }

    fn spans(&self) -> Vec<(i32, i32, i32)> {
        let (first, last) = if self.anchor <= self.end { (self.anchor, self.end) } else { (self.end, self.anchor) };
        (first.0..=last.0)
            .map(|y| {
                let left = if y == first.0 { first.1 } else { self.area.x };
                let right = if y == last.0 {
                    last.1 + i32::from(self.screen.widths[(y * self.screen.width + last.1) as usize].max(1))
                } else {
                    self.area.x + self.area.width
                };
                (y, left, right.min(self.area.x + self.area.width))
            })
            .collect()
    }

    pub fn text(&self) -> String {
        self.spans()
            .into_iter()
            .map(|(y, left, right)| {
                let mut line = String::new();
                for x in left..right {
                    let at = (y * self.screen.width + x) as usize;
                    if self.screen.widths[at] != 0 {
                        line.push_str(&self.screen.chars[at]);
                    }
                }
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn painted(&self) -> Screen {
        let mut screen = self.screen.clone();
        if self.moved {
            for (y, left, right) in self.spans() {
                for x in left..right {
                    let at = (y * screen.width + x) as usize;
                    let mut style = screen.table.style(screen.styles[at]);
                    style.inverse = !style.inverse;
                    screen.styles[at] = screen.table.id(&style);
                }
            }
        }
        screen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::style::Style;

    #[test]
    fn reverse_drag_keeps_unicode_and_excludes_the_other_pane() {
        let mut screen = Screen::new(20, 3);
        screen.put(0, 0, "A界e\u{301}  ", &Style::default());
        screen.put(0, 1, "second", &Style::default());
        screen.put(10, 0, "OTHER", &Style::default());
        let mut selection = Selection::new(screen, Rect::new(0, 0, 8, 3), 5, 1);
        selection.update(2, 0); // continuation cell of 界
        assert_eq!(selection.text(), "界e\u{301}\nsecond");
        let painted = selection.painted();
        assert!(painted.table.style(painted.styles[1]).inverse);
        assert!(painted.table.style(painted.styles[2]).inverse);
        assert!(!painted.table.style(painted.styles[10]).inverse);
    }

    #[test]
    fn a_drag_outside_the_screen_clamps_to_the_original_pane() {
        let mut screen = Screen::new(10, 2);
        screen.put(5, 0, "right", &Style::default());
        screen.put(5, 1, "end", &Style::default());
        let mut selection = Selection::new(screen, Rect::new(5, 0, 5, 2), 5, 0);
        selection.update(500, 500);
        assert_eq!(selection.text(), "right\nend");
    }
}
