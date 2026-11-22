//! Text from prepared character bitmaps.
//!
//! A `Font` holds every character as a small picture with one byte per
//! pixel that says how much of the pixel the character covers. Drawing a
//! character means blending the wanted colour into the frame in that
//! proportion, which gives smooth edges and is exactly what the chip's
//! blitter does by itself (`services::canvas`). The bitmaps are made on
//! the PC by `tools/mkfont.py`.
//!
//! This replaces working every character out pixel by pixel from the font
//! data each time it is drawn, which measured as the slowest kind of
//! drawing there was.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

/// A monospaced font: every character is `width` x `height` pixels.
pub struct Font {
    pub width: u8,
    pub height: u8,
    /// The first character the font has, and how many follow in order.
    pub first: u8,
    pub count: u8,
    /// For each character, row by row, how much of each pixel it covers.
    pub coverage: &'static [u8],
}

impl Font {
    /// The coverage bitmap of `character`, or `None` if the font lacks it.
    pub fn glyph(&self, character: u8) -> Option<&'static [u8]> {
        let index = character.checked_sub(self.first)? as usize;
        let size = self.width as usize * self.height as usize;
        (index < self.count as usize).then(|| &self.coverage[index * size..][..size])
    }

    /// The rectangle `count` characters starting at (x, y) cover.
    pub fn rect(&self, x: i32, y: i32, count: usize) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(self.width as u32 * count as u32, self.height as u32))
    }
}

/// Something that can be drawn on with `embedded-graphics` and with text
/// from a `Font`.
pub trait Paint: DrawTarget<Color = Rgb565> {
    /// Draw one character with its top left corner at (x, y), inside
    /// `clip` only. The colour is blended over what is there.
    ///
    /// This version works on a black background without reading it: each
    /// pixel gets the colour scaled by the coverage. It is what the
    /// preview on the PC uses; the watch's `Canvas` has the blitter do the
    /// blending.
    fn glyph(&mut self, clip: &Rectangle, x: i32, y: i32, font: &Font, character: u8, color: Rgb565) {
        let Some(coverage) = font.glyph(character) else {
            return;
        };
        let area = font.rect(x, y, 1).intersection(clip);
        for row in area.rows() {
            for column in area.columns() {
                let c = coverage[(row - y) as usize * font.width as usize + (column - x) as usize] as u32;
                if c != 0 {
                    let part = |channel: u8| ((channel as u32 * c + 127) / 255) as u8;
                    let shade = Rgb565::new(part(color.r()), part(color.g()), part(color.b()));
                    let _ = self.fill_solid(&Rectangle::new(Point::new(column, row), Size::new(1, 1)), shade);
                }
            }
        }
    }

    /// Draw `text` with its top left corner at (x, y), inside `clip` only.
    fn text(&mut self, clip: &Rectangle, x: i32, y: i32, font: &Font, text: &str, color: Rgb565) {
        for (n, character) in text.bytes().enumerate() {
            let at = x + n as i32 * font.width as i32;
            // Characters wholly outside the clip cost nothing.
            if !font.rect(at, y, 1).intersection(clip).is_zero_sized() && character != b' ' {
                self.glyph(clip, at, y, font, character, color);
            }
        }
    }
}
