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

use embassy_stm32::dma2d::{self, Dma2d};
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
    /// The brightness command, with the level the panel is to start at.
    Brightness,
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
    Step::Brightness,
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

/// How many frames the panel is given.
pub const FRAMES: usize = 9;

/// A piece of a picture of a move (`Panel::present_move`): a rectangle of
/// the frame with the page coming (`coming`) or of the frame with the
/// page being left, with its top left corner at `from` in that frame, shown at
/// `to` on the screen; all as (x, y), `size` as (width, height).
#[derive(Clone, Copy)]
pub struct Piece {
    pub coming: bool,
    pub from: (u32, u32),
    pub to: (u32, u32),
    pub size: (u32, u32),
}

pub struct Panel {
    /// The frames pictures are drawn into and shown from. Which frame
    /// holds what is the caller's business.
    frames: [&'static mut Frame; FRAMES],
    blitter: Dma2d<'static, peripherals::DMA2D>,
    _supplies: [Output<'static>; 2],
    power: Output<'static>,
    reset: Output<'static>,
    /// The brightness last asked for.
    brightness: u8,
    /// Powered down (`set_visible(false)`).
    suspended: bool,
    /// Powered up again with the brightness at zero: the level is set
    /// once a picture has gone out, as its frame memory was lost.
    unlit: bool,
}

/// The brightness the panel starts with (the stock app's value).
const START_BRIGHTNESS: u8 = 0x85;

impl Panel {
    /// Power the panel, set up LTDC and DSI with `frame` as the picture
    /// source, switch the panel on. Every stage is logged; `None` if one of
    /// them does not complete.
    pub async fn start(r: Resources, frames: [&'static mut Frame; FRAMES], log: &mut Log) -> Option<Self> {
        let pg5 = Output::new(r.supply_pg5, Level::High, Speed::Low);
        let pf12 = Output::new(r.supply_pf12, Level::High, Speed::Low);
        let power = Output::new(r.power, Level::Low, Speed::Low);
        let reset = Output::new(r.reset, Level::Low, Speed::Low);
        let blitter = Dma2d::new(r.blitter, Irqs);
        let mut panel = Self {
            frames,
            blitter,
            _supplies: [pg5, pf12],
            power,
            reset,
            brightness: START_BRIGHTNESS,
            suspended: false,
            unlit: false,
        };
        if !panel.bring_up(START_BRIGHTNESS, log).await {
            return None;
        }
        let _ = writeln!(log.stamp(), "display: panel switched on");
        Some(panel)
    }

    /// Power the panel, set up LTDC and DSI with the first frame as the
    /// picture source, and send the panel its start-up commands, with
    /// `brightness` as its level. Every stage is logged; false if one of
    /// them does not complete.
    async fn bring_up(&mut self, brightness: u8, log: &mut Log) -> bool {
        // Power-up, as in the stock app.
        self.power.set_low();
        self.reset.set_low();
        Timer::after_millis(2).await;
        self.power.set_high();
        Timer::after_millis(10).await;
        self.reset.set_high();
        Timer::after_millis(12).await;

        let config = dsi::Config {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            framebuffer: self.frames[0].as_ptr() as u32,
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
                return false;
            }
            Err(dsi::Error::PllNotLocked) => {
                let _ = writeln!(log.stamp(), "display: DSI PLL did not lock");
                return false;
            }
        }

        for (i, step) in PANEL_ON.iter().enumerate() {
            let taken = match *step {
                Step::Set(command, value) => dsi::dcs_1(command, value).await,
                Step::Do(command) => dsi::dcs(command).await,
                Step::Brightness => dsi::dcs_1(0x51, brightness).await,
                Step::Pair(command, params) => dsi::dcs_long(command, &params).await,
                Step::Window(command, params) => dsi::dcs_long(command, &params).await,
                Step::WaitMs(ms) => {
                    Timer::after_millis(ms).await;
                    true
                }
            };
            if !taken {
                let _ = writeln!(log.stamp(), "display: panel command {} was not taken", i);
                return false;
            }
        }
        dsi::enable();
        true
    }

    /// Light a panel that was powered up dark, once a picture has gone
    /// out to it.
    async fn light(&mut self) {
        if core::mem::take(&mut self.unlit) {
            dsi::frame_out().await;
            dsi::dcs_1(0x51, self.brightness).await;
        }
    }

    /// Send the framebuffer to the panel. `done` is false if the transfer
    /// did not finish within 200 ms (for example no tearing-effect pulse).
    #[allow(dead_code)] // `present` is what the app uses
    pub async fn refresh(&mut self) -> dsi::RefreshStatus {
        dsi::refresh().await
    }

    /// Show one picture of a move from the page in frame `leaving` to the
    /// page in frame `coming`: each piece is a rectangle of one of the
    /// two, which the display controller shows straight from there.
    /// Nothing is copied and no frame is changed.
    pub async fn present_move(&mut self, pieces: [Option<Piece>; 2], leaving: usize, coming: usize) -> dsi::RefreshStatus {
        let layers = pieces.map(|piece| {
            piece.map(|p| dsi::Layer {
                address: self.frames[if p.coming { coming } else { leaving }].as_ptr() as u32 + (p.from.1 * WIDTH as u32 + p.from.0) * 2,
                x: p.to.0,
                y: p.to.1,
                width: p.size.0,
                height: p.size.1,
            })
        });
        if self.suspended {
            return dsi::RefreshStatus::skipped();
        }
        let status = dsi::refresh_layers(layers).await;
        self.light().await;
        status
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

    /// Frame `frame` as a target for `embedded-graphics`. It must not be
    /// the frame on its way to the panel (`settle` first).
    pub fn canvas(&mut self, frame: usize) -> Canvas<'_> {
        Canvas::new(self.frames[frame], &mut self.blitter)
    }

    /// Start sending frame `frame` to the panel, in step with it. It
    /// returns while the frame is on its way; the next `present` waits for
    /// it first. `done` in the answer says whether the frame before had
    /// gone out.
    pub async fn present(&mut self, frame: usize) -> dsi::RefreshStatus {
        if self.suspended {
            return dsi::RefreshStatus::skipped();
        }
        let status = dsi::refresh_from(self.frames[frame].as_ptr() as u32).await;
        self.light().await;
        status
    }

    /// Set the brightness, 0 (darkest) to 255. False if the command was
    /// not taken. A panel that is powered down, or powered up and not
    /// lit yet, gets the level when it is lit.
    pub async fn set_brightness(&mut self, level: u8) -> bool {
        self.brightness = level;
        if self.suspended || self.unlit {
            return true;
        }
        dsi::dcs_1(0x51, level).await
    }

    /// Switch the panel off or on, as the stock app does when its display
    /// goes dark and comes back (0x08039B72 and the start-up it runs
    /// again, in B.70).
    ///
    /// Off: brightness 0, the vendor register 0x6A on page 1 to 0x15,
    /// 15 ms, "display off", "sleep in"; then the DSI host and the display
    /// controller are switched off, the reset line goes low and 2 ms
    /// later the panel's power. The stock app also keeps PF5 pulled down
    /// for its own low-power modes, which is not needed while the pin is
    /// driven.
    ///
    /// On: the whole start-up again, with the brightness at zero. The
    /// panel's frame memory was lost with its power, so the caller has to
    /// send a picture; the brightness follows it (`light`).
    ///
    /// False if a command was not taken or the start-up failed.
    pub async fn set_visible(&mut self, visible: bool, log: &mut Log) -> bool {
        if visible != self.suspended {
            return true;
        }
        if visible {
            let started = embassy_time::Instant::now();
            let up = self.bring_up(0, log).await;
            self.suspended = !up;
            self.unlit = up;
            let _ = writeln!(log.stamp(), "display: panel powered up in {} ms{}", started.elapsed().as_millis(), if up { "" } else { ", FAILED" });
            return up;
        }
        dsi::frame_out().await;
        let mut taken = dsi::dcs_1(0x51, 0).await;
        taken &= dsi::dcs_1(0xFE, 0x01).await;
        taken &= dsi::dcs_1(0x6A, 0x15).await;
        Timer::after_millis(15).await;
        taken &= dsi::dcs_1(0xFE, 0x00).await;
        taken &= dsi::dcs(0x28).await;
        taken &= dsi::dcs(0x10).await;
        // The commands leave the host's queue before it is switched off.
        Timer::after_millis(5).await;
        dsi::deinit();
        self.reset.set_low();
        Timer::after_millis(2).await;
        self.power.set_low();
        self.suspended = true;
        self.unlit = false;
        taken
    }
}
