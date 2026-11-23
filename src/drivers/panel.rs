//! The display panel: 454 x 454 AMOLED with its own frame memory, driven
//! through `hal::dsi`.
//!
//! Values follow the stock *app* (factory build; panel start-up 0x08039662,
//! power-up 0x08039948). The bootloader's own LCD_Init (0x08009E48) differs
//! in timings and uses 24-bit colour on the link.
//!
//! * power: PG5 and PF12 high (supplies), PF5 high (panel power),
//!   PA11 low then high (reset)
//! * pixel clock: PLLSAI2 (HSE / 3 * 10 / 2 = 40 MHz), divided by 2 = 20 MHz
//! * LTDC: sync 0, back porch 1, active 454, total 456 on both axes
//! * DSI: PLL from the 24 MHz crystal with NDIV 80, IDF 6, ODF 1; escape
//!   clock divider 2
//! * commands: see `PANEL_ON`
//!
//! The two frames are handed in by the caller. Like the stock app, we
//! keep them in the external PSRAM; the LTDC reads from there itself. One
//! frame is drawn into while the other is the one the panel was sent
//! (`back`, `present`).

use core::fmt::Write;

use embassy_stm32::dma2d::{self, Buffer2D, Dma2d, PixelFormat};
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::{bind_interrupts, peripherals, Peri};
use embassy_time::Timer;

use crate::hal::dsi;
use crate::services::canvas::Canvas;
use crate::services::log::Log;

bind_interrupts!(struct Irqs {
    DMA2D => dma2d::InterruptHandler<peripherals::DMA2D>;
});

pub const WIDTH: usize = 454;
pub const HEIGHT: usize = 454;
pub type Frame = [u16; WIDTH * HEIGHT];

/// One step of the panel's start-up.
enum Step {
    /// DCS command with one parameter.
    Set(u8, u8),
    /// DCS command without parameter.
    Do(u8),
    /// DCS command with two parameters.
    Pair(u8, [u8; 2]),
    /// DCS command with four parameters.
    Window(u8, [u8; 4]),
    WaitMs(u64),
}

/// The stock app's "display on" sequence.
const PANEL_ON: [Step; 18] = [
    Step::Set(0xFE, 0x01), // vendor register page 1
    Step::Set(0x6A, 0x03),
    Step::Set(0xFE, 0x05),
    Step::Set(0xD0, 0x1D),
    Step::Set(0xFE, 0x01),
    Step::Set(0x0A, 0xF8),
    Step::Set(0xFE, 0x00),                        // back to the standard commands
    // Tearing-effect pulse at line 444 (0x01BC), then its output on: what
    // the stock app's display layer sends before its first frame
    // (0x08039D1C in B.70).
    Step::Pair(0x44, [0x01, 0xBC]),
    Step::Set(0x35, 0x00),
    Step::Set(0x53, 0x20),                        // brightness control on
    Step::Window(0x2A, [0x00, 0x0C, 0x01, 0xD1]), // columns 12..=465
    Step::Set(0x3A, 0x75),                        // 16 bits per pixel
    Step::Window(0x2B, [0x00, 0x00, 0x01, 0xC5]), // rows 0..=453
    Step::Do(0x11),                               // sleep out
    Step::WaitMs(80),
    Step::Set(0x51, 0x85), // brightness
    Step::Do(0x29),        // display on
    Step::WaitMs(80),
];

pub struct Resources {
    pub supply_pg5: Peri<'static, peripherals::PG5>,
    pub supply_pf12: Peri<'static, peripherals::PF12>,
    pub power: Peri<'static, peripherals::PF5>,
    pub reset: Peri<'static, peripherals::PA11>,
    pub _te: Peri<'static, peripherals::PF11>,
    pub _ltdc: Peri<'static, peripherals::LTDC>,
    pub _dsi: Peri<'static, peripherals::DSIHOST>,
    /// The chip's hardware blitter (DMA2D), which the stock app draws
    /// with (its base address is referenced at 0x0808EE14 in B.70).
    pub blitter: Peri<'static, peripherals::DMA2D>,
}

/// A piece of a picture of a move (`Panel::present_move`): a rectangle of
/// the frame with the page coming (`coming`) or of the frame on the
/// screen, with its top left corner at `from` in that frame, shown at
/// `to` on the screen; all as (x, y), `size` as (width, height).
#[derive(Clone, Copy)]
pub struct Piece {
    pub coming: bool,
    pub from: (u32, u32),
    pub to: (u32, u32),
    pub size: (u32, u32),
}

pub struct Panel {
    /// Two frames, as in the stock app: one is on its way to the panel or
    /// was sent last, the other is free to be drawn into.
    frames: [&'static mut Frame; 2],
    /// Which of the two the panel was sent last.
    shown: usize,
    blitter: Dma2d<'static, peripherals::DMA2D>,
    _pins: [Output<'static>; 4],
}

impl Panel {
    /// Power the panel, set up LTDC and DSI with `frame` as the picture
    /// source, switch the panel on. Every stage is logged; `None` if one of
    /// them does not complete.
    pub async fn start(r: Resources, frames: [&'static mut Frame; 2], log: &mut Log) -> Option<Self> {
        // Power-up, as in the stock app.
        let pg5 = Output::new(r.supply_pg5, Level::High, Speed::Low);
        let pf12 = Output::new(r.supply_pf12, Level::High, Speed::Low);
        let mut power = Output::new(r.power, Level::Low, Speed::Low);
        let mut reset = Output::new(r.reset, Level::Low, Speed::Low);
        Timer::after_millis(2).await;
        power.set_high();
        Timer::after_millis(10).await;
        reset.set_high();
        Timer::after_millis(12).await;

        let config = dsi::Config {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            framebuffer: frames[0].as_ptr() as u32,
            back_porch: 1, // the bootloader uses sync 2, back porch 4
            pll_ndiv: 80,
            pll_idf: 6,
            unit_interval_x4: 12, // 4e6 * IDF * ODF / (24000 * NDIV)
            tx_escape_divider: 2,
            clock_lane_time: 0x16,
            data_lp_to_hs: 0x10,
            data_hs_to_lp: 0x0C,
            stop_wait_time: 10,
        };
        match dsi::init(&config).await {
            Ok(()) => {}
            Err(dsi::Error::RegulatorNotReady) => {
                let _ = writeln!(log.stamp(), "display: DSI regulator not ready");
                return None;
            }
            Err(dsi::Error::PllNotLocked) => {
                let _ = writeln!(log.stamp(), "display: DSI PLL did not lock");
                return None;
            }
        }

        for (i, step) in PANEL_ON.iter().enumerate() {
            let taken = match *step {
                Step::Set(command, value) => dsi::dcs_1(command, value).await,
                Step::Do(command) => dsi::dcs(command).await,
                Step::Pair(command, params) => dsi::dcs_long(command, &params).await,
                Step::Window(command, params) => dsi::dcs_long(command, &params).await,
                Step::WaitMs(ms) => {
                    Timer::after_millis(ms).await;
                    true
                }
            };
            if !taken {
                let _ = writeln!(log.stamp(), "display: panel command {} was not taken", i);
                return None;
            }
        }
        dsi::enable();
        let _ = writeln!(log.stamp(), "display: panel switched on");
        let blitter = Dma2d::new(r.blitter, Irqs);
        Some(Self { frames, shown: 0, blitter, _pins: [pg5, pf12, power, reset] })
    }

    /// Send the framebuffer to the panel. `done` is false if the transfer
    /// did not finish within 200 ms (for example no tearing-effect pulse).
    #[allow(dead_code)] // `present` is what the app uses
    pub async fn refresh(&mut self) -> dsi::RefreshStatus {
        dsi::refresh().await
    }

    /// The frame to draw the next picture into. It holds what was drawn
    /// two pictures ago, so a picture has to be drawn completely.
    pub fn back(&mut self) -> &mut Frame {
        self.frames[1 - self.shown]
    }

    /// `frame` (0 or 1) as the blitter wants it described.
    fn blitter_buffer(&mut self, frame: usize) -> Buffer2D {
        Buffer2D::new(self.frames[frame].as_mut_ptr() as *mut u8, PixelFormat::Rgb565, WIDTH as u16, WIDTH as u16, HEIGHT as u16)
    }

    /// Fill a rectangle of the frame to draw into with one colour, using
    /// the hardware blitter. False if the blitter reported an error.
    pub async fn blitter_fill(&mut self, x: u16, y: u16, w: u16, h: u16, color: u16) -> bool {
        let back = 1 - self.shown;
        let region = self.blitter_buffer(back).region(x, y, w, h);
        self.blitter.fill(&region, color as u32).await.is_ok()
    }

    /// Copy the frame the panel was sent last into the frame to draw into,
    /// using the hardware blitter. False if the blitter reported an error.
    pub async fn blitter_copy(&mut self) -> bool {
        let (from, to) = (self.blitter_buffer(self.shown), self.blitter_buffer(1 - self.shown));
        self.blitter.copy(&from, &to).await.is_ok()
    }

    /// The opposite copy, done by the processor: the frame to draw into
    /// is copied over the frame the panel was sent last. For timing only.
    pub fn processor_copy_back_to_shown(&mut self) {
        let (first, second) = self.frames.split_at_mut(1);
        let (from, to) = if self.shown == 0 { (&*second[0], &mut *first[0]) } else { (&*first[0], &mut *second[0]) };
        to.copy_from_slice(from);
    }

    /// True if the two frames hold the same picture.
    pub fn frames_equal(&self) -> bool {
        self.frames[0][..] == self.frames[1][..]
    }

    /// Show one picture of a move from the page on the screen to the page
    /// that has been drawn into the frame to draw into: each piece is a
    /// rectangle of one of the two frames, which the display controller
    /// shows straight from there. Nothing is copied and no frame is
    /// changed; which frame is which stays as it is until `end_move`.
    pub async fn present_move(&mut self, pieces: [Option<Piece>; 2]) -> dsi::RefreshStatus {
        let layers = pieces.map(|piece| {
            piece.map(|p| {
                let frame = if p.coming { 1 - self.shown } else { self.shown };
                dsi::Layer {
                    address: self.frames[frame].as_ptr() as u32 + (p.from.1 * WIDTH as u32 + p.from.0) * 2,
                    x: p.to.0,
                    y: p.to.1,
                    width: p.size.0,
                    height: p.size.1,
                }
            })
        });
        dsi::refresh_layers(layers).await
    }

    /// A move is over, with the page that came on the screen (`arrived`)
    /// or the old one back. The frame with the page that is not shown is
    /// made a copy of the other, so that both hold the page shown, as
    /// drawing expects. False if the blitter reported an error.
    pub async fn end_move(&mut self, arrived: bool) -> bool {
        if arrived {
            self.shown = 1 - self.shown;
        }
        self.settle().await;
        self.blitter_copy().await
    }

    /// Wait until the frame on its way to the panel has gone out, before
    /// the frame memory is read or written for the next picture.
    ///
    /// While a frame travels (10.45 ms), the display controller reads it
    /// from the external memory, and any other use of that memory in the
    /// same time shifts the picture sideways from some line on. Measured
    /// with a moving bar (2026-11-23): whole when the memory is left
    /// alone, displaced when the blitter copies between two frames that
    /// are not even shown, or when the processor only reads one. The
    /// frames in memory stay intact, and neither the display controller
    /// nor the DSI host raises a flag.
    pub async fn settle(&mut self) {
        dsi::frame_out().await;
    }

    /// The frame to draw into, as a target for `embedded-graphics`.
    pub fn canvas(&mut self) -> Canvas<'_> {
        Canvas::new(self.frames[1 - self.shown], &mut self.blitter)
    }

    /// Which of the two frames `back` is: 0 or 1.
    pub fn back_index(&self) -> usize {
        1 - self.shown
    }

    /// Run `f` on both frames (for clearing them).
    #[allow(dead_code)]
    pub fn both(&mut self, mut f: impl FnMut(&mut Frame)) {
        f(self.frames[0]);
        f(self.frames[1]);
    }

    /// Start sending the frame that was drawn into (`back`) to the panel,
    /// in step with it, and make the other one the frame to draw into. It
    /// returns while the frame is on its way; the next `present` waits for
    /// it first. `done` in the answer says whether the frame before had
    /// gone out.
    pub async fn present(&mut self) -> dsi::RefreshStatus {
        let next = 1 - self.shown;
        let status = dsi::refresh_from(self.frames[next].as_ptr() as u32).await;
        self.shown = next;
        status
    }

    /// Set the brightness, 0 (darkest) to 255. False if the command was
    /// not taken.
    pub async fn set_brightness(&mut self, level: u8) -> bool {
        dsi::dcs_1(0x51, level).await
    }

    /// Blank or show the picture (DCS "display off" / "display on"). The
    /// panel stays powered and keeps its frame memory.
    pub async fn set_visible(&mut self, visible: bool) -> bool {
        dsi::dcs(if visible { 0x29 } else { 0x28 }).await
    }

    #[allow(dead_code)] // kept for checking the panel after changes to its set-up
    /// Fill the framebuffer with a test picture: red, green and blue bands
    /// from top to bottom, a white stripe down the left edge, and a black
    /// square in the middle.
    pub fn test_picture(&mut self) {
        for y in 0..HEIGHT {
            let band = match y * 3 / HEIGHT {
                0 => 0xF800u16,
                1 => 0x07E0,
                _ => 0x001F,
            };
            let row = &mut self.back()[y * WIDTH..(y + 1) * WIDTH];
            row.fill(band);
            row[..40].fill(0xFFFF);
        }
        for y in HEIGHT / 2 - 50..HEIGHT / 2 + 50 {
            self.back()[y * WIDTH + WIDTH / 2 - 50..y * WIDTH + WIDTH / 2 + 50].fill(0);
        }
    }
}
