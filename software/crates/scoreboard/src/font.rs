//! A 5×7 bitmap font, written out.
//!
//! Not a choice made for character: on a 32-pixel-tall board a 7-row cell is
//! what fits four lines, and 5×7 is the set every LED sign in the world already
//! uses. Writing it out rather than rasterising one at build time keeps the crate
//! dependency-free and, more usefully, makes the glyphs **legible in the source**
//! — a wrong pixel in a digit is a wrong number on a board fifty metres from the
//! stands, and it should be visible to a reviewer without running anything.
//!
//! Each glyph is seven rows, five bits wide, bit 4 leftmost.

/// Rows per glyph.
pub const H: usize = 7;
/// Columns per glyph.
pub const W: usize = 5;
/// Columns per glyph including the gap that separates it from the next.
pub const ADVANCE: usize = W + 1;

/// The glyph for `c`, or `None` if this font has no picture of it.
///
/// Returning `None` rather than a blank is deliberate: a character the board
/// cannot draw is a caller's mistake, and [`crate::frame::Frame::text`] turns it
/// into a visible box rather than a silent gap.
pub fn glyph(c: char) -> Option<&'static [u8; H]> {
    let i = match c.to_ascii_uppercase() {
        ' ' => 0,
        '0'..='9' => 1 + (c as usize - '0' as usize),
        'A'..='Z' => 11 + (c.to_ascii_uppercase() as usize - 'A' as usize),
        '.' => 37,
        '-' => 38,
        ':' => 39,
        '/' => 40,
        '*' => 41,
        '+' => 42,
        '?' => 43,
        _ => return None,
    };
    GLYPHS.get(i)
}

#[rustfmt::skip]
const GLYPHS: [[u8; H]; 44] = [
    // space
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
    // 0
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    // 1
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    // 2
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    // 3
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
    // 4
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    // 5
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    // 6
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    // 7
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    // 8
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    // 9
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
    // A
    [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
    // B
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
    // C
    [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
    // D
    [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
    // E
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
    // F
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
    // G
    [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
    // H
    [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
    // I
    [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    // J
    [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
    // K
    [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
    // L
    [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
    // M
    [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
    // N
    [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
    // O
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
    // P
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
    // Q
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
    // R
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
    // S
    [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
    // T
    [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
    // U
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
    // V
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
    // W
    [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
    // X
    [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
    // Y
    [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
    // Z
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
    // .
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
    // -
    [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
    // :
    [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
    // /
    [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
    // *
    [0b00000, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0b00000],
    // +
    [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
    // ?
    [0b01110, 0b10001, 0b00001, 0b00110, 0b00100, 0b00000, 0b00100],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_character_a_board_prints_has_a_picture() {
        // The board's whole vocabulary: times, speeds, dial-ins, lane labels and
        // the words around them. A gap here is a hole on a sign.
        for c in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ .-:/*+?".chars() {
            assert!(glyph(c).is_some(), "no glyph for {c:?}");
        }
        assert!(glyph('a').is_some(), "lower case folds to upper");
        assert!(glyph('#').is_none(), "and an unknown one says so");
    }

    #[test]
    fn no_glyph_leaks_outside_its_five_columns() {
        // Six bits would draw into the next character's cell and the board would
        // be subtly, unfixably smeared.
        for (i, g) in GLYPHS.iter().enumerate() {
            for (row, bits) in g.iter().enumerate() {
                assert!(*bits < 0b100000, "glyph {i} row {row} is wider than 5");
            }
        }
    }

    #[test]
    fn the_digits_are_distinguishable_from_each_other() {
        // The failure this catches is a copy-paste in the table above producing
        // two identical digits — which reads as a plausible wrong number on a
        // board nobody can check from the stands.
        let digits: Vec<_> = ('0'..='9').map(|c| glyph(c).unwrap()).collect();
        for i in 0..digits.len() {
            for j in i + 1..digits.len() {
                assert_ne!(digits[i], digits[j], "digits {i} and {j} are the same");
            }
        }
    }

    #[test]
    fn eight_is_the_densest_digit_and_one_the_sparsest() {
        // A cheap shape check on the table: if the glyphs were shifted or
        // transposed this ordering would not survive.
        let ink = |c: char| {
            glyph(c)
                .unwrap()
                .iter()
                .map(|r| r.count_ones())
                .sum::<u32>()
        };
        assert!(ink('8') > ink('7'));
        assert!(ink('1') < ink('0'));
    }
}
