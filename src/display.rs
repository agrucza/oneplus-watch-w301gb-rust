//! The display: a 454 x 454 AMOLED panel behind the STM32's LTDC controller
//! and DSI host, in DSI "adapted command mode" (the panel has its own frame
//! memory; a frame is sent when asked for, timed by the panel's
//! tearing-effect signal).
//!
//! Values follow the stock *app* (factory build; LTDC init 0x08039260, DSI
//! init 0x08039318, panel start-up 0x08039662, power-up 0x08039948, low-level
//! setup 0x08039E90). The bootloader's own LCD_Init (0x08009E48) differs in
//! timings and uses 24-bit colour on the link; it is noted where relevant.
//!
//! * power: PG5 and PF12 high (supplies), PF5 high (panel power),
//!   PA11 low then high (reset)
//! * pixel clock: PLLSAI2 (HSE / 3 * 10 / 2 = 40 MHz), divided by 2 = 20 MHz
//! * LTDC: sync 0, back porch 1, active 454, total 456 on both axes; one
//!   layer, RGB565
//! * DSI: one data lane; PLL from the 24 MHz crystal with NDIV 80, IDF 6,
//!   ODF 1; escape clock divider 2; RGB565 on the link; tearing effect on
//!   pin PF11 (alternate function 11), rising edge
//! * panel commands, low-power mode: see `PANEL_ON`
//!
//! The stock app keeps its framebuffer in external PSRAM. This first version
//! uses internal RAM (412 KB), as the bootloader does.

use core::fmt::Write;
use core::ptr::{read_volatile, write_volatile};

use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::pac::{dsihost::regs as dsi, ltdc::regs as lt, DSIHOST, LTDC};
use embassy_stm32::{peripherals, Peri};
use embassy_time::{Duration, Instant, Timer};

use crate::log::Log;

pub const WIDTH: usize = 454;
pub const HEIGHT: usize = 454;

const RCC_APB2RSTR: *mut u32 = 0x4002_1040 as *mut u32;
const RCC_APB2ENR: *mut u32 = 0x4002_1060 as *mut u32;
const GPIOF: usize = 0x4800_1400;
const LTDC_BIT: u32 = 1 << 26;
const DSI_BIT: u32 = 1 << 27;

// DSI packet types
const DCS_SHORT_NO_PARAM: u32 = 0x05;
const DCS_SHORT_ONE_PARAM: u32 = 0x15;
const DCS_LONG: u32 = 0x39;

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
const PANEL_ON: [Step; 18] = [
    Step::Set(0xFE, 0x01), // vendor register page 1
    Step::Set(0x6A, 0x03),
    Step::Set(0xFE, 0x05),
    Step::Set(0xD0, 0x1D),
    Step::Set(0xFE, 0x01),
    Step::Set(0x0A, 0xF8),
    Step::Set(0xFE, 0x00),                   // back to the standard commands
    Step::Set(0x35, 0x00),                   // tearing-effect output on
    Step::Set(0x53, 0x20),                   // brightness control on
    Step::Window(0x2A, [0x00, 0x0C, 0x01, 0xD1]), // columns 12..=465
    Step::Set(0x3A, 0x75),                   // 16 bits per pixel
    Step::Window(0x2B, [0x00, 0x00, 0x01, 0xC5]), // rows 0..=453
    Step::Do(0x11),                          // sleep out
    Step::WaitMs(80),
    Step::Set(0x51, 0x85),                   // brightness
    Step::Do(0x29),                          // display on
    Step::WaitMs(80),
    Step::WaitMs(0),
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

pub struct Display {
    pub frame: &'static mut [u16; WIDTH * HEIGHT],
    _pins: [Output<'static>; 4],
}

unsafe fn modify(reg: *mut u32, clear: u32, set: u32) {
    write_volatile(reg, (read_volatile(reg) & !clear) | set);
}

/// Wait until `done()` is true; false if `limit` passes first.
async fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while !done() {
        if Instant::now() > end {
            return false;
        }
        Timer::after_micros(100).await;
    }
    true
}

/// Send one DSI packet header once the command FIFO is empty.
async fn dsi_header(kind: u32, low: u32, high: u32) -> bool {
    // GPSR bit 0: command FIFO empty.
    if !wait_for(Duration::from_millis(100), || DSIHOST.gpsr().read().0 & 1 != 0).await {
        return false;
    }
    DSIHOST.ghcr().write_value(dsi::Ghcr(kind | low << 8 | high << 16));
    true
}

async fn run_step(step: &Step) -> bool {
    match *step {
        Step::Set(command, value) => dsi_header(DCS_SHORT_ONE_PARAM, command as u32, value as u32).await,
        Step::Do(command) => dsi_header(DCS_SHORT_NO_PARAM, command as u32, 0).await,
        Step::Window(command, p) => {
            if !wait_for(Duration::from_millis(100), || DSIHOST.gpsr().read().0 & 1 != 0).await {
                return false;
            }
            // Payload: command byte, then the parameters, four bytes per word.
            DSIHOST.gpdr().write_value(dsi::Gpdr(u32::from_le_bytes([command, p[0], p[1], p[2]])));
            DSIHOST.gpdr().write_value(dsi::Gpdr(p[3] as u32));
            dsi_header(DCS_LONG, 5, 0).await
        }
        Step::WaitMs(ms) => {
            Timer::after_millis(ms).await;
            true
        }
    }
}

impl Display {
    /// Power the panel, set up LTDC and DSI, switch the panel on. Every stage
    /// is logged; `None` if one of them does not complete.
    pub async fn start(r: Resources, log: &mut Log) -> Option<Self> {
        let frame = cortex_m::singleton!(: [u16; WIDTH * HEIGHT] = [0; WIDTH * HEIGHT])?;

        // --- power-up, as in the stock app ---------------------------------
        let pg5 = Output::new(r.supply_pg5, Level::High, Speed::Low);
        let pf12 = Output::new(r.supply_pf12, Level::High, Speed::Low);
        let mut power = Output::new(r.power, Level::Low, Speed::Low);
        let mut reset = Output::new(r.reset, Level::Low, Speed::Low);
        Timer::after_millis(2).await;
        power.set_high();
        Timer::after_millis(10).await;
        reset.set_high();
        Timer::after_millis(12).await;

        unsafe {
            // Clocks and a clean start for both peripherals.
            modify(RCC_APB2ENR, 0, LTDC_BIT | DSI_BIT);
            let _ = read_volatile(RCC_APB2ENR);
            modify(RCC_APB2RSTR, 0, LTDC_BIT | DSI_BIT);
            modify(RCC_APB2RSTR, LTDC_BIT | DSI_BIT, 0);
            // Tearing-effect input: PF11, alternate function 11.
            modify(GPIOF as *mut u32, 0b11 << 22, 0b10 << 22);
            modify((GPIOF + 0x24) as *mut u32, 0xF << 12, 11 << 12);
        }

        // --- LTDC -----------------------------------------------------------
        // Timings as accumulated counts: sync 0, back porch 1, active to 455,
        // total 456 (stock app; the bootloader uses 2 / 4 / 458 / 460).
        const BACK_PORCH: u32 = 1;
        LTDC.sscr().write_value(lt::Sscr(0));
        LTDC.bpcr().write_value(lt::Bpcr(BACK_PORCH << 16 | BACK_PORCH));
        LTDC.awcr().write_value(lt::Awcr((BACK_PORCH + WIDTH as u32) << 16 | (BACK_PORCH + HEIGHT as u32)));
        LTDC.twcr().write_value(lt::Twcr((BACK_PORCH + WIDTH as u32 + 1) << 16 | (BACK_PORCH + HEIGHT as u32 + 1)));
        LTDC.bccr().write_value(lt::Bccr(0));
        let layer = LTDC.layer(0);
        layer.whpcr().write_value(lt::Whpcr((BACK_PORCH + WIDTH as u32) << 16 | (BACK_PORCH + 1)));
        layer.wvpcr().write_value(lt::Wvpcr((BACK_PORCH + HEIGHT as u32) << 16 | (BACK_PORCH + 1)));
        layer.pfcr().write_value(lt::Pfcr(2)); // RGB565
        layer.dccr().write_value(lt::Dccr(0));
        layer.cacr().write_value(lt::Cacr(0xFF));
        layer.bfcr().write_value(lt::Bfcr(0x600 | 0x7));
        layer.cfbar().write_value(lt::Cfbar(frame.as_ptr() as u32));
        layer.cfblr().write_value(lt::Cfblr((WIDTH as u32 * 2) << 16 | (WIDTH as u32 * 2 + 3)));
        layer.cfblnr().write_value(lt::Cfblnr(HEIGHT as u32));
        layer.cr().write_value(lt::Cr(1)); // layer on
        LTDC.srcr().write_value(lt::Srcr(1)); // load the layer settings now
        LTDC.gcr().modify(|w| w.0 |= 1); // LTDC on

        // --- DSI host -------------------------------------------------------
        // Regulator, then the PLL (WRPCR: bit 24 regulator, bits 8:2 NDIV,
        // 14:11 IDF, 17:16 ODF, bit 0 enable). WISR bit 12 / bit 8 = ready.
        DSIHOST.wrpcr().write_value(dsi::Wrpcr(1 << 24));
        if !wait_for(Duration::from_millis(100), || DSIHOST.wisr().read().0 & 1 << 12 != 0).await {
            let _ = writeln!(log.stamp(), "display: DSI regulator not ready");
            return None;
        }
        DSIHOST.wrpcr().write_value(dsi::Wrpcr(1 << 24 | 80 << 2 | 6 << 11));
        DSIHOST.wrpcr().modify(|w| w.0 |= 1);
        if !wait_for(Duration::from_millis(100), || DSIHOST.wisr().read().0 & 1 << 8 != 0).await {
            let _ = writeln!(log.stamp(), "display: DSI PLL did not lock");
            return None;
        }
        // D-PHY: clock and data lanes on; automatic clock-lane control; one
        // data lane; stop-wait time 10.
        DSIHOST.pctlr().write_value(dsi::Pctlr(1 << 2 | 1 << 1));
        DSIHOST.clcr().write_value(dsi::Clcr(1 << 1 | 1 << 0));
        DSIHOST.pconfr().write_value(dsi::Pconfr(10 << 8));
        // Escape clock divider 2, timeout clock divider 1.
        DSIHOST.ccr().write_value(dsi::Ccr(1 << 8 | 2));
        // Unit interval in quarter nanoseconds: 4e6 * IDF * ODF / (24000 * NDIV) = 12.
        DSIHOST.wpcr0().write_value(dsi::Wpcr0(12));
        DSIHOST.ier0().write_value(dsi::Ier0(0));
        DSIHOST.ier1().write_value(dsi::Ier1(0));
        // PHY timings: clock lane 0x16 both ways; data lane LP->HS 0x10, HS->LP 0x0C.
        DSIHOST.cltcr().write_value(dsi::Cltcr(0x16 << 16 | 0x16));
        DSIHOST.dltcr().write_value(dsi::Dltcr(0x0C << 24 | 0x10 << 16));
        // Bus turn-around on.
        DSIHOST.pcr().write_value(dsi::Pcr(1 << 2));
        // Adapted command mode: 16-bit colour, tearing effect from the pin
        // (rising edge), refresh on request, 454 pixels per line command.
        DSIHOST.mcr().write_value(dsi::Mcr(1));
        DSIHOST.wcfgr().write_value(dsi::Wcfgr(1 << 4 | 1));
        DSIHOST.lvcidr().write_value(dsi::Lvcidr(0));
        DSIHOST.lpcr().write_value(dsi::Lpcr(1 << 2 | 1 << 1));
        DSIHOST.lcolcr().write_value(dsi::Lcolcr(0));
        DSIHOST.lccr().write_value(dsi::Lccr(WIDTH as u32));
        // All short commands and long generic writes in low-power mode; DCS
        // long writes (the pixels) at high speed.
        DSIHOST.cmcr().write_value(dsi::Cmcr(0x0107_7F00));
        DSIHOST.cr().write_value(dsi::Cr(1)); // host on

        // --- panel ----------------------------------------------------------
        for (i, step) in PANEL_ON.iter().enumerate() {
            if !run_step(step).await {
                let _ = writeln!(log.stamp(), "display: panel command {} was not taken", i);
                return None;
            }
        }
        DSIHOST.wcr().modify(|w| w.0 |= 1 << 3); // DSI wrapper on
        let _ = writeln!(log.stamp(), "display: panel switched on");
        Some(Self { frame, _pins: [pg5, pf12, power, reset] })
    }

    /// Send the framebuffer to the panel. Returns false if the transfer did
    /// not finish within 200 ms (for example no tearing-effect pulse).
    pub async fn refresh(&mut self, log: &mut Log) -> bool {
        DSIHOST.wifcr().write_value(dsi::Wifcr(0b11)); // clear "tearing effect" and "refresh done"
        DSIHOST.wcr().modify(|w| w.0 |= 1 << 2); // request one frame
        // WISR bit 1: end of refresh.
        let done = wait_for(Duration::from_millis(200), || DSIHOST.wisr().read().0 & 1 << 1 != 0).await;
        let _ = writeln!(
            log.stamp(),
            "display: refresh {}; WISR={:#010x} ISR0={:#010x} ISR1={:#010x} PSR={:#010x}",
            if done { "done" } else { "NOT finished in 200 ms" },
            DSIHOST.wisr().read().0,
            DSIHOST.isr0().read().0,
            DSIHOST.isr1().read().0,
            DSIHOST.psr().read().0
        );
        done
    }

    /// Fill the framebuffer with a test picture: red, green and blue bands
    /// from top to bottom, with a white stripe down the left edge.
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
    }
}
