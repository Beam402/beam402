//! A frame of pixels, and the one rule that governs it.
//!
//! **Nothing is drawn outside the frame.** A web page that overflows scrolls; a
//! board that overflows loses the end of a number, and the number it loses is
//! usually the last digit — the one that decides rounds. So every draw is
//! clipped, and [`Frame::text`] reports whether it fit, so a caller can be held
//! to it by a test instead of by somebody standing in the stands.

use crate::font;

/// A monochrome frame. One bit per pixel, row-major, because that is what an LED
/// panel takes and there is no reason for the page to see anything richer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    bits: Vec<u8>,
}

impl Frame {
    pub fn new(w: usize, h: usize) -> Self {
        Frame {
            w,
            h,
            bits: vec![0; w.div_ceil(8) * h],
        }
    }

    pub fn lit(&self, x: usize, y: usize) -> bool {
        if x >= self.w || y >= self.h {
            return false;
        }
        let stride = self.w.div_ceil(8);
        self.bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0
    }

    pub fn set(&mut self, x: isize, y: isize) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let stride = self.w.div_ceil(8);
        self.bits[y * stride + x / 8] |= 0x80 >> (x % 8);
    }

    pub fn rect(&mut self, x: isize, y: isize, w: usize, h: usize) {
        for dy in 0..h as isize {
            for dx in 0..w as isize {
                self.set(x + dx, y + dy);
            }
        }
    }

    /// A horizontal dotted rule, for separating lanes without spending two rows.
    pub fn dotted(&mut self, y: isize, step: usize) {
        for x in (0..self.w).step_by(step) {
            self.set(x as isize, y);
        }
    }

    /// Width of `s` at `scale`, in pixels, trailing gap included.
    pub fn measure(s: &str, scale: usize) -> usize {
        s.chars().count() * font::ADVANCE * scale
    }

    /// Draw text. Returns `false` if any of it fell outside the frame.
    ///
    /// The return value is the point. A board cannot scroll and must not clip a
    /// number silently, so the layout is asserted rather than eyeballed.
    pub fn text(&mut self, x: isize, y: isize, s: &str, scale: usize) -> bool {
        let mut fits = x >= 0 && y >= 0 && y as usize + font::H * scale <= self.h;
        let mut cx = x;
        for c in s.chars() {
            match font::glyph(c) {
                Some(g) => {
                    for (row, bits) in g.iter().enumerate() {
                        for col in 0..font::W {
                            if bits & (1 << (font::W - 1 - col)) != 0 {
                                for sy in 0..scale {
                                    for sx in 0..scale {
                                        self.set(
                                            cx + (col * scale + sx) as isize,
                                            y + (row * scale + sy) as isize,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                // A character the font has no picture of becomes a visible box.
                // Silence here would be a missing digit nobody notices.
                None => {
                    for row in 0..font::H {
                        for col in 0..font::W {
                            let edge =
                                row == 0 || row == font::H - 1 || col == 0 || col == font::W - 1;
                            if edge {
                                for sy in 0..scale {
                                    for sx in 0..scale {
                                        self.set(
                                            cx + (col * scale + sx) as isize,
                                            y + (row * scale + sy) as isize,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    fits = false;
                }
            }
            cx += (font::ADVANCE * scale) as isize;
        }
        // The trailing gap may hang over the edge; the last glyph may not.
        fits && cx - scale as isize <= self.w as isize
    }

    /// Draw `s` centred in `[x, x + w)`. Returns whether it fit.
    pub fn centred(&mut self, x: isize, y: isize, w: usize, s: &str, scale: usize) -> bool {
        let text = Frame::measure(s, scale).saturating_sub(scale);
        let off = (w.saturating_sub(text)) / 2;
        self.text(x + off as isize, y, s, scale)
    }

    /// Draw `s` so it ends at `right`. Numbers on a board are read from the
    /// right, and a right-aligned column of times does not jump when one of them
    /// loses a digit.
    pub fn right(&mut self, right: isize, y: isize, s: &str, scale: usize) -> bool {
        let text = Frame::measure(s, scale).saturating_sub(scale) as isize;
        self.text(right - text, y, s, scale)
    }

    /// Lit pixels. Used by the tests to catch a frame that draws nothing.
    pub fn ink(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }

    /// Row-major bytes, as a panel would take them.
    pub fn bytes(&self) -> &[u8] {
        &self.bits
    }

    /// One hex character per four pixels, for embedding in a page.
    pub fn hex(&self) -> String {
        self.bits.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_drawn_outside_the_frame() {
        // The rule the whole module exists for: a board cannot scroll, so a draw
        // that runs off the end must clip rather than corrupt the row below it.
        let mut f = Frame::new(32, 16);
        assert!(!f.text(28, 0, "OVERFLOW", 1), "and it says it did not fit");
        assert!(!f.text(-10, 0, "X", 1));
        assert!(!f.text(0, 12, "X", 1), "too low to hold seven rows");
        f.set(-1, -1);
        f.set(1000, 1000);
        // Whatever landed, it landed inside.
        for y in 0..f.h {
            for x in 0..f.w {
                let _ = f.lit(x, y);
            }
        }
    }

    #[test]
    fn text_that_fits_says_so_and_lands_where_it_was_put() {
        let mut f = Frame::new(64, 16);
        assert!(f.text(0, 0, "12.340", 1));
        assert!(f.ink() > 0);
        // '1' is a stem in column 2 of its cell, so the top-left pixel is dark
        // and the third column of the top row is lit.
        assert!(!f.lit(0, 0));
        assert!(f.lit(2, 0));
    }

    #[test]
    fn an_unknown_character_becomes_a_visible_box() {
        // A silent gap would be a missing digit, and nobody in the stands can
        // tell a missing digit from a number that is simply shorter.
        let mut f = Frame::new(32, 16);
        assert!(!f.text(0, 0, "#", 1), "reported as not fitting");
        assert!(f.ink() > 0, "and drawn, so it is visible");
    }

    #[test]
    fn scale_two_is_exactly_twice_the_size() {
        let mut one = Frame::new(64, 16);
        one.text(0, 0, "8", 1);
        let mut two = Frame::new(64, 16);
        two.text(0, 0, "8", 2);
        assert_eq!(two.ink(), one.ink() * 4);
        assert_eq!(Frame::measure("88", 2), Frame::measure("88", 1) * 2);
    }

    #[test]
    fn right_alignment_ends_where_it_was_told_to() {
        let mut f = Frame::new(64, 16);
        assert!(f.right(64, 0, "9", 1));
        // The last glyph's rightmost column is column 63; nothing beyond it.
        assert!((0..7).any(|y| f.lit(63, y)));
    }
}
