//! Kitty's graphics protocol with Unicode placeholders: an image is uploaded once under its id as a virtual
//! placement, and the screen shows it through ordinary cells (U+10EEEE with a row and a column mark, the id in the
//! foreground colour). Those cells go through the same diff as any other, so scrolling and clipping keep working.
//! Every upload asks for no reply (`q=2`): a reply would reach the input as keys.

use std::io::Write;

use base64::Engine;

use super::super::style::{Rgb, Style};
use super::super::wrap::Span;
use super::raster::Raster;

/// The character a placeholder cell shows.
pub const PLACEHOLDER: char = '\u{10EEEE}';
/// A transmission's base64 per escape, at most (the protocol's limit).
const CHUNK: usize = 4096;

/// Kitty's row and column marks (its rowcolumn-diacritics.txt): the nth is row or column n.
pub const DIACRITICS: [char; 297] = [
    '\u{0305}',
    '\u{030D}',
    '\u{030E}',
    '\u{0310}',
    '\u{0312}',
    '\u{033D}',
    '\u{033E}',
    '\u{033F}',
    '\u{0346}',
    '\u{034A}',
    '\u{034B}',
    '\u{034C}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035B}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036A}',
    '\u{036B}',
    '\u{036C}',
    '\u{036D}',
    '\u{036E}',
    '\u{036F}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059C}',
    '\u{059D}',
    '\u{059E}',
    '\u{059F}',
    '\u{05A0}',
    '\u{05A1}',
    '\u{05A8}',
    '\u{05A9}',
    '\u{05AB}',
    '\u{05AC}',
    '\u{05AF}',
    '\u{05C4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065A}',
    '\u{065B}',
    '\u{065D}',
    '\u{065E}',
    '\u{06D6}',
    '\u{06D7}',
    '\u{06D8}',
    '\u{06D9}',
    '\u{06DA}',
    '\u{06DB}',
    '\u{06DC}',
    '\u{06DF}',
    '\u{06E0}',
    '\u{06E1}',
    '\u{06E2}',
    '\u{06E4}',
    '\u{06E7}',
    '\u{06E8}',
    '\u{06EB}',
    '\u{06EC}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073A}',
    '\u{073D}',
    '\u{073F}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074A}',
    '\u{07EB}',
    '\u{07EC}',
    '\u{07ED}',
    '\u{07EE}',
    '\u{07EF}',
    '\u{07F0}',
    '\u{07F1}',
    '\u{07F3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081B}',
    '\u{081C}',
    '\u{081D}',
    '\u{081E}',
    '\u{081F}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082A}',
    '\u{082B}',
    '\u{082C}',
    '\u{082D}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0F82}',
    '\u{0F83}',
    '\u{0F86}',
    '\u{0F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

/// The colour a placeholder carries its image's id in (24 bits).
pub fn id_color(id: u32) -> Rgb {
    [(id >> 16) as u8, (id >> 8) as u8, id as u8]
}

/// One placeholder cell: the image's row and column it shows.
pub fn placeholder(row: usize, column: usize) -> String {
    let mut cell = String::from(PLACEHOLDER);
    cell.push(DIACRITICS[row.min(DIACRITICS.len() - 1)]);
    cell.push(DIACRITICS[column.min(DIACRITICS.len() - 1)]);
    cell
}

/// Whether a grapheme is a placeholder cell: U+10EEEE and nothing after it but row and column marks (combining marks,
/// so it's one cluster, one cell wide).
pub fn is_placeholder(grapheme: &str) -> bool {
    let mut characters = grapheme.chars();
    characters.next() == Some(PLACEHOLDER) && characters.all(|mark| DIACRITICS.binary_search(&mark).is_ok())
}

/// Uploads the image under `id` and places it virtually over `cols` × `rows` cells: RGBA, deflated, in chunks. Deflated
/// fast: the frame waits for it, and a map's image is megabytes of mostly clear pixels.
pub fn transmit(id: u32, raster: &Raster, cols: usize, rows: usize) -> String {
    let mut deflate = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = deflate.write_all(&raster.rgba);
    let data = base64::engine::general_purpose::STANDARD.encode(deflate.finish().unwrap_or_default());
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(CHUNK).collect();
    let mut out = String::with_capacity(data.len() + chunks.len() * 32 + 96);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let keys = if index == 0 {
            format!("a=T,U=1,i={id},f=32,s={},v={},c={cols},r={rows},o=z,q=2,m={more}", raster.width, raster.height)
        } else {
            format!("q=2,m={more}")
        };
        out.push_str(&format!("\u{1b}_G{keys};{}\u{1b}\\", std::str::from_utf8(chunk).unwrap_or("")));
    }
    out
}

/// Deletes every image Kumi put up (ids 1 to 2^24 − 2, as Kumi gives them) and their placements.
pub const DELETE_ALL: &str = "\u{1b}_Ga=d,d=R,x=1,y=16777214,q=2\u{1b}\\";

/// Deletes the image under `id` and its placements.
pub fn delete(id: u32) -> String {
    format!("\u{1b}_Ga=d,d=I,i={id},q=2\u{1b}\\")
}

/// The cells that show the image under `id`: `rows` of `cols` placeholders in its colour.
pub fn cells(id: u32, cols: usize, rows: usize) -> Vec<Vec<Span>> {
    let style = Style::fg(id_color(id));
    (0..rows).map(|row| (0..cols).map(|column| Span::styled(placeholder(row, column), style)).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::width::cell_width;

    #[test]
    fn an_upload_is_chunked_quiet_and_placed_virtually() {
        // Noise compresses badly: several chunks.
        let mut raster = Raster::new(64, 64);
        let mut seed = 1u32;
        for byte in raster.rgba.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *byte = (seed >> 16) as u8;
        }
        let out = transmit(42, &raster, 10, 4);
        let escapes: Vec<&str> = out.split("\u{1b}\\").filter(|s| !s.is_empty()).collect();
        assert!(escapes.len() > 2, "{} chunks", escapes.len());
        assert!(escapes[0].starts_with("\u{1b}_Ga=T,U=1,i=42,f=32,s=64,v=64,c=10,r=4,o=z,q=2,m=1;"));
        for middle in &escapes[1..escapes.len() - 1] {
            assert!(middle.starts_with("\u{1b}_Gq=2,m=1;"));
        }
        assert!(escapes.last().unwrap().starts_with("\u{1b}_Gq=2,m=0;"));
        for escape in &escapes {
            let payload = escape.split_once(';').unwrap().1;
            assert!(payload.len() <= CHUNK && payload.len() % 4 == 0, "{}", payload.len());
        }
        assert_eq!(delete(42), "\u{1b}_Ga=d,d=I,i=42,q=2\u{1b}\\");
    }

    #[test]
    fn a_placeholder_is_one_cell_carrying_its_row_column_and_image() {
        let cell = placeholder(2, 5);
        assert_eq!(cell.chars().collect::<Vec<_>>(), [PLACEHOLDER, DIACRITICS[2], DIACRITICS[5]]);
        assert_eq!(cell_width(&cell), 1);
        assert_eq!(DIACRITICS[0], '\u{0305}');
        assert_eq!(id_color(0x01_02_03), [1, 2, 3]);
        let rows = cells(7, 3, 2);
        assert_eq!((rows.len(), rows[0].len()), (2, 3));
        assert_eq!(rows[1][2].text, placeholder(1, 2));
        assert_eq!(rows[0][0].style.fg, Some([0, 0, 7]));
    }
}
