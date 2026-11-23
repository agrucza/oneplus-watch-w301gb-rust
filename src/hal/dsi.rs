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

use embassy_stm32::interrupt;
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
    // The second layer: the same fixed settings, off until a picture is
    // put together from two frames (`refresh_layers`).
    let second = LTDC.layer(1);
    second.pfcr().write_value(lt::Pfcr(2));
    second.dccr().write_value(lt::Dccr(0));
    second.cacr().write_value(lt::Cacr(0xFF));
    second.bfcr().write_value(lt::Bfcr(0x600 | 0x7));
    second.cr().write_value(lt::Cr(0));
    BACK_PORCH.store(bp, core::sync::atomic::Ordering::Relaxed);
    PITCH.store(c.width * 2, core::sync::atomic::Ordering::Relaxed);
    WHOLE.store(c.width << 16 | c.height, core::sync::atomic::Ordering::Relaxed);
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
    // The tearing-effect pulse is answered in the interrupt handler below;
    // it is switched on per frame (`refresh_from`).
    DSIHOST.wier().write_value(dsi::Wier(0));
    unsafe { cortex_m::peripheral::NVIC::unmask(embassy_stm32::interrupt::DSI) };
    Ok(())
}

/// Switch the DSI wrapper on; frames can be sent from here on.
/// Switch the DSI host and the display controller off: both are put
/// into reset and lose their clocks, as after the stock app's
/// `LCD_DeInit` (0x08039464 in B.70). No frame may be on its way
/// (`frame_out` first). `init` brings both back.
pub fn deinit() {
    cortex_m::peripheral::NVIC::mask(embassy_stm32::interrupt::DSI);
    ARMED.store(false, core::sync::atomic::Ordering::Relaxed);
    IN_FLIGHT.store(false, core::sync::atomic::Ordering::Relaxed);
    unsafe {
        modify(RCC_APB2RSTR, 0, LTDC_BIT | DSI_BIT);
        modify(RCC_APB2RSTR, LTDC_BIT | DSI_BIT, 0);
        modify(RCC_APB2ENR, LTDC_BIT | DSI_BIT, 0);
    }
}

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
    // Did the display controller run short of pixels during that frame?
    // (LTDC_ISR bit 1, "FIFO underrun"; bit 2, "transfer error".)
    let flags = LTDC.isr().read().0;
    if flags & 0b110 != 0 {
        LTDC.icr().write_value(lt::Icr(0b110));
        STARVED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        STARVED_EVER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    done
}

/// Frames during which the display controller reported that it ran short
/// of pixels or could not read them.
static STARVED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The same, never set back.
static STARVED_EVER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// How many frames were starved since the firmware started.
pub fn starved_ever() -> u32 {
    STARVED_EVER.load(core::sync::atomic::Ordering::Relaxed)
}

/// How many frames were starved since this was last asked.
pub fn take_starved() -> u32 {
    STARVED.swap(0, core::sync::atomic::Ordering::Relaxed)
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

impl RefreshStatus {
    /// Nothing was sent: the display is switched off.
    pub fn skipped() -> Self {
        Self { done: true, in_step: false, wisr: 0, isr0: 0, isr1: 0, psr: 0 }
    }
}

/// What one of the display controller's two layers shows: a rectangle of
/// a frame, at its own size, somewhere on the screen.
#[derive(Clone, Copy)]
pub struct Layer {
    /// Where the rectangle's first pixel lies in memory.
    pub address: u32,
    /// Where on the screen its top left corner goes, and its size.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Start sending the frame at `framebuffer` to the panel and return while
/// it is on its way, so the next picture can be drawn into the other
/// frame meanwhile. `done` in the answer is about the frame before: it
/// had gone out when this one was started.
pub async fn refresh_from(framebuffer: u32) -> RefreshStatus {
    let whole = WHOLE.load(core::sync::atomic::Ordering::Relaxed);
    refresh_layers([Some(Layer { address: framebuffer, x: 0, y: 0, width: whole >> 16, height: whole & 0xFFFF }), None]).await
}

/// Start sending a picture made of up to two layers to the panel, and
/// return while it is on its way. A layer given as `None` is off; where
/// no layer is, the picture is black. The layers must not overlap (the
/// second would lie over the first, but each pixel would then be read
/// from memory twice).
///
/// As the stock app (0x080ABC0A in B.70): on the tearing-effect pulse the
/// DSI host is stopped, the layer settings are changed and loaded at
/// once, and the host is started again; then the frame is requested.
/// Stock changes only the frame's address that way; here the layers'
/// windows go the same way.
///
/// The pulse is answered in the interrupt handler, as in the stock app,
/// and not by looking for it from the task: the frame has to start within
/// a fraction of a millisecond of the pulse, and a task that waits can be
/// kept waiting for many milliseconds by the firmware's other task.
pub async fn refresh_layers(layers: [Option<Layer>; 2]) -> RefreshStatus {
    use core::sync::atomic::Ordering;
    let done = frame_out().await;
    DSIHOST.wifcr().write_value(dsi::Wifcr(0b11)); // clear "tearing effect" and "refresh done"
    // Hand the picture to the interrupt handler and let the pulse call it
    // (WIER bit 0).
    cortex_m::interrupt::free(|cs| NEXT.borrow(cs).set(layers));
    ARMED.store(true, Ordering::Relaxed);
    DSIHOST.wier().modify(|w| w.0 |= 1);
    let in_step = wait_for(Duration::from_millis(40), || !ARMED.load(Ordering::Relaxed)).await;
    if !in_step {
        // No pulse: send the frame anyway, unless the pulse comes this
        // very moment.
        cortex_m::interrupt::free(|_| {
            DSIHOST.wier().modify(|w| w.0 &= !1);
            if ARMED.swap(false, Ordering::Relaxed) {
                start_frame(layers);
            }
        });
    }
    RefreshStatus {
        done,
        in_step,
        wisr: DSIHOST.wisr().read().0,
        isr0: DSIHOST.isr0().read().0,
        isr1: DSIHOST.isr1().read().0,
        psr: DSIHOST.psr().read().0,
    }
}

/// A picture is waiting for the next tearing-effect pulse (`NEXT`).
static ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The layers of the picture the next pulse starts.
static NEXT: cortex_m::interrupt::Mutex<core::cell::Cell<[Option<Layer>; 2]>> = cortex_m::interrupt::Mutex::new(core::cell::Cell::new([None, None]));
/// From `init`: the display controller's back porch, the bytes from one
/// line of a frame to the next, and the picture's size (width << 16 |
/// height).
static BACK_PORCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static PITCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static WHOLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Make `layers` what the panel is sent and send it once.
fn start_frame(layers: [Option<Layer>; 2]) {
    use core::sync::atomic::Ordering;
    let (bp, pitch) = (BACK_PORCH.load(Ordering::Relaxed), PITCH.load(Ordering::Relaxed));
    DSIHOST.cr().write_value(dsi::Cr(0)); // host off
    DSIHOST.wcr().modify(|w| w.0 &= !(1 << 3)); // wrapper off
    for (n, layer) in layers.iter().enumerate() {
        let registers = LTDC.layer(n);
        match layer {
            Some(l) if l.width != 0 && l.height != 0 => {
                // Window: first and last pixel, counted from the start
                // of the line and of the picture including the porch.
                registers.whpcr().write_value(lt::Whpcr((bp + l.x + l.width) << 16 | (bp + l.x + 1)));
                registers.wvpcr().write_value(lt::Wvpcr((bp + l.y + l.height) << 16 | (bp + l.y + 1)));
                registers.cfbar().write_value(lt::Cfbar(l.address));
                registers.cfblr().write_value(lt::Cfblr(pitch << 16 | (l.width * 2 + 3)));
                registers.cfblnr().write_value(lt::Cfblnr(l.height));
                registers.cr().write_value(lt::Cr(1));
            }
            _ => registers.cr().write_value(lt::Cr(0)),
        }
    }
    LTDC.srcr().write_value(lt::Srcr(1)); // load the layer settings now
    DSIHOST.cr().write_value(dsi::Cr(1)); // host on
    DSIHOST.wcr().modify(|w| w.0 |= 1 << 3); // wrapper on
    DSIHOST.wcr().modify(|w| w.0 |= 1 << 2); // request one frame
    IN_FLIGHT.store(true, Ordering::Relaxed);
    // Diagnosis: time the frame. The "end of refresh" interrupt (WIER
    // bit 1) notes when it has gone out.
    STARTED_US.store(Instant::now().as_micros() as u32, Ordering::Relaxed);
    DSIHOST.wier().modify(|w| w.0 |= 1 << 1);
}

/// Diagnosis: when the frame on its way was started, and the shortest and
/// longest time a frame took to go out, in microseconds.
static STARTED_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static TRAVEL_MIN_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(u32::MAX);
static TRAVEL_MAX_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The shortest and the longest time a frame took from its start to
/// "end of refresh" since this was last asked, in microseconds.
pub fn take_travel_us() -> (u32, u32) {
    use core::sync::atomic::Ordering;
    (TRAVEL_MIN_US.swap(u32::MAX, Ordering::Relaxed), TRAVEL_MAX_US.swap(0, Ordering::Relaxed))
}

/// The DSI host's interrupt. The tearing-effect pulse is enabled only
/// while a frame is waiting for it: it starts that frame and switches
/// itself off again. "End of refresh" is enabled while a frame travels,
/// for timing it (diagnosis); its flag is left for `frame_out` to see.
#[interrupt]
unsafe fn DSI() {
    use core::sync::atomic::Ordering;
    let (enabled, flags) = (DSIHOST.wier().read().0, DSIHOST.wisr().read().0);
    if enabled & flags & 1 << 1 != 0 {
        DSIHOST.wier().modify(|w| w.0 &= !(1 << 1));
        let took = (Instant::now().as_micros() as u32).wrapping_sub(STARTED_US.load(Ordering::Relaxed));
        TRAVEL_MIN_US.fetch_min(took, Ordering::Relaxed);
        TRAVEL_MAX_US.fetch_max(took, Ordering::Relaxed);
    }
    if enabled & flags & 1 != 0 {
        DSIHOST.wier().modify(|w| w.0 &= !1);
        DSIHOST.wifcr().write_value(dsi::Wifcr(1));
        if ARMED.swap(false, Ordering::Relaxed) {
            start_frame(cortex_m::interrupt::free(|cs| NEXT.borrow(cs).get()));
        }
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
