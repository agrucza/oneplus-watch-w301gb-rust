//! Drawing into the display's framebuffer with `embedded-graphics`.
//!
//! `Canvas` borrows a frame for as long as something is being drawn; the
//! picture reaches the panel with the next `Panel::present`.
//!
//! Drawing can be limited to a rectangle (`clip_to`), which is how only
//! the changed parts of a picture are repainted.
//!
//! Text from a `services::font::Font` is blended in by the blitter, one
//! character an operation.
//!
//! Filled rectangles above a certain size go through the chip's hardware
//! blitter (DMA2D), everything else is written by the processor. Measured
//! on the watch: the blitter fills at about 22 ns a pixel against the
//! processor's 116 ns, but one blitter operation costs about 30 us in
//! all, in which the processor fills some 260 pixels itself.

use core::future::Future;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};

use embassy_time::{Duration, Instant};

use embassy_stm32::dma2d::{AlphaMode, Buffer2D, BufferKind, ColorConfig, Dma2d, PixelFormat};
use embassy_stm32::peripherals::DMA2D;
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use crate::drivers::panel::{Frame, HEIGHT, WIDTH};
use crate::services::font::{self, Paint};

/// Rectangles of at least this many pixels are filled by the blitter.
const BLITTER_FROM: u32 = 320;

pub struct Canvas<'a> {
    frame: &'a mut Frame,
    blitter: &'a mut Dma2d<'static, DMA2D>,
    /// Nothing is drawn outside this rectangle.
    clip: Rectangle,
}

impl<'a> Canvas<'a> {
    pub fn new(frame: &'a mut Frame, blitter: &'a mut Dma2d<'static, DMA2D>) -> Self {
        // What the blitter writes is always a frame: 16 bits a pixel.
        blitter.set_color_config(BufferKind::Output, &ColorConfig { pixel_format: PixelFormat::Rgb565, alpha_mode: AlphaMode::NoModify });
        Self { frame, blitter, clip: whole() }
    }

    /// Draw only inside `area` from now on.
    pub fn clip_to(&mut self, area: &Rectangle) {
        self.clip = area.intersection(&whole());
    }

    /// The frame as the blitter wants it described.
    fn buffer(&mut self) -> Buffer2D {
        Buffer2D::new(self.frame.as_mut_ptr() as *mut u8, PixelFormat::Rgb565, WIDTH as u16, WIDTH as u16, HEIGHT as u16)
    }
}

fn whole() -> Rectangle {
    Rectangle::new(Point::zero(), Size::new(WIDTH as u32, HEIGHT as u32))
}

impl Paint for Canvas<'_> {
    /// A coverage bitmap, blended into the frame by the blitter: the
    /// bitmap is the "how much" for every pixel, the colour is fixed, and
    /// what is in the frame is the background.
    fn coverage(&mut self, clip: &Rectangle, x: i32, y: i32, width: u16, height: u16, coverage: &[u8], color: Rgb565) {
        let area = Rectangle::new(Point::new(x, y), Size::new(width as u32, height as u32)).intersection(clip).intersection(&self.clip);
        if area.is_zero_sized() {
            return;
        }
        if !blitter_usable() {
            // Without the blitter: the plain way, on black.
            return font::coverage_on_black(self, &area, x, y, width, coverage, color);
        }
        let (w, h) = (area.size.width as u16, area.size.height as u16);
        let bitmap = Buffer2D::new(coverage.as_ptr() as *mut u8, PixelFormat::A8, width, width, height);
        let from = bitmap.region((area.top_left.x - x) as u16, (area.top_left.y - y) as u16, w, h);
        let buffer = self.buffer();
        let onto = buffer.region(area.top_left.x as u16, area.top_left.y as u16, w, h);
        let into = buffer.region(area.top_left.x as u16, area.top_left.y as u16, w, h);
        // The colour as the blitter takes it: eight bits each of red,
        // green and blue.
        let rgb = (color.r() as u32 * 255 / 31) << 16 | (color.g() as u32 * 255 / 63) << 8 | (color.b() as u32 * 255 / 31);
        if !matches!(finish(self.blitter.blit_with(&from, &onto, &into, Some(rgb))), Some(Ok(()))) {
            font::coverage_on_black(self, &area, x, y, width, coverage, color);
        }
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
///
/// A blitter that has not finished after `BLITTER_PATIENCE` is given up
/// on: the operation is aborted, `None` comes back, and from then on
/// everything is drawn by the processor (`blitter_usable`). Every
/// operation here finishes in well under a millisecond when all is well.
fn finish<F: Future>(operation: F) -> Option<F::Output> {
    let mut operation = core::pin::pin!(operation);
    let mut context = Context::from_waker(Waker::noop());
    let deadline = Instant::now() + BLITTER_PATIENCE;
    loop {
        if let Poll::Ready(output) = operation.as_mut().poll(&mut context) {
            return Some(output);
        }
        // Started. Wait for "transfer complete", "transfer error" or
        // "configuration error".
        loop {
            let flags = embassy_stm32::pac::DMA2D.isr().read();
            if flags.tcif() || flags.teif() || flags.ceif() {
                break;
            }
            if Instant::now() > deadline {
                // Stop it (control register bit 2, "abort") and do without.
                embassy_stm32::pac::DMA2D.cr().modify(|w| w.0 |= 1 << 2);
                BLITTER_GIVEN_UP.store(true, Ordering::Relaxed);
                return None;
            }
        }
    }
}

/// How long one blitter operation may take before it is given up on.
const BLITTER_PATIENCE: Duration = Duration::from_millis(50);
/// Set when the blitter once failed to finish; see `finish`.
static BLITTER_GIVEN_UP: AtomicBool = AtomicBool::new(false);

/// False once the blitter has been given up on.
pub fn blitter_usable() -> bool {
    !BLITTER_GIVEN_UP.load(Ordering::Relaxed)
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
            if self.clip.contains(point) {
                self.frame[point.y as usize * WIDTH + point.x as usize] = RawU16::from(color).into_inner();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.clip);
        if area.is_zero_sized() {
            return Ok(());
        }
        let raw = RawU16::from(color).into_inner();
        let (x, w) = (area.top_left.x as usize, area.size.width as usize);
        if area.size.width * area.size.height >= BLITTER_FROM && blitter_usable() {
            let region = self.buffer().region(x as u16, area.top_left.y as u16, w as u16, area.size.height as u16);
            if matches!(finish(self.blitter.fill(&region, raw as u32)), Some(Ok(()))) {
                return Ok(());
            }
            // The blitter reported an error or did not finish: the
            // processor does it.
        }
        for y in area.rows() {
            let start = y as usize * WIDTH + x;
            self.frame[start..start + w].fill(raw);
        }
        Ok(())
    }
}
