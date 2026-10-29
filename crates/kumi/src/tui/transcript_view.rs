//! Where the conversation is scrolled to. At the end, the view follows what arrives. Scrolled up, an entry holds it:
//! that entry's row stays at the top while text streams in below, a notice comes in above, or the window's height
//! changes, and the top row keeps a request's first line in view while you read its answer.
//!
//! A frame lays out only the entries it shows, so it costs about a screen of rows however long the conversation is.

use std::collections::VecDeque;
use std::rc::Rc;

use super::transcript::{Entry, EntryRef, Row, Transcript};

/// One of a frame's rows: an entry's row, or the empty row before an entry.
pub enum Line {
    Gap,
    Row(Rc<[Row]>, usize),
}

impl Line {
    pub fn row(&self) -> Option<&Row> {
        match self {
            Line::Gap => None,
            Line::Row(rows, at) => rows.get(*at),
        }
    }
}

/// The top row while scrolled up: kept for a request, so the content doesn't move a row each time one passes the top.
pub enum Header {
    Blank,
    /// The request whose answer (or its steps and notices) is at the top, its own first line scrolled away: its rows as
    /// laid out, and where it is, to go back to it.
    Pinned {
        entry: EntryRef,
        index: usize,
        rows: Rc<[Row]>,
    },
}

/// A frame of the conversation, from the area's top.
pub struct Frame {
    /// Some while scrolled up.
    pub header: Option<Header>,
    pub lines: Vec<Line>,
    /// While scrolled up with more below: the entries that came in since the view left the end.
    pub new_below: Option<usize>,
    /// When the steps of an entry on screen next fold or move.
    pub next: Option<f64>,
}

#[derive(Clone)]
struct Anchor {
    entry: EntryRef,
    /// Where the entry was: looked at first, as entries come in before it or go.
    hint: usize,
    /// The row of the entry's block (its gap row, then its rows) at the content's top.
    row: usize,
    /// The width the block was laid out at, and its height there: at a new width the row keeps its place by proportion.
    width: i32,
    height: usize,
}

#[derive(Default)]
pub struct TranscriptView {
    /// None while following the end.
    anchor: Option<Anchor>,
    /// The newest entry when the view left the end: those after it are new below.
    seen: u64,
    /// The last frame's width and height.
    width: i32,
    height: usize,
}

/// Entry `index`'s block at `width`: its gap row (every entry's but the first) and its rows.
fn block(transcript: &mut Transcript, index: usize, width: i32, now: f64) -> (usize, Rc<[Row]>) {
    let rows = transcript.rows_of(index, width, now);
    (usize::from(index > 0) + rows.len(), rows)
}

fn line(index: usize, rows: &Rc<[Row]>, row: usize) -> Line {
    match row.checked_sub(usize::from(index > 0)) {
        Some(at) => Line::Row(Rc::clone(rows), at),
        None => Line::Gap,
    }
}

impl TranscriptView {
    pub fn is_following(&self) -> bool {
        self.anchor.is_none()
    }

    /// Back to the end, following it (ctrl+end, a message sent, a new conversation).
    pub fn follow(&mut self) {
        self.anchor = None;
    }

    /// The frame for an area `height` rows tall, its rows laid out at `width`.
    pub fn frame(&mut self, transcript: &mut Transcript, width: i32, height: usize, now: f64) -> Frame {
        (self.width, self.height) = (width, height);
        let mut frame = Frame { header: None, lines: vec![], new_below: None, next: None };
        let count = transcript.entries.len();
        if count == 0 || height == 0 {
            self.anchor = None;
            return frame;
        }
        let mut visited = vec![];
        if !self.anchored(transcript, &mut frame, &mut visited, now) {
            self.anchor = None;
            // The last rows that fit, from the last entry up.
            let mut lines = VecDeque::with_capacity(height);
            let mut index = count;
            while index > 0 && lines.len() < height {
                index -= 1;
                visited.push(index);
                let (len, rows) = block(transcript, index, width, now);
                for row in (0..len).rev().take(height - lines.len()) {
                    lines.push_front(line(index, &rows, row));
                }
            }
            frame.lines = lines.into();
        }
        frame.next = visited.into_iter().filter_map(|index| transcript.change_of(index, now)).reduce(f64::min);
        frame
    }

    /// The frame from the anchor down, under the header row; false where the rest of the conversation fits under it, as
    /// the view then follows the end again.
    fn anchored(&mut self, transcript: &mut Transcript, frame: &mut Frame, visited: &mut Vec<usize>, now: f64) -> bool {
        let Some(anchor) = self.anchor.clone() else { return false };
        let (width, room, count) = (self.width, self.height.saturating_sub(1), transcript.entries.len());
        // The anchor's entry, or where it was once it's gone: the next one takes its place.
        let (index, row) = match transcript.find(&anchor.entry, anchor.hint) {
            Some(index) if anchor.width != width => {
                let (len, _) = block(transcript, index, width, now);
                (index, anchor.row * len / anchor.height.max(1))
            }
            Some(index) => (index, anchor.row),
            None => (anchor.hint.min(count - 1), 0),
        };
        let (len, _) = block(transcript, index, width, now);
        let row = row.min(len.saturating_sub(1));
        let (mut at, mut at_row, mut more) = (index, row, false);
        'fill: while at < count {
            visited.push(at);
            let (len, rows) = block(transcript, at, width, now);
            while at_row < len {
                if frame.lines.len() == room {
                    more = true;
                    break 'fill;
                }
                frame.lines.push(line(at, &rows, at_row));
                at_row += 1;
            }
            (at, at_row) = (at + 1, 0);
        }
        if !more {
            frame.lines.clear();
            return false;
        }
        self.anchor = Some(Anchor { entry: Rc::clone(&transcript.entries[index]), hint: index, row, width, height: len });
        frame.header = Some(self.pinned(transcript, index, row, now));
        frame.new_below = Some(transcript.entries.iter().filter(|entry| entry.id > self.seen).count());
        true
    }

    /// The header for a top row at `row` of entry `index`'s block: the nearest request at or above it, if its first line
    /// has scrolled away.
    fn pinned(&self, transcript: &mut Transcript, index: usize, row: usize, now: f64) -> Header {
        let request = (0..=index).rev().find(|at| matches!(&*transcript.entries[*at].borrow(), Entry::User { .. }));
        let Some(at) = request else { return Header::Blank };
        if at == index && row <= usize::from(at > 0) {
            return Header::Blank;
        }
        Header::Pinned { entry: Rc::clone(&transcript.entries[at]), index: at, rows: transcript.rows_of(at, self.width, now) }
    }

    /// Up `rows` (down where it's less than 0), as the wheel and the page keys move it.
    pub fn scroll_by(&mut self, transcript: &mut Transcript, rows: i32, now: f64) {
        let held = self.anchor.as_ref().map(|anchor| (transcript.find(&anchor.entry, anchor.hint), anchor.row));
        let (index, row, rows) = match held {
            Some((Some(index), row)) => (index, row, rows),
            Some((None, _)) => return,
            // At the end, only up leaves it; the header row coming in above moves the content down one row of the way.
            None if rows > 0 => match self.end_top(transcript, now) {
                Some((index, row)) => {
                    self.leave_end(transcript);
                    (index, row, rows - 1)
                }
                None => return,
            },
            None => return,
        };
        let (index, row) = if rows >= 0 {
            self.up(transcript, index, row, rows.unsigned_abs() as usize, now)
        } else {
            match self.down(transcript, index, row, rows.unsigned_abs() as usize, now) {
                Some(at) => at,
                None => {
                    self.follow();
                    return;
                }
            }
        };
        self.anchor_at(transcript, index, row, now);
    }

    /// The first entry's first row at the top (ctrl+home).
    pub fn to_start(&mut self, transcript: &mut Transcript, now: f64) {
        if transcript.entries.is_empty() {
            return;
        }
        if self.anchor.is_none() {
            self.leave_end(transcript);
        }
        self.anchor_at(transcript, 0, 0, now);
    }

    /// `entry`'s first row at the top: a click on the pinned request, and what search's jumps use.
    pub fn scroll_to(&mut self, transcript: &mut Transcript, entry: &EntryRef, now: f64) {
        let Some(index) = transcript.find(entry, self.anchor.as_ref().map_or(0, |anchor| anchor.hint)) else { return };
        if self.anchor.is_none() {
            self.leave_end(transcript);
        }
        self.anchor_at(transcript, index, usize::from(index > 0), now);
    }

    /// The top row at the end, at the last frame's size: from the transcript as it is now, which may have grown since that
    /// frame was drawn.
    fn end_top(&self, transcript: &mut Transcript, now: f64) -> Option<(usize, usize)> {
        let (mut left, mut index, mut top) = (self.height, transcript.entries.len(), None);
        while index > 0 && left > 0 {
            index -= 1;
            let (len, _) = block(transcript, index, self.width, now);
            let taken = len.min(left);
            top = Some((index, len - taken));
            left -= taken;
        }
        top
    }

    fn leave_end(&mut self, transcript: &Transcript) {
        self.seen = transcript.entries.iter().map(|entry| entry.id).max().unwrap_or(0);
    }

    fn anchor_at(&mut self, transcript: &mut Transcript, index: usize, row: usize, now: f64) {
        let (height, _) = block(transcript, index, self.width, now);
        let entry = Rc::clone(&transcript.entries[index]);
        self.anchor = Some(Anchor { entry, hint: index, row, width: self.width, height });
    }

    /// `rows` up from row `row` of entry `index`'s block, stopping at the first entry's first row.
    fn up(&self, transcript: &mut Transcript, mut index: usize, mut row: usize, mut rows: usize, now: f64) -> (usize, usize) {
        while rows > row {
            if index == 0 {
                return (0, 0);
            }
            rows -= row + 1;
            index -= 1;
            row = block(transcript, index, self.width, now).0.saturating_sub(1);
        }
        (index, row - rows)
    }

    /// `rows` down from row `row` of entry `index`'s block; None past the conversation's last row.
    fn down(&self, transcript: &mut Transcript, mut index: usize, mut row: usize, mut rows: usize, now: f64) -> Option<(usize, usize)> {
        loop {
            let (len, _) = block(transcript, index, self.width, now);
            if row + rows < len {
                return Some((index, row + rows));
            }
            rows -= len - row;
            (index, row) = (index + 1, 0);
            if index == transcript.entries.len() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::transcript::NoticeTone;

    fn notices(count: usize) -> Transcript {
        let mut transcript = Transcript::default();
        for index in 0..count {
            transcript.add(Entry::Notice { text: format!("note {index}"), tone: NoticeTone::Info });
        }
        transcript
    }

    fn texts(frame: &Frame) -> Vec<String> {
        frame
            .lines
            .iter()
            .map(|line| line.row().map(|row| row.spans.iter().map(|span| span.text.as_str()).collect()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn at_the_end_the_last_rows_show_and_scrolling_up_holds_an_entry_across_gap_rows() {
        // Ten one-row notices: rows "note 0", gap, "note 1", gap, … 19 rows in all.
        let mut transcript = notices(10);
        let mut view = TranscriptView::default();
        let frame = view.frame(&mut transcript, 40, 5, 0.);
        assert_eq!(texts(&frame), ["note 7", "", "note 8", "", "note 9"]);
        assert!(frame.header.is_none() && frame.new_below.is_none());
        // One row up: the header row comes in, so the content moves down just one row.
        view.scroll_by(&mut transcript, 1, 0.);
        let frame = view.frame(&mut transcript, 40, 5, 0.);
        assert_eq!(texts(&frame), ["note 7", "", "note 8", ""]);
        assert_eq!(frame.new_below, Some(0));
        view.scroll_by(&mut transcript, 1, 0.);
        assert_eq!(texts(&view.frame(&mut transcript, 40, 5, 0.)), ["", "note 7", "", "note 8"]);
        // Up past the top stops at the first row; down to the end follows again.
        view.scroll_by(&mut transcript, 100, 0.);
        assert_eq!(texts(&view.frame(&mut transcript, 40, 5, 0.))[0], "note 0");
        view.scroll_by(&mut transcript, -100, 0.);
        assert!(view.frame(&mut transcript, 40, 5, 0.).header.is_none() && view.is_following());
    }

    #[test]
    fn an_anchor_keeps_its_entry_when_one_comes_in_before_it_or_it_goes() {
        let mut transcript = notices(10);
        let mut view = TranscriptView::default();
        view.frame(&mut transcript, 40, 5, 0.);
        view.scroll_by(&mut transcript, 5, 0.);
        let before = texts(&view.frame(&mut transcript, 40, 5, 0.));
        assert_eq!(before, ["note 5", "", "note 6", ""]);
        // A notice placed before an earlier entry shifts every index: the view holds note 5.
        let second = Rc::clone(&transcript.entries[1]);
        transcript.insert_before(Entry::Notice { text: "inserted".into(), tone: NoticeTone::Info }, Some(&second));
        assert_eq!(texts(&view.frame(&mut transcript, 40, 5, 0.)), before);
        // Its entry gone (note 5, now sixth), the next one takes its place.
        let held = Rc::clone(&transcript.entries[6]);
        transcript.remove(&held);
        assert_eq!(texts(&view.frame(&mut transcript, 40, 5, 0.)), ["", "note 6", "", "note 7"]);
    }

    #[test]
    fn a_frame_lays_out_only_the_entries_it_shows() {
        let mut transcript = notices(2_000);
        let mut view = TranscriptView::default();
        view.frame(&mut transcript, 40, 30, 0.);
        assert!(transcript.laid_out <= 16, "{} entries laid out", transcript.laid_out);
        let before = transcript.laid_out;
        view.scroll_by(&mut transcript, 600, 0.);
        view.frame(&mut transcript, 40, 30, 0.);
        assert!(transcript.laid_out - before <= 320, "{} entries laid out for 600 rows up", transcript.laid_out - before);
    }
}
