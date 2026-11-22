//! Drawing into the display's framebuffer with `embedded-graphics`.
//!
//! `Canvas` borrows a frame for as long as something is being drawn; the
//! picture reaches the panel with the next `Panel::present`.
//!
//! Filled rectangles above a certain size go through the chip's hardware
//! blitter (DMA2D), everything else is written by the processor. Measured
//! on the watch: the blitter fills at about 22 ns a pixel against the
//! processor's 116 ns, but one blitter operation costs about 30 us in
//! all, in which the processor fills some 260 pixels itself.

use core::future::Future;
use core::task::{Context, Poll, Waker};

use embassy_stm32::dma2d::{Buffer2D, Dma2d, PixelFormat};
use embassy_stm32::peripherals::DMA2D;
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use crate::drivers::panel::{Frame, HEIGHT, WIDTH};

/// Rectangles of at least this many pixels are filled by the blitter.
const BLITTER_FROM: u32 = 320;

pub struct Canvas<'a> {
    frame: &'a mut Frame,
    blitter: &'a mut Dma2d<'static, DMA2D>,
}

impl<'a> Canvas<'a> {
    pub fn new(frame: &'a mut Frame, blitter: &'a mut Dma2d<'static, DMA2D>) -> Self {
        Self { frame, blitter }
    }
}

/// Run one blitter operation to its end, on the spot.
///
/// The blitter's driver is written for waiting on its interrupt; drawing
/// here is not, and for an operation this short waiting on the spot costs
/// less than coming back later. The driver's operation is asked exactly
/// twice: the first time starts the blitter, the second collects the
/// result. In between, the blitter's own flags are watched.
///
/// It must not simply be asked again and again: each time the driver finds
/// the blitter unfinished it sets the start bit anew, and if the blitter
/// finishes just before that, it starts the same operation a second time
/// while the first one's "done" is already being reported. The processor
/// then draws into an area the blitter is still filling. (Seen on the
/// watch as pieces missing from what was drawn after a fill.)
fn finish<F: Future>(operation: F) -> F::Output {
    let mut operation = core::pin::pin!(operation);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = operation.as_mut().poll(&mut context) {
            return output;
        }
        // Started. Wait for "transfer complete", "transfer error" or
        // "configuration error".
        loop {
            let flags = embassy_stm32::pac::DMA2D.isr().read();
            if flags.tcif() || flags.teif() || flags.ceif() {
                break;
            }
        }
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
                self.frame[point.y as usize * WIDTH + point.x as usize] = RawU16::from(color).into_inner();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        let raw = RawU16::from(color).into_inner();
        let (x, w) = (area.top_left.x as usize, area.size.width as usize);
        if area.size.width * area.size.height >= BLITTER_FROM {
            let buffer = Buffer2D::new(self.frame.as_mut_ptr() as *mut u8, PixelFormat::Rgb565, WIDTH as u16, WIDTH as u16, HEIGHT as u16);
            let region = buffer.region(x as u16, area.top_left.y as u16, w as u16, area.size.height as u16);
            if finish(self.blitter.fill(&region, raw as u32)).is_ok() {
                return Ok(());
            }
            // The blitter reported an error: the processor does it.
        }
        for y in area.rows() {
            let start = y as usize * WIDTH + x;
            self.frame[start..start + w].fill(raw);
        }
        Ok(())
    }
}
