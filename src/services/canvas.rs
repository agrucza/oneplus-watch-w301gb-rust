//! Drawing into the display's framebuffer with `embedded-graphics`.
//!
//! `Canvas` borrows the frame for as long as something is being drawn; the
//! picture reaches the panel with the next `Panel::refresh`.

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use crate::drivers::panel::{Frame, HEIGHT, WIDTH};

pub struct Canvas<'a>(pub &'a mut Frame);

impl Canvas<'_> {
    /// Fill the whole frame with one colour.
    pub fn clear_to(&mut self, color: Rgb565) {
        self.0.fill(RawU16::from(color).into_inner());
    }
}

impl OriginDimensions for Canvas<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for Canvas<'_> {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if (0..WIDTH as i32).contains(&point.x) && (0..HEIGHT as i32).contains(&point.y) {
                self.0[point.y as usize * WIDTH + point.x as usize] = RawU16::from(color).into_inner();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        let raw = RawU16::from(color).into_inner();
        let (x, w) = (area.top_left.x as usize, area.size.width as usize);
        for y in area.rows() {
            let start = y as usize * WIDTH + x;
            self.0[start..start + w].fill(raw);
        }
        Ok(())
    }
}
