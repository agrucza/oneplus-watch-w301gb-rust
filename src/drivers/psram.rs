//! The 16 MB PSRAM on the flexible memory controller (FMC), at 0x6000_0000.
//!
//! Mirrors the stock app's `BSP_FMC_PsramInit` (0x0808F63A) and its pin setup
//! (0x0808F860). The memory is 16 bits wide with address and data sharing
//! the same lines, and is used in two steps:
//!
//! 1. the FMC runs bank 1 in asynchronous mode;
//! 2. with the memory's CRE pin (PE2) high, one write sets the memory's own
//!    configuration register - the value travels on the address lines, hence
//!    the odd address 0x6010_3A3E - which puts it into synchronous burst
//!    mode; then the FMC is switched to burst mode as well.
//!
//! The bus itself (pins, bank registers) is in `hal::fmc`; PE2 is the
//! memory's CRE pin, a plain output.
//! FMC clock = system clock / 2 = 60 MHz, as in the stock app.
//!
//! Measured on the watch: all 16 MB written and read back correctly, in
//! 3.2 s at the 60 MHz bus clock (3.7 s at 48 MHz, when the system clock
//! was 96 MHz).
//!
//! Memory map: the stock app keeps its framebuffer in the 2 MB at
//! 0x60C0_0000, and so do we.

use core::fmt::Write;
use core::ptr::{read_volatile, write_volatile};

use embassy_time::{Instant, Timer};

use crate::hal::fmc;
use crate::hal::reg::modify;
use crate::services::log::Log;

pub const BASE: usize = 0x6000_0000;
pub const SIZE: usize = 16 * 1024 * 1024;
/// Start of the 2 MB the display's framebuffer lives in.
pub const FRAMEBUFFER: usize = BASE + 0x00C0_0000;
/// Frames lie 0x71800 bytes apart from there (512 x 454 pixels of two
/// bytes; the stock app has its second frame at that distance,
/// 0x080ABBD8 in B.70). Nine of them fit before the end of the memory.
pub const FRAME_SPACING: usize = 0x0007_1800;
const _: () = assert!(FRAMEBUFFER + 9 * FRAME_SPACING <= BASE + SIZE);
/// Where a recording from the microphone is kept (`drivers::audio`), and
/// how many bytes it may have; below the frames, used by nothing else.
pub const RECORDING: usize = BASE + 0x0080_0000;
pub const RECORDING_SIZE: usize = 0x0004_0000;

const GPIOE: usize = 0x4800_1000;
const CRE_PIN: u32 = 2;

/// Bank settings: PSRAM, 16 bits wide, address and data on the same lines,
/// wait signal active high. First asynchronous, then with burst mode added.
const ASYNC: u32 = fmc::MUXEN | fmc::MTYP_PSRAM | fmc::MWID_16 | fmc::WAITPOL | fmc::WREN | fmc::WAITEN;
const BURST: u32 = ASYNC | fmc::BURSTEN | fmc::CBURSTRW;
/// Timing: address setup 1, address hold 5, data setup 4, data hold 1,
/// clock divider 2, data latency 2 (the last three are stored minus 1 / 2).
const TIMING: u32 = 1 | 5 << 4 | 4 << 8 | (2 - 1) << 20;

unsafe fn cre(high: bool) {
    // PE2 through the bit set/reset register.
    write_volatile((GPIOE + 0x18) as *mut u32, if high { 1 << CRE_PIN } else { 1 << (CRE_PIN + 16) });
}

/// Bring the memory up as the stock app does.
pub async fn init() {
    unsafe {
        fmc::enable();
        // CRE: output, low.
        cre(false);
        modify(GPIOE as *mut u32, 0b11 << (2 * CRE_PIN), 0b01 << (2 * CRE_PIN));
        fmc::multiplexed_16bit_pins();

        // Step 1: asynchronous.
        fmc::configure_bank1(ASYNC, TIMING);
    }
    Timer::after_millis(100).await;
    unsafe {
        // Step 2: the memory's configuration register, then burst mode.
        cre(true);
        write_volatile((BASE + 0x10_3A3E) as *mut u16, 0x1D1F);
        let _ = read_volatile((BASE + 0x10_0000) as *const u16);
        cre(false);
        fmc::configure_bank1(BURST, TIMING);
    }
    Timer::after_millis(1).await;
}

/// A value that depends on the word's position, so that address faults show.
fn pattern(index: usize) -> u16 {
    let x = (index as u32).wrapping_mul(0x9E37_79B1);
    (x >> 13) as u16 ^ index as u16
}

/// Write and read back 64 words spread over the first megabyte. Run at
/// every boot before anything is put into the memory.
pub fn quick_check(log: &mut Log) -> bool {
    let mem = BASE as *mut u16;
    let mut bad = 0;
    unsafe {
        for i in 0..64 {
            write_volatile(mem.add(i * 4099), pattern(i));
        }
        for i in 0..64 {
            bad += (read_volatile(mem.add(i * 4099)) != pattern(i)) as u32;
        }
    }
    if bad != 0 {
        let got = unsafe { read_volatile(mem) };
        let _ = writeln!(
            log.stamp(),
            "psram: quick check failed, {} of 64 words wrong (first word reads {:#06x}, expected {:#06x})",
            bad,
            got,
            pattern(0)
        );
    }
    bad == 0
}

/// Write and read back the whole memory (about 4 s; destroys its contents);
/// the result goes to the log. Returns true if every word read back
/// correctly.
#[allow(dead_code)] // a diagnostic to call when the memory is in doubt
pub fn full_test(log: &mut Log) -> bool {
    let words = SIZE / 2;
    let mem = BASE as *mut u16;
    let start = Instant::now();
    if !quick_check(log) {
        return false;
    }

    let mut bad = 0u32;
    let mut first_bad = None;
    unsafe {
        for i in 0..words {
            write_volatile(mem.add(i), pattern(i));
            if i & 0x3_FFFF == 0 {
                crate::boot::watchdog::feed();
            }
        }
        for i in 0..words {
            let got = read_volatile(mem.add(i));
            if got != pattern(i) {
                bad += 1;
                if first_bad.is_none() {
                    first_bad = Some((i, got));
                }
            }
            if i & 0x3_FFFF == 0 {
                crate::boot::watchdog::feed();
            }
        }
    }
    let ms = start.elapsed().as_millis();
    match first_bad {
        None => {
            let _ = writeln!(log.stamp(), "psram: all {} MB written and read back correctly in {} ms", SIZE >> 20, ms);
        }
        Some((i, got)) => {
            let _ = writeln!(
                log.stamp(),
                "psram: {} of {} words wrong; first at {:#010x}: read {:#06x}, expected {:#06x} ({} ms)",
                bad,
                words,
                BASE + 2 * i,
                got,
                pattern(i),
                ms
            );
        }
    }
    bad == 0
}
