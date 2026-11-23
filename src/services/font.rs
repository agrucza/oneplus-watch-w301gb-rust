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

/// A font: every character in a cell of `width` x `height` pixels.
pub struct Font {
    pub width: u8,
    pub height: u8,
    /// The first character the font has, and how many follow in order.
    pub first: u8,
    pub count: u8,
    /// For each character, row by row, how much of each pixel it covers.
    pub coverage: &'static [u8],
    /// How far each character moves the place of the next one. Empty for
    /// a monospaced font, where every character moves it by `width`.
    pub advance: &'static [u8],
}

impl Font {
    /// The coverage bitmap of `character`, or `None` if the font lacks it.
    pub fn glyph(&self, character: u8) -> Option<&'static [u8]> {
        let index = character.checked_sub(self.first)? as usize;
        let size = self.width as usize * self.height as usize;
        (index < self.count as usize).then(|| &self.coverage[index * size..][..size])
    }

    /// How far `character` moves the place of the next one.
    pub fn advance(&self, character: u8) -> i32 {
        let index = character.wrapping_sub(self.first) as usize;
        self.advance.get(index).copied().unwrap_or(self.width) as i32
    }

    /// How wide `text` comes out.
    pub fn measure(&self, text: &str) -> i32 {
        text.bytes().map(|character| self.advance(character)).sum()
    }

    /// The rectangle `count` characters of a monospaced font starting at
    /// (x, y) cover.
    pub fn rect(&self, x: i32, y: i32, count: usize) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(self.width as u32 * count as u32, self.height as u32))
    }
}

/// A drawing: how much of each pixel it covers, row by row, to be put
/// down in any colour.
pub struct Art {
    pub width: u16,
    pub height: u16,
    pub coverage: &'static [u8],
}

impl Art {
    /// The rectangle the drawing covers with its top left corner at (x, y).
    pub fn rect(&self, x: i32, y: i32) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(self.width as u32, self.height as u32))
    }
}

/// Something that can be drawn on with `embedded-graphics` and with
/// coverage bitmaps: text from a `Font` and drawings (`Art`).
pub trait Paint: DrawTarget<Color = Rgb565> {
    /// Put a coverage bitmap of `width` x `height` down with its top left
    /// corner at (x, y), inside `clip` only: the colour is blended over
    /// what is there, in proportion to the coverage.
    ///
    /// This version works on a black background without reading it: each
    /// pixel gets the colour scaled by the coverage. The watch's `Canvas`
    /// has the blitter do the blending, over whatever is there.
    #[allow(clippy::too_many_arguments)]
    fn coverage(&mut self, clip: &Rectangle, x: i32, y: i32, width: u16, height: u16, coverage: &[u8], color: Rgb565) {
        let area = Rectangle::new(Point::new(x, y), Size::new(width as u32, height as u32)).intersection(clip);
        coverage_on_black(self, &area, x, y, width, coverage, color);
    }

    /// Draw one character with its top left corner at (x, y), inside
    /// `clip` only.
    fn glyph(&mut self, clip: &Rectangle, x: i32, y: i32, font: &Font, character: u8, color: Rgb565) {
        if let Some(coverage) = font.glyph(character) {
            self.coverage(clip, x, y, font.width as u16, font.height as u16, coverage, color);
        }
    }

    /// Draw a drawing with its top left corner at (x, y), inside `clip` only.
    fn art(&mut self, clip: &Rectangle, x: i32, y: i32, art: &Art, color: Rgb565) {
        self.coverage(clip, x, y, art.width, art.height, art.coverage, color);
    }

    /// Draw `text` with its top left corner at (x, y), inside `clip` only.
    fn text(&mut self, clip: &Rectangle, x: i32, y: i32, font: &Font, text: &str, color: Rgb565) {
        let mut at = x;
        for character in text.bytes() {
            // Characters wholly outside the clip cost nothing.
            if !font.rect(at, y, 1).intersection(clip).is_zero_sized() && character != b' ' {
                self.glyph(clip, at, y, font, character, color);
            }
            at += font.advance(character);
        }
    }
}

/// Put the part of a coverage bitmap that lies in `area` down by hand:
/// each pixel gets the colour scaled by the coverage there, which is right
/// on a black background. The bitmap is `width` wide and has its top left
/// corner at (x, y).
pub fn coverage_on_black<D: DrawTarget<Color = Rgb565> + ?Sized>(target: &mut D, area: &Rectangle, x: i32, y: i32, width: u16, coverage: &[u8], color: Rgb565) {
    for row in area.rows() {
        for column in area.columns() {
            let c = coverage[(row - y) as usize * width as usize + (column - x) as usize] as u32;
            if c != 0 {
                let part = |channel: u8| ((channel as u32 * c + 127) / 255) as u8;
                let shade = Rgb565::new(part(color.r()), part(color.g()), part(color.b()));
                let _ = target.fill_solid(&Rectangle::new(Point::new(column, row), Size::new(1, 1)), shade);
            }
        }
    }
}
