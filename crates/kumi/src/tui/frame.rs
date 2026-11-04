//! Rounded frames: the boxes Kumi's chrome is drawn in (the input box, what opens over the conversation, the pane), with
//! words set into their edges (`╭─ Title ──── hint ─╮`) and rules across them (`├─ NOW ───┤`). Words in a frame's edge
//! start `INSET` cells in from its side, where what the input box and the panels hold starts too; the pane and the
//! drawer hold theirs `SNUG` to their sides. On the consoles that show badges for Kumi's icons, the corners are square:
//! their fonts have the box drawing every console has, and not its arcs.

use super::screen::{Rect, Screen};
use super::style::{palette, Style};
use super::width::{text_width, truncate};
use super::wrap::Span;

/// A frame's line; the frame the keyboard is in has its line lit.
pub const LINE: Style = Style::fg(palette::RULE);
pub const LIT: Style = Style::fg(palette::TRACK);

/// The cells from a frame's side to the words in its edges and to what it holds: its line and two blanks.
pub const INSET: i32 = 3;
/// The cells from a frame's side to what the pane and the drawer hold: its line and one blank. Their trees, piano rolls
/// and pictures need the room more than their rows need to line up with the words in the edges.
pub const SNUG: i32 = 2;

const GROUND: Style = Style::bg(palette::GROUND);

/// A frame's corners: rounded, or square where the terminal's font may not have the arcs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    Rounded,
    Square,
}

impl Corners {
    /// Top left, top right, bottom left, bottom right.
    fn glyphs(self) -> [&'static str; 4] {
        match self {
            Corners::Rounded => ["╭", "╮", "╰", "╯"],
            Corners::Square => ["┌", "┐", "└", "┘"],
        }
    }
}

/// The frame around `rect`, with the ground painted inside it.
pub fn border(screen: &mut Screen, rect: Rect, line: &Style, corners: Corners) {
    screen.fill(rect, &GROUND);
    outline(screen, rect, line, corners);
}

/// The frame's line around `rect` alone, over what's drawn inside it: a body that ran onto its edges is cut there.
pub fn outline(screen: &mut Screen, rect: Rect, line: &Style, corners: Corners) {
    if rect.width < 2 || rect.height < 1 {
        return;
    }
    let [top_left, top_right, bottom_left, bottom_right] = corners.glyphs();
    let (right, bottom) = (rect.x + rect.width - 1, rect.y + rect.height - 1);
    let rule = "─".repeat((rect.width - 2) as usize);
    screen.put(rect.x, rect.y, &format!("{top_left}{rule}{top_right}"), line);
    for y in rect.y + 1..bottom {
        screen.put(rect.x, y, "│", line);
        screen.put(right, y, "│", line);
    }
    if bottom > rect.y {
        screen.put(rect.x, bottom, &format!("{bottom_left}{rule}{bottom_right}"), line);
    }
}

/// A rule across the frame around `rect` at row `y`, its ends joining the frame's sides.
pub fn divider(screen: &mut Screen, rect: Rect, y: i32, line: &Style) {
    if rect.width >= 2 {
        screen.put(rect.x, y, &format!("├{}┤", "─".repeat((rect.width - 2) as usize)), line);
    }
}

/// A rule across `rect`'s first row with no frame to join: what folds to one row (the drawer) sets its words into it
/// as a frame's edge has them.
pub fn rule(screen: &mut Screen, rect: Rect, line: &Style) {
    screen.fill(Rect { height: 1, ..rect }, &GROUND);
    screen.put(rect.x, rect.y, &"─".repeat(rect.width.max(0) as usize), line);
}

/// Words set into the edge of the frame around `rect` at row `y`, at its left (`╭─ words ─…`), ending in an ellipsis
/// where they'd come within `INSET` cells of its right corner. The column after the blank that closes them; with no
/// words, the column after the edge's first `─`.
pub fn left_words(screen: &mut Screen, rect: Rect, y: i32, words: &[Span]) -> i32 {
    let start = rect.x + INSET;
    if words.iter().all(|span| span.text.is_empty()) {
        return start - 1;
    }
    let end = rect.x + rect.width - INSET;
    screen.put(start - 1, y, " ", &GROUND);
    let mut column = start;
    for span in words {
        let room = end - column;
        if room <= 0 {
            break;
        }
        let text = if text_width(&span.text) > room { truncate(&span.text, room) } else { span.text.clone() };
        column = screen.put(column, y, &text, &span.style);
    }
    screen.put(column, y, " ", &GROUND);
    column + 1
}

/// Words set into the edge of the frame around `rect` at row `y`, at its right (`…─ words ─╮`), where they fit after
/// column `after`: the rule between them and what's at the left gives way first, down to the two blanks around it. The
/// column of the blank that opens them; None, and nothing drawn, where they don't fit.
pub fn right_words(screen: &mut Screen, rect: Rect, y: i32, words: &[Span], after: i32) -> Option<i32> {
    let width: i32 = words.iter().map(|span| text_width(&span.text)).sum();
    if width == 0 || width > right_room(rect, after) {
        return None;
    }
    let at = rect.x + rect.width - INSET - width;
    screen.put(at - 1, y, " ", &GROUND);
    let mut column = at;
    for span in words {
        column = screen.put(column, y, &span.text, &span.style);
    }
    screen.put(column, y, " ", &GROUND);
    Some(at - 1)
}

/// How wide words set into the edge of the frame around `rect` at its right can be, after column `after`.
pub fn right_room(rect: Rect, after: i32) -> i32 {
    rect.x + rect.width - INSET - after - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<Span> {
        vec![Span::styled(text, Style::default())]
    }

    #[test]
    fn words_in_an_edge_keep_a_rule_between_them_or_give_way() {
        let rect = Rect::new(0, 0, 20, 2);
        let mut screen = Screen::new(20, 2);
        border(&mut screen, rect, &LINE, Corners::Rounded);
        // A title too long for the edge ends in an ellipsis three cells from the corner, and leaves no room at its right.
        let after = left_words(&mut screen, rect, 0, &words("A long title, cut"));
        assert_eq!(screen.lines()[0], "╭─ A long title,… ─╮");
        assert_eq!(right_words(&mut screen, rect, 0, &words("x"), after), None);
        // With room, a rule between the two; short of it, the rule goes, and two blanks keep them apart.
        let mut screen = Screen::new(20, 2);
        border(&mut screen, rect, &LINE, Corners::Rounded);
        let after = left_words(&mut screen, rect, 0, &words("ab"));
        assert_eq!(right_words(&mut screen, rect, 0, &words("hint"), after), Some(12));
        assert_eq!(screen.lines()[0], format!("╭─ ab {} hint ─╮", "─".repeat(6)));
        assert_eq!(right_room(rect, after), 10);
        assert_eq!(right_words(&mut screen, rect, 0, &words(&"x".repeat(11)), after), None);
        assert_eq!(right_words(&mut screen, rect, 0, &words(&"y".repeat(10)), after), Some(6));
        assert_eq!(screen.lines()[0], format!("╭─ ab  {} ─╮", "y".repeat(10)));
        // With nothing at its left, the words at its right have the whole edge. Square corners where arcs may not show.
        let mut screen = Screen::new(20, 2);
        border(&mut screen, rect, &LINE, Corners::Square);
        let after = left_words(&mut screen, rect, 0, &[]);
        assert_eq!(right_words(&mut screen, rect, 0, &words(&"x".repeat(14)), after), Some(2));
        assert_eq!(screen.lines()[0], format!("┌─ {} ─┐", "x".repeat(14)));
        assert_eq!(screen.lines()[1], format!("└{}┘", "─".repeat(18)));
    }
}
