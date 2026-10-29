//! Words cut to a length as Kumi counts it, in UTF-16 units (as JavaScript's `slice` counts them, and Kumi's limits
//! always have), at the edge of a character: an emoji, a flag or a letter with its accent goes whole or not at all,
//! never leaving half of itself (U+FFFD) or a joiner with nothing after it.
use unicode_segmentation::UnicodeSegmentation;

/// `text` cut to at most `units` UTF-16 units, between characters.
pub fn cut(text: &str, units: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for character in text.graphemes(true) {
        used += character.encode_utf16().count();
        if used > units {
            break;
        }
        end += character.len();
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn words_are_cut_between_characters() {
        assert_eq!(cut("🎹🎹", 3), "🎹");
        assert_eq!(cut("Bass", 60), "Bass");
        // An emoji, its joined form, a letter with its accent, a flag and an ideograph outside the Basic Multilingual
        // Plane go whole or not at all; ideographs inside it keep exactly the units asked for.
        let a = |n: usize| "a".repeat(n);
        assert_eq!(cut(&format!("{}🎹 Keys", a(59)), 60), a(59));
        assert_eq!(cut(&format!("{}👩‍🎤 Vox", a(57)), 60), a(57));
        assert_eq!(cut(&format!("{}e\u{301}x", a(59)), 60), a(59));
        assert_eq!(cut(&format!("{}🇯🇵", a(58)), 60), a(58));
        assert_eq!(cut(&format!("{}𠀋", a(59)), 60), a(59));
        assert_eq!(cut(&"漢".repeat(70), 60), "漢".repeat(60));
    }
}
