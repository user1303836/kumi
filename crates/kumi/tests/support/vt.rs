//! A minimal terminal, enough of xterm to replay Kumi's renderer output in tests: cursor moves, colours, erasing in
//! the colours set, a scrolling region and links.

use kumi::tui::style::{link_id, style_key, Rgb, Style};
use kumi::tui::width::{cell_width, graphemes};
use kumi_common::js;

pub struct VirtualTerminal {
    pub width: i32,
    pub height: i32,
    pub chars: Vec<Vec<String>>,
    pub styles: Vec<Vec<String>>,
    pub x: i32,
    pub y: i32,
    pub cursor_visible: bool,
    style: Style,
    /// The scrolling region's rows, from `top` up to `bottom`.
    top: i32,
    bottom: i32,
}

impl VirtualTerminal {
    pub fn new(width: i32, height: i32) -> VirtualTerminal {
        VirtualTerminal {
            width,
            height,
            chars: vec![vec![" ".to_string(); width as usize]; height as usize],
            styles: vec![vec![style_key(&Style::default()); width as usize]; height as usize],
            x: 0,
            y: 0,
            cursor_visible: true,
            style: Style::default(),
            top: 0,
            bottom: height,
        }
    }

    pub fn write(&mut self, data: &str) {
        let bytes = data.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'[') {
                let mut end = index + 2;
                while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                    end += 1;
                }
                let final_byte = bytes.get(end).map_or('\0', |byte| *byte as char);
                self.csi(&data[index + 2..end], final_byte);
                index = end + 1;
                continue;
            }
            if bytes[index] == 0x1b {
                // OSC (clipboard, titles) ends at BEL or ESC \, and APC (Kitty's images) and DCS at ESC \; they draw
                // nothing. Other escapes are two characters.
                if matches!(bytes.get(index + 1), Some(b'_' | b'P')) {
                    let st = data[index..].find("\u{1b}\\").map(|at| index + at + 2);
                    index = st.unwrap_or(bytes.len()).min(bytes.len());
                } else if bytes.get(index + 1) == Some(&b']') {
                    let bell = data[index..].find('\u{7}').map(|at| (index + at, index + at + 1));
                    let st = data[index..].find("\u{1b}\\").map(|at| (index + at, index + at + 2));
                    let (end, next) = [bell, st].into_iter().flatten().min().unwrap_or((bytes.len(), bytes.len()));
                    // A link (OSC 8): `8;params;address`, an empty address closing it.
                    if let Some(link) = data[index + 2..end].strip_prefix("8;") {
                        let address = link.split_once(';').map_or("", |(_, address)| address);
                        self.style.link = if address.is_empty() { 0 } else { link_id(address) };
                    }
                    index = next.min(bytes.len());
                } else {
                    index += 1 + data[index + 1..].chars().next().map_or(1, char::len_utf8);
                }
                continue;
            }
            let mut end = index;
            while end < bytes.len() && bytes[end] != 0x1b {
                end += 1;
            }
            for grapheme in graphemes(&data[index..end]) {
                self.print(grapheme);
            }
            index = end;
        }
    }

    #[allow(dead_code)]
    pub fn lines(&self) -> Vec<String> {
        self.chars.iter().map(|row| row.concat()).collect()
    }

    fn csi(&mut self, params: &str, final_byte: char) {
        if params.starts_with('?') {
            if params == "?25" {
                self.cursor_visible = final_byte == 'h';
            }
            return;
        }
        if final_byte == 'H' {
            let mut parts = params.split(';');
            let row = parts.next().unwrap_or("1");
            let column = parts.next().unwrap_or("1");
            self.y = js::number::parse(row).unwrap_or(f64::NAN) as i32 - 1;
            self.x = js::number::parse(column).unwrap_or(f64::NAN) as i32 - 1;
        } else if final_byte == 'J' && params == "2" {
            for y in 0..self.height {
                self.erase(y, 0, self.width);
            }
        } else if final_byte == 'K' {
            let (x, width) = (self.x.min(self.width - 1), self.width);
            match params {
                "1" => self.erase(self.y, 0, x + 1),
                "2" => self.erase(self.y, 0, width),
                _ => self.erase(self.y, x, width),
            }
        } else if final_byte == 'X' {
            let count = js::number::parse(params).map_or(1, |count| (count as i32).max(1));
            let x = self.x.min(self.width - 1);
            self.erase(self.y, x, (x + count).min(self.width));
        } else if final_byte == 'r' {
            let mut parts = params.split(';').map(|part| js::number::parse(part).map(|n| n as i32));
            let top = parts.next().flatten().unwrap_or(1).max(1);
            let bottom = parts.next().flatten().unwrap_or(self.height).min(self.height);
            if top < bottom {
                (self.top, self.bottom) = (top - 1, bottom);
                (self.x, self.y) = (0, 0);
            }
        } else if final_byte == 'S' {
            let count = js::number::parse(params).map_or(1, |count| (count as i32).max(1));
            for _ in 0..count.min(self.bottom - self.top) {
                let (top, bottom) = (self.top as usize, self.bottom as usize);
                self.chars[top..bottom].rotate_left(1);
                self.styles[top..bottom].rotate_left(1);
                self.erase(self.bottom - 1, 0, self.width);
            }
        } else if final_byte == 'm' {
            self.sgr(params);
        }
    }

    /// Cells `from` to `to` of row `y` blank, in the background set (as xterm erases). A wide character cut by either
    /// end goes blank whole.
    fn erase(&mut self, y: i32, from: i32, to: i32) {
        if y < 0 || y >= self.height || from >= to {
            return;
        }
        let blank = style_key(&self.style.bg.map_or_else(Style::default, Style::bg));
        let row = y as usize;
        let from = if from > 0 && self.chars[row][from as usize].is_empty() { from - 1 } else { from };
        let to = if to < self.width && self.chars[row][to as usize].is_empty() { to + 1 } else { to };
        for x in from as usize..to as usize {
            self.chars[row][x] = " ".to_string();
            self.styles[row][x] = blank.clone();
        }
    }

    fn sgr(&mut self, params: &str) {
        let codes: Vec<Option<f64>> = if params.is_empty() { vec![Some(0.0)] } else { params.split(';').map(js::number::parse).collect() };
        let at = |index: usize| codes.get(index).copied().flatten();
        let mut index = 0;
        while index < codes.len() {
            let code = at(index);
            if code == Some(0.0) {
                // A link stays open through a reset of colours, as it does in a terminal.
                self.style = Style { link: self.style.link, ..Style::default() };
            } else if code == Some(1.0) {
                self.style.bold = true;
            } else if code == Some(2.0) {
                self.style.dim = true;
            } else if code == Some(3.0) {
                self.style.italic = true;
            } else if code == Some(4.0) {
                self.style.underline = true;
            } else if code == Some(7.0) {
                self.style.inverse = true;
            } else if (code == Some(38.0) || code == Some(48.0)) && at(index + 1) == Some(2.0) {
                let channel = |offset: usize| at(index + offset).unwrap_or(0.0) as u8;
                let rgb: Rgb = [channel(2), channel(3), channel(4)];
                if code == Some(38.0) {
                    self.style.fg = Some(rgb);
                } else {
                    self.style.bg = Some(rgb);
                }
                index += 4;
            } else if (code == Some(38.0) || code == Some(48.0)) && at(index + 1) == Some(5.0) {
                index += 2;
            }
            index += 1;
        }
    }

    fn print(&mut self, grapheme: &str) {
        let cells = cell_width(grapheme);
        if cells == 0 || self.y >= self.height {
            return;
        }
        if self.x >= self.width {
            self.x = self.width - 1; // autowrap is off
        }
        let (x, y) = (self.x as usize, self.y as usize);
        let key = style_key(&self.style);
        let row = &mut self.chars[y];
        // Like a real terminal, overwriting half of a wide character blanks the other half.
        if row[x].is_empty() && x > 0 {
            row[x - 1] = " ".to_string();
        }
        if cells == 1 && row.get(x + 1).is_some_and(String::is_empty) {
            row[x + 1] = " ".to_string();
        }
        row[x] = grapheme.to_string();
        self.styles[y][x] = key.clone();
        if cells == 2 && (x as i32) + 1 < self.width {
            if row.get(x + 2).is_some_and(String::is_empty) {
                row[x + 2] = " ".to_string();
            }
            row[x + 1] = String::new();
            self.styles[y][x + 1] = key;
        }
        self.x += cells;
    }
}
