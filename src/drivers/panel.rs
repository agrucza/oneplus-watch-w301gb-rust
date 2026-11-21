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
//! The framebuffer is handed in by the caller. Like the stock app, we keep
//! it in the external PSRAM; the LTDC reads it from there itself.

use core::fmt::Write;

use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::{peripherals, Peri};
use embassy_time::Timer;

use crate::hal::dsi;
use crate::services::log::Log;

pub const WIDTH: usize = 454;
pub const HEIGHT: usize = 454;
pub type Frame = [u16; WIDTH * HEIGHT];

/// One step of the panel's start-up.
enum Step {
    /// DCS command with one parameter.
    Set(u8, u8),
    /// DCS command without parameter.
    Do(u8),
    /// DCS command with four parameters.
    Window(u8, [u8; 4]),
    WaitMs(u64),
}

/// The stock app's "display on" sequence.
const PANEL_ON: [Step; 17] = [
    Step::Set(0xFE, 0x01), // vendor register page 1
    Step::Set(0x6A, 0x03),
    Step::Set(0xFE, 0x05),
    Step::Set(0xD0, 0x1D),
    Step::Set(0xFE, 0x01),
    Step::Set(0x0A, 0xF8),
    Step::Set(0xFE, 0x00),                        // back to the standard commands
    Step::Set(0x35, 0x00),                        // tearing-effect output on
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
}

pub struct Panel {
    pub frame: &'static mut Frame,
    _pins: [Output<'static>; 4],
}

impl Panel {
    /// Power the panel, set up LTDC and DSI with `frame` as the picture
    /// source, switch the panel on. Every stage is logged; `None` if one of
    /// them does not complete.
    pub async fn start(r: Resources, frame: &'static mut Frame, log: &mut Log) -> Option<Self> {
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
            framebuffer: frame.as_ptr() as u32,
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
        Some(Self { frame, _pins: [pg5, pf12, power, reset] })
    }

    /// Send the framebuffer to the panel. `done` is false if the transfer
    /// did not finish within 200 ms (for example no tearing-effect pulse).
    pub async fn refresh(&mut self) -> dsi::RefreshStatus {
        dsi::refresh().await
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
            let row = &mut self.frame[y * WIDTH..(y + 1) * WIDTH];
            row.fill(band);
            row[..40].fill(0xFFFF);
        }
        for y in HEIGHT / 2 - 50..HEIGHT / 2 + 50 {
            self.frame[y * WIDTH + WIDTH / 2 - 50..y * WIDTH + WIDTH / 2 + 50].fill(0);
        }
    }
}
