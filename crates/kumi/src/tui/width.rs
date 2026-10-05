//! How many terminal cells text occupies, measured per grapheme (what a reader sees as one character).
//!
//! Widths follow `string-width` (the TypeScript's dependency), per grapheme cluster: a cluster of
//! nothing but default-ignorable characters, controls and marks takes no cell; an RGI emoji (a
//! character shown as emoji by default, one shown as text with U+FE0F after it, a keycap, a flag,
//! a skin tone on its base, a tag sequence or a ZWJ sequence of those) takes two; anything else
//! takes the East Asian Width of its first visible character (Wide and Fullwidth two, Ambiguous
//! one), plus that of any Halfwidth and Fullwidth Forms after it.

use std::sync::LazyLock;

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

use kumi_common::js;

pub fn graphemes(text: &str) -> Vec<&str> {
    text.graphemes(true).collect()
}

/// 0 for marks that attach to the previous character, 2 for wide CJK and emoji, else 1.
pub fn cell_width(grapheme: &str) -> i32 {
    let bytes = grapheme.as_bytes();
    if bytes.len() == 1 && (0x20..0x7f).contains(&bytes[0]) {
        return 1;
    }
    string_width(grapheme).min(2)
}

pub fn text_width(text: &str) -> i32 {
    text.graphemes(true).map(cell_width).sum()
}

/// At most `width` cells, ending in an ellipsis when something was cut.
pub fn truncate(text: &str, width: i32) -> String {
    truncate_with(text, width, "…")
}

/// `truncate` with an ellipsis of one's own.
pub fn truncate_with(text: &str, width: i32, ellipsis: &str) -> String {
    if width <= 0 {
        return String::new();
    }
    if text_width(text) <= width {
        return text.to_string();
    }
    let room = width - text_width(ellipsis);
    if room <= 0 {
        return js::string::head(ellipsis, width as usize);
    }
    let mut used = 0;
    let mut out = String::new();
    for grapheme in text.graphemes(true) {
        let cells = cell_width(grapheme);
        if used + cells > room {
            break;
        }
        out.push_str(grapheme);
        used += cells;
    }
    out + ellipsis
}

// Whole-cluster zero-width (surrogates can't occur in Rust strings).
static ZERO_WIDTH_CLUSTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:\p{Default_Ignorable_Code_Point}|\p{Cc}|\p{M})+$").unwrap());
// Pick the base scalar if the cluster starts with Prepend/Format/Marks.
static LEADING_NON_PRINTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{Default_Ignorable_Code_Point}\p{Cc}\p{Cf}\p{M}]+").unwrap());
static EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Emoji}$").unwrap());
static EMOJI_PRESENTATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Emoji_Presentation}$").unwrap());
static EMOJI_MODIFIER_BASE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Emoji_Modifier_Base}$").unwrap());

fn has(property: &Regex, character: char) -> bool {
    let mut buffer = [0u8; 4];
    property.is_match(character.encode_utf8(&mut buffer))
}

/// `string-width`'s count for `text`.
fn string_width(text: &str) -> i32 {
    let mut width = 0;
    for segment in text.graphemes(true) {
        // Zero-width / non-printing clusters
        if ZERO_WIDTH_CLUSTER.is_match(segment) {
            continue;
        }
        // Emoji width logic
        if rgi_emoji(segment) {
            width += 2;
            continue;
        }
        // Everything else: EAW of the cluster's first visible scalar
        let base = LEADING_NON_PRINTING.replace(segment, "");
        // TS: a cluster of only format characters has no base scalar and string-width throws; it is one cell here.
        width += base.chars().next().map_or(1, east_asian_width);
        // Add width for trailing Halfwidth and Fullwidth Forms (e.g., ﾞ, ﾟ, ｰ)
        for character in segment.chars().skip(1) {
            if ('\u{FF00}'..='\u{FFEF}').contains(&character) {
                width += east_asian_width(character);
            }
        }
    }
    width
}

/// 2 for East Asian Wide and Fullwidth characters, else 1 (Ambiguous is narrow).
fn east_asian_width(character: char) -> i32 {
    if is_wide(character) {
        2
    } else {
        1
    }
}

const fn is_regional_indicator(character: char) -> bool {
    matches!(character, '\u{1F1E6}'..='\u{1F1FF}')
}

const fn is_emoji_modifier(character: char) -> bool {
    matches!(character, '\u{1F3FB}'..='\u{1F3FF}')
}

const fn is_keycap_base(character: char) -> bool {
    matches!(character, '#' | '*' | '0'..='9')
}

/// Whether a grapheme cluster is an RGI emoji sequence (`\p{RGI_Emoji}`): a keycap, a flag, a tag
/// sequence, or emoji joined by ZWJ, each shown as emoji by default, as text with U+FE0F after it,
/// or as a skin tone on its base.
fn rgi_emoji(segment: &str) -> bool {
    let characters: Vec<char> = segment.chars().collect();
    match characters.as_slice() {
        [base, '\u{FE0F}', '\u{20E3}'] if is_keycap_base(*base) => return true,
        [first, second] if is_regional_indicator(*first) && is_regional_indicator(*second) => return true,
        ['\u{1F3F4}', tags @ .., '\u{E007F}'] if !tags.is_empty() && tags.iter().all(|tag| ('\u{E0020}'..='\u{E007E}').contains(tag)) => {
            return true
        }
        [] => return false,
        _ => {}
    }
    characters.split(|character| *character == '\u{200D}').all(|element| match element {
        [character] => has(&EMOJI_PRESENTATION, *character) && !is_regional_indicator(*character),
        [character, '\u{FE0F}'] => has(&EMOJI, *character) && !is_regional_indicator(*character) && !is_keycap_base(*character),
        [base, modifier] => has(&EMOJI_MODIFIER_BASE, *base) && is_emoji_modifier(*modifier),
        _ => false,
    })
}

fn is_wide(character: char) -> bool {
    let code = character as u32;
    if code < WIDE[0].0 {
        return false;
    }
    let index = WIDE.partition_point(|(start, _)| *start <= code);
    index > 0 && code <= WIDE[index - 1].1
}

/// East Asian Width Wide and Fullwidth, as `get-east-asian-width` (string-width's table) has them.
const WIDE: [(u32, u32); 126] = [
    (0x1100, 0x115F),
    (0x231A, 0x231B),
    (0x2329, 0x232A),
    (0x23E9, 0x23EC),
    (0x23F0, 0x23F0),
    (0x23F3, 0x23F3),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2630, 0x2637),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x268A, 0x268F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x2E80, 0x2E99),
    (0x2E9B, 0x2EF3),
    (0x2F00, 0x2FD5),
    (0x2FF0, 0x303E),
    (0x3041, 0x3096),
    (0x3099, 0x30FF),
    (0x3105, 0x312F),
    (0x3131, 0x318E),
    (0x3190, 0x31E5),
    (0x31EF, 0x321E),
    (0x3220, 0x3247),
    (0x3250, 0xA48C),
    (0xA490, 0xA4C6),
    (0xA960, 0xA97C),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE10, 0xFE19),
    (0xFE30, 0xFE52),
    (0xFE54, 0xFE66),
    (0xFE68, 0xFE6B),
    (0xFF01, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x16FE0, 0x16FE4),
    (0x16FF0, 0x16FF6),
    (0x17000, 0x18CDA),
    (0x18CFF, 0x18D20),
    (0x18D80, 0x18DF2),
    (0x18E00, 0x19191),
    (0x191A0, 0x191D2),
    (0x1AFF0, 0x1AFF3),
    (0x1AFF5, 0x1AFFB),
    (0x1AFFD, 0x1AFFE),
    (0x1B000, 0x1B128),
    (0x1B132, 0x1B132),
    (0x1B150, 0x1B152),
    (0x1B155, 0x1B155),
    (0x1B164, 0x1B168),
    (0x1B170, 0x1B2FB),
    (0x1D300, 0x1D356),
    (0x1D360, 0x1D376),
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F1AE, 0x1F1AE),
    (0x1F200, 0x1F202),
    (0x1F210, 0x1F23B),
    (0x1F240, 0x1F248),
    (0x1F250, 0x1F251),
    (0x1F260, 0x1F265),
    (0x1F300, 0x1F320),
    (0x1F32D, 0x1F335),
    (0x1F337, 0x1F37C),
    (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA),
    (0x1F3CF, 0x1F3D3),
    (0x1F3E0, 0x1F3F0),
    (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E),
    (0x1F440, 0x1F440),
    (0x1F442, 0x1F4FC),
    (0x1F4FF, 0x1F53D),
    (0x1F54B, 0x1F54E),
    (0x1F550, 0x1F567),
    (0x1F57A, 0x1F57A),
    (0x1F595, 0x1F596),
    (0x1F5A4, 0x1F5A4),
    (0x1F5FB, 0x1F64F),
    (0x1F680, 0x1F6C5),
    (0x1F6CC, 0x1F6CC),
    (0x1F6D0, 0x1F6D2),
    (0x1F6D5, 0x1F6D9),
    (0x1F6DC, 0x1F6DF),
    (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC),
    (0x1F7DA, 0x1F7DA),
    (0x1F7E0, 0x1F7EB),
    (0x1F7F0, 0x1F7F0),
    (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945),
    (0x1F947, 0x1F9FF),
    (0x1FA70, 0x1FA7C),
    (0x1FA80, 0x1FAC6),
    (0x1FAC8, 0x1FAC8),
    (0x1FACC, 0x1FADD),
    (0x1FADF, 0x1FAEB),
    (0x1FAEF, 0x1FAFA),
    (0x20000, 0x2FFFD),
    (0x30000, 0x3FFFD),
];
