//! LTDC and DSI host in DSI "adapted command mode": the LTDC reads a
//! framebuffer and the DSI host sends it to a panel that has its own frame
//! memory, one frame per request, timed by the panel's tearing-effect
//! signal on PF11.
//!
//! This is the controller side only; what the panel needs (timings, PLL,
//! commands) comes in through `Config` and the DCS functions. Register
//! order follows the stock app (LTDC init 0x08039260, DSI init 0x08039318,
//! low-level setup 0x08039E90).
//!
//! The pixel clock (PLLSAI2) is set up in `clocks`.

use core::ptr::read_volatile;

use embassy_stm32::pac::{dsihost::regs as dsi, ltdc::regs as lt, DSIHOST, LTDC};
use embassy_time::{Duration, Instant, Timer};

use crate::hal::reg::{modify, RCC_APB2ENR, RCC_APB2RSTR};

const GPIOF: usize = 0x4800_1400;
const LTDC_BIT: u32 = 1 << 26;
const DSI_BIT: u32 = 1 << 27;

// DSI packet types
const DCS_SHORT_NO_PARAM: u32 = 0x05;
const DCS_SHORT_ONE_PARAM: u32 = 0x15;
const DCS_LONG: u32 = 0x39;

/// What a panel needs from the controller. One data lane, RGB565 from the
/// framebuffer to the link, tearing effect from the pin on its rising edge.
pub struct Config {
    pub width: u32,
    pub height: u32,
    /// Start of the RGB565 framebuffer the LTDC reads.
    pub framebuffer: u32,
    /// LTDC back porch on both axes (sync width is 0).
    pub back_porch: u32,
    /// DSI PLL from the HSE crystal: multiplier and input divider (the
    /// output divider is 1).
    pub pll_ndiv: u32,
    pub pll_idf: u32,
    /// Unit interval in quarter nanoseconds, from the PLL settings.
    pub unit_interval_x4: u32,
    pub tx_escape_divider: u32,
    /// Clock lane switching times, both directions.
    pub clock_lane_time: u32,
    pub data_lp_to_hs: u32,
    pub data_hs_to_lp: u32,
    pub stop_wait_time: u32,
}

#[derive(Clone, Copy)]
pub enum Error {
    RegulatorNotReady,
    PllNotLocked,
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

/// Set up LTDC and DSI host and switch the host on. The panel must be
/// powered and out of reset. Afterwards: panel commands with the `dcs_*`
/// functions, then `enable`.
pub async fn init(c: &Config) -> Result<(), Error> {
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

    // --- LTDC ---------------------------------------------------------------
    // Timings as accumulated counts.
    let bp = c.back_porch;
    LTDC.sscr().write_value(lt::Sscr(0));
    LTDC.bpcr().write_value(lt::Bpcr(bp << 16 | bp));
    LTDC.awcr().write_value(lt::Awcr((bp + c.width) << 16 | (bp + c.height)));
    LTDC.twcr().write_value(lt::Twcr((bp + c.width + 1) << 16 | (bp + c.height + 1)));
    LTDC.bccr().write_value(lt::Bccr(0));
    let layer = LTDC.layer(0);
    layer.whpcr().write_value(lt::Whpcr((bp + c.width) << 16 | (bp + 1)));
    layer.wvpcr().write_value(lt::Wvpcr((bp + c.height) << 16 | (bp + 1)));
    layer.pfcr().write_value(lt::Pfcr(2)); // RGB565
    layer.dccr().write_value(lt::Dccr(0));
    layer.cacr().write_value(lt::Cacr(0xFF));
    layer.bfcr().write_value(lt::Bfcr(0x600 | 0x7));
    layer.cfbar().write_value(lt::Cfbar(c.framebuffer));
    layer.cfblr().write_value(lt::Cfblr((c.width * 2) << 16 | (c.width * 2 + 3)));
    layer.cfblnr().write_value(lt::Cfblnr(c.height));
    layer.cr().write_value(lt::Cr(1)); // layer on
    LTDC.srcr().write_value(lt::Srcr(1)); // load the layer settings now
    LTDC.gcr().modify(|w| w.0 |= 1); // LTDC on

    // --- DSI host -----------------------------------------------------------
    // Regulator, then the PLL (WRPCR: bit 24 regulator, bits 8:2 NDIV,
    // 14:11 IDF, 17:16 ODF, bit 0 enable). WISR bit 12 / bit 8 = ready.
    DSIHOST.wrpcr().write_value(dsi::Wrpcr(1 << 24));
    if !wait_for(Duration::from_millis(100), || DSIHOST.wisr().read().0 & 1 << 12 != 0).await {
        return Err(Error::RegulatorNotReady);
    }
    DSIHOST.wrpcr().write_value(dsi::Wrpcr(1 << 24 | c.pll_ndiv << 2 | c.pll_idf << 11));
    DSIHOST.wrpcr().modify(|w| w.0 |= 1);
    if !wait_for(Duration::from_millis(100), || DSIHOST.wisr().read().0 & 1 << 8 != 0).await {
        return Err(Error::PllNotLocked);
    }
    // D-PHY: clock and data lanes on; automatic clock-lane control; one
    // data lane.
    DSIHOST.pctlr().write_value(dsi::Pctlr(1 << 2 | 1 << 1));
    DSIHOST.clcr().write_value(dsi::Clcr(1 << 1 | 1 << 0));
    DSIHOST.pconfr().write_value(dsi::Pconfr(c.stop_wait_time << 8));
    // Escape clock divider, timeout clock divider 1.
    DSIHOST.ccr().write_value(dsi::Ccr(1 << 8 | c.tx_escape_divider));
    DSIHOST.wpcr0().write_value(dsi::Wpcr0(c.unit_interval_x4));
    DSIHOST.ier0().write_value(dsi::Ier0(0));
    DSIHOST.ier1().write_value(dsi::Ier1(0));
    DSIHOST.cltcr().write_value(dsi::Cltcr(c.clock_lane_time << 16 | c.clock_lane_time));
    DSIHOST.dltcr().write_value(dsi::Dltcr(c.data_hs_to_lp << 24 | c.data_lp_to_hs << 16));
    // Bus turn-around on.
    DSIHOST.pcr().write_value(dsi::Pcr(1 << 2));
    // Adapted command mode: 16-bit colour, tearing effect from the pin
    // (rising edge), refresh on request, one line per command.
    DSIHOST.mcr().write_value(dsi::Mcr(1));
    DSIHOST.wcfgr().write_value(dsi::Wcfgr(1 << 4 | 1));
    DSIHOST.lvcidr().write_value(dsi::Lvcidr(0));
    DSIHOST.lpcr().write_value(dsi::Lpcr(1 << 2 | 1 << 1));
    DSIHOST.lcolcr().write_value(dsi::Lcolcr(0));
    DSIHOST.lccr().write_value(dsi::Lccr(c.width));
    // All short commands and long generic writes in low-power mode; DCS
    // long writes (the pixels) at high speed.
    DSIHOST.cmcr().write_value(dsi::Cmcr(0x0107_7F00));
    DSIHOST.cr().write_value(dsi::Cr(1)); // host on
    Ok(())
}

/// Switch the DSI wrapper on; frames can be sent from here on.
pub fn enable() {
    DSIHOST.wcr().modify(|w| w.0 |= 1 << 3);
}

async fn command_fifo_empty() -> bool {
    // GPSR bit 0: command FIFO empty.
    wait_for(Duration::from_millis(100), || DSIHOST.gpsr().read().0 & 1 != 0).await
}

/// A frame has been requested and may still be on its way to the panel.
static IN_FLIGHT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Wait until the frame that is on its way has gone out (up to 200 ms).
/// True if none was, or it finished in time.
pub async fn frame_out() -> bool {
    if !IN_FLIGHT.load(core::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    // WISR bit 1: end of refresh.
    let done = wait_for(Duration::from_millis(200), || DSIHOST.wisr().read().0 & 1 << 1 != 0).await;
    IN_FLIGHT.store(false, core::sync::atomic::Ordering::Relaxed);
    done
}

async fn header(kind: u32, low: u32, high: u32) -> bool {
    // Commands and a frame do not share the link.
    frame_out().await;
    if !command_fifo_empty().await {
        return false;
    }
    DSIHOST.ghcr().write_value(dsi::Ghcr(kind | low << 8 | high << 16));
    true
}

/// DCS command without parameter. False if the host did not take it.
pub async fn dcs(command: u8) -> bool {
    header(DCS_SHORT_NO_PARAM, command as u32, 0).await
}

/// DCS command with one parameter.
pub async fn dcs_1(command: u8, value: u8) -> bool {
    header(DCS_SHORT_ONE_PARAM, command as u32, value as u32).await
}

/// DCS command with up to seven parameters, as a long packet.
pub async fn dcs_long(command: u8, params: &[u8]) -> bool {
    let mut bytes = [0u8; 8];
    let n = 1 + params.len().min(7);
    bytes[0] = command;
    bytes[1..n].copy_from_slice(&params[..n - 1]);
    if !command_fifo_empty().await {
        return false;
    }
    // Payload first, four bytes per word, then the header with the length.
    for word in bytes[..n].chunks(4) {
        let mut w = [0u8; 4];
        w[..word.len()].copy_from_slice(word);
        DSIHOST.gpdr().write_value(dsi::Gpdr(u32::from_le_bytes(w)));
    }
    header(DCS_LONG, n as u32, 0).await
}

/// The host's state after a refresh, for the log.
pub struct RefreshStatus {
    /// The frame went out within the time limit.
    pub done: bool,
    /// The panel's tearing-effect pulse was seen before the frame was
    /// started (see `refresh`).
    pub in_step: bool,
    pub wisr: u32,
    pub isr0: u32,
    pub isr1: u32,
    pub psr: u32,
}

/// Start sending the frame at `framebuffer` to the panel and return while
/// it is on its way, so the next picture can be drawn into the other
/// frame meanwhile. `done` in the answer is about the frame before: it
/// had gone out when this one was started.
///
/// As the stock app (0x080ABC0A in B.70): on the tearing-effect pulse the
/// DSI host is stopped, the layer's frame address is changed and loaded
/// at once, and the host is started again; then the frame is requested.
pub async fn refresh_from(framebuffer: u32) -> RefreshStatus {
    let done = frame_out().await;
    DSIHOST.wifcr().write_value(dsi::Wifcr(0b11)); // clear "tearing effect" and "refresh done"
    // WISR bit 0: tearing-effect pulse.
    let in_step = wait_for(Duration::from_millis(40), || DSIHOST.wisr().read().0 & 1 != 0).await;
    DSIHOST.cr().write_value(dsi::Cr(0)); // host off
    DSIHOST.wcr().modify(|w| w.0 &= !(1 << 3)); // wrapper off
    LTDC.layer(0).cfbar().write_value(lt::Cfbar(framebuffer));
    LTDC.srcr().write_value(lt::Srcr(1)); // load the layer settings now
    DSIHOST.cr().write_value(dsi::Cr(1)); // host on
    DSIHOST.wcr().modify(|w| w.0 |= 1 << 3); // wrapper on
    DSIHOST.wcr().modify(|w| w.0 |= 1 << 2); // request one frame
    IN_FLIGHT.store(true, core::sync::atomic::Ordering::Relaxed);
    RefreshStatus {
        done,
        in_step,
        wisr: DSIHOST.wisr().read().0,
        isr0: DSIHOST.isr0().read().0,
        isr1: DSIHOST.isr1().read().0,
        psr: DSIHOST.psr().read().0,
    }
}

/// Send the framebuffer to the panel once.
///
/// As the stock app (0x080ABE1A in B.70, its handler for the panel's
/// tearing-effect pulse): the frame is started on that pulse, which the
/// panel gives once per picture at the line set with its "tear scanline"
/// command, so the panel never shows half an old and half a new frame.
/// Without a pulse within 40 ms the frame is sent anyway (`in_step` is
/// then false).
pub async fn refresh() -> RefreshStatus {
    frame_out().await;
    DSIHOST.wifcr().write_value(dsi::Wifcr(0b11)); // clear "tearing effect" and "refresh done"
    // WISR bit 0: tearing-effect pulse.
    let in_step = wait_for(Duration::from_millis(40), || DSIHOST.wisr().read().0 & 1 != 0).await;
    request_frame(in_step).await
}

/// Ask for one frame and wait until it has gone out (up to 200 ms).
async fn request_frame(in_step: bool) -> RefreshStatus {
    DSIHOST.wcr().modify(|w| w.0 |= 1 << 2); // request one frame
    // WISR bit 1: end of refresh.
    let done = wait_for(Duration::from_millis(200), || DSIHOST.wisr().read().0 & 1 << 1 != 0).await;
    RefreshStatus {
        done,
        in_step,
        wisr: DSIHOST.wisr().read().0,
        isr0: DSIHOST.isr0().read().0,
        isr1: DSIHOST.isr1().read().0,
        psr: DSIHOST.psr().read().0,
    }
}
