//! Kumi's wordmark: lowercase letters in half blocks over a dithered rule. The same pixels make the
//! README's logo (docs/assets/kumi-logo.svg).

/// The letters, four rows of half blocks.
pub const LOGO_LETTERS: [&str; 4] = [
    "██                          ▀▀",
    "██ ▄█▀  ██  ██  ██▀▀██▀▀█▄  ██",
    "██▀█▄   ██  ██  ██  ██  ██  ██",
    "▀▀  ▀▀   ▀▀▀▀▀  ▀▀  ▀▀  ▀▀  ▀▀",
];

/// The rule under the letters: solid in the middle, dithering out at both ends.
pub const LOGO_RULE: &str = "░▒▓████████████████████████▓▒░";

/// How wide the wordmark is, in cells.
pub const LOGO_WIDTH: i32 = 30;

/// Rows the whole wordmark takes: the letters, a gap and the rule.
pub const LOGO_HEIGHT: i32 = LOGO_LETTERS.len() as i32 + 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_width_is_the_widest_row() {
        let widest = LOGO_LETTERS.iter().map(|line| line.chars().count()).chain(std::iter::once(LOGO_RULE.chars().count())).max().unwrap();
        assert_eq!(LOGO_WIDTH, widest as i32);
        assert_eq!(LOGO_HEIGHT, 6);
    }
}
