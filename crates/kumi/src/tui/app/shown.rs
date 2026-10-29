//! Changes' pictures on screen: in HISTORY when a row is opened, in NOW as a change lands, in the narrow dock. On a
//! terminal that shows images (Kitty's placeholders) each image is uploaded once, before the frame that first shows
//! it, and deleted once it hasn't been shown for a while.

use std::collections::HashMap;

use super::super::pictures::{self, kitty, Drawn, Graphics};
use super::super::style::palette;
use super::*;

/// Images still shown this recently are kept; others are deleted from the terminal.
const IMAGE_KEPT_MS: f64 = 5_000.;

/// The images uploaded to the terminal, by what they show ("<change id> <cols>x<rows>"), and what to write before
/// the next frame.
#[derive(Default)]
pub(super) struct Images {
    shown: HashMap<String, (u32, f64)>,
    /// Pictures drawn already, by the same key: NOW and the dock draw every frame, and a picture is drawn once.
    drawn: HashMap<String, Drawn>,
    next: u32,
    writes: String,
}

/// The most drawn pictures kept: past it, they're drawn again as needed.
const DRAWN_KEPT: usize = 64;

impl Images {
    /// A change's picture changed (its curve came after it): what was drawn of it, and its images, go.
    pub(super) fn forget(&mut self, change: &str) {
        let prefix = format!("{change} ");
        self.drawn.retain(|key, _| !key.starts_with(&prefix));
        let gone: Vec<String> = self.shown.keys().filter(|key| key.starts_with(&prefix)).cloned().collect();
        for key in gone {
            if let Some((id, _)) = self.shown.remove(&key) {
                self.writes.push_str(&kitty::delete(id));
            }
        }
    }

    /// The id the image for `key` shows under, uploading it first if it isn't up yet.
    pub(super) fn id(&mut self, key: &str, upload: impl FnOnce(u32) -> String) -> u32 {
        let now = perf_now();
        if let Some((id, seen)) = self.shown.get_mut(key) {
            *seen = now;
            return *id;
        }
        // Ids from 1, under 2^24: a placeholder's colour carries 24 bits.
        self.next = self.next % 0xff_fffe + 1;
        let id = self.next;
        self.writes.push_str(&upload(id));
        self.shown.insert(key.to_string(), (id, now));
        id
    }

    /// Shown again this frame (from rows kept between frames).
    fn touch(&mut self, key: &str) {
        if let Some((_, seen)) = self.shown.get_mut(key) {
            *seen = perf_now();
        }
    }

    /// What to write before this frame: uploads, and deletions of images not shown lately.
    pub(super) fn writes(&mut self) -> String {
        let now = perf_now();
        let stale: Vec<String> =
            self.shown.iter().filter(|(_, (_, seen))| now - seen > IMAGE_KEPT_MS).map(|(key, _)| key.clone()).collect();
        for key in stale {
            if let Some((id, _)) = self.shown.remove(&key) {
                self.writes.push_str(&kitty::delete(id));
            }
            self.drawn.remove(&key);
        }
        std::mem::take(&mut self.writes)
    }

    /// Every image still up, deleted: as Kumi closes.
    pub(super) fn clear(&mut self) -> String {
        let mut out = std::mem::take(&mut self.writes);
        for (id, _) in self.shown.drain().map(|(_, shown)| shown) {
            out.push_str(&kitty::delete(id));
        }
        out
    }
}

impl TuiApp {
    /// A change's picture at `cols` × `rows`, as this terminal shows it, and the key of the image it shows (on a
    /// terminal with graphics); None for a change without one.
    pub(super) fn change_drawn(&self, change: &ChangeRecord, cols: usize, rows: usize) -> Option<(Drawn, Option<String>)> {
        let picture = change.picture.as_ref()?;
        let name = format!("{} {cols}x{rows}", change.id);
        let image_key = |drawn: &Drawn| (self.0.graphics == Graphics::Kitty && !drawn.rows.is_empty()).then(|| name.clone());
        {
            let mut state = self.0.state.borrow_mut();
            if let Some(drawn) = state.images.drawn.get(&name).cloned() {
                state.images.touch(&name);
                return Some((drawn.clone(), image_key(&drawn)));
            }
        }
        let mut drawn = pictures::draw(picture, cols, rows, self.0.graphics, 0, palette::SURFACE);
        let mut state = self.0.state.borrow_mut();
        if let Some(image) = drawn.image.take() {
            let id = state.images.id(&name, |id| kitty::transmit(id, &image, cols, rows));
            drawn.rows = kitty::cells(id, cols, rows);
        }
        if state.images.drawn.len() >= DRAWN_KEPT {
            state.images.drawn.clear();
        }
        state.images.drawn.insert(name.clone(), drawn.clone());
        drop(state);
        let key = image_key(&drawn);
        Some((drawn, key))
    }

    /// Every picture drawn and image up goes, to be drawn and uploaded afresh as each is next shown: after the
    /// producer's editor (a terminal may drop images with the screen they were on) and in another theme (they were drawn
    /// in the last one's colours). HISTORY's rows hold their pictures' cells and an image's id, so they're made again too.
    pub(super) fn redraw_pictures(&self) {
        let gone = {
            let mut state = self.0.state.borrow_mut();
            state.history_rows = None;
            state.history_image = None;
            std::mem::take(&mut state.images).clear()
        };
        self.0.tty.write(&gone);
    }

    /// The image an open HISTORY row shows stays up while its rows are kept between frames.
    pub(super) fn touch_images(&self) {
        let mut state = self.0.state.borrow_mut();
        if let Some(key) = state.history_image.clone() {
            state.images.touch(&key);
        }
    }

    /// How tall a change's picture is in HISTORY: curves, notes and lanes take 6 rows; values and levels are lines.
    pub(super) fn picture_height(&self, change: &ChangeRecord) -> usize {
        match &change.picture {
            Some(picture) if self.0.graphics != Graphics::Words && pictures::scene_of(picture).is_some() => 6,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_goes_up_once_and_comes_down_once_it_isnt_shown() {
        let mut images = Images::default();
        let first = images.id("c1 40x6", |id| format!("<up {id}>"));
        let again = images.id("c1 40x6", |id| format!("<up {id} again>"));
        assert_eq!(first, again);
        assert_eq!(images.writes(), "<up 1>", "uploaded once");
        images.drawn.insert("c1 40x6".into(), Drawn::default());
        // Shown long ago: taken down, and drawn again next time.
        images.shown.get_mut("c1 40x6").unwrap().1 -= IMAGE_KEPT_MS + 1.;
        assert_eq!(images.writes(), kitty::delete(1));
        assert!(images.drawn.is_empty());
        assert_eq!(images.id("c1 40x6", |id| format!("<up {id}>")), 2);
        // A picture that changed loses its image; closing takes the rest down.
        images.forget("c1");
        assert!(images.writes().ends_with(&kitty::delete(2)));
        images.id("c2 40x6", |_| String::new());
        assert_eq!(images.clear(), kitty::delete(3));
    }
}
