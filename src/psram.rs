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
//! Pins, all alternate function 12: port D except PD2, PB7 (address latch),
//! PE0/PE1 (byte lanes), PE3..PE15, PG9. PE2 is a plain output (CRE).
//! FMC clock = system clock / 2 (stock: 60 MHz; here 48 MHz).
//!
//! The HAL has neither a driver nor register definitions for the FMC.
//!
//! Measured on the watch at a 48 MHz bus clock: all 16 MB written and read
//! back correctly, 3.7 s for the full pass.
//!
//! Memory map: the stock app keeps its framebuffer in the 2 MB at
//! 0x60C0_0000, and so do we.

use core::fmt::Write;
use core::ptr::{read_volatile, write_volatile};

use embassy_time::{Instant, Timer};

use crate::log::Log;

pub const BASE: usize = 0x6000_0000;
pub const SIZE: usize = 16 * 1024 * 1024;
/// Start of the 2 MB the display's framebuffer lives in.
pub const FRAMEBUFFER: usize = BASE + 0x00C0_0000;

const RCC_AHB2ENR: *mut u32 = 0x4002_104C as *mut u32;
const RCC_AHB3ENR: *mut u32 = 0x4002_1050 as *mut u32;
const FMC_BCR1: *mut u32 = 0xA000_0000 as *mut u32;
const FMC_BTR1: *mut u32 = 0xA000_0004 as *mut u32;

const GPIOB: usize = 0x4800_0400;
const GPIOD: usize = 0x4800_0C00;
const GPIOE: usize = 0x4800_1000;
const GPIOG: usize = 0x4800_1800;

// FMC_BCR1 bits
const MBKEN: u32 = 1 << 0; // bank enabled
const MUXEN: u32 = 1 << 1; // address/data multiplexed
const MTYP_PSRAM: u32 = 1 << 2;
const MWID_16: u32 = 1 << 4;
const RESERVED_ONE: u32 = 1 << 7;
const BURSTEN: u32 = 1 << 8; // synchronous reads
const WAITPOL: u32 = 1 << 9; // wait signal active high
const WREN: u32 = 1 << 12;
const WAITEN: u32 = 1 << 13;
const CBURSTRW: u32 = 1 << 19; // synchronous writes
const ASYNC: u32 = RESERVED_ONE | MUXEN | MTYP_PSRAM | MWID_16 | WAITPOL | WREN | WAITEN | MBKEN;
const BURST: u32 = ASYNC | BURSTEN | CBURSTRW;
/// Timing: address setup 1, address hold 5, data setup 4, data hold 1,
/// clock divider 2, data latency 2 (the last three are stored minus 1 / 2).
const TIMING: u32 = 1 | 5 << 4 | 4 << 8 | (2 - 1) << 20;

unsafe fn modify(reg: *mut u32, clear: u32, set: u32) {
    write_volatile(reg, (read_volatile(reg) & !clear) | set);
}

/// Put the pins in `mask` of a port on alternate function 12, no pull.
unsafe fn fmc_pins(port: usize, mask: u32, speed: u32) {
    for n in (0..16).filter(|n| mask & 1 << n != 0) {
        modify(port as *mut u32, 0b11 << (2 * n), 0b10 << (2 * n));
        modify((port + 0x08) as *mut u32, 0b11 << (2 * n), speed << (2 * n));
        modify((port + 0x0C) as *mut u32, 0b11 << (2 * n), 0);
        let afr = (port + 0x20 + 4 * (n as usize / 8)) as *mut u32;
        modify(afr, 0xF << (4 * (n % 8)), 12 << (4 * (n % 8)));
    }
}

unsafe fn cre(high: bool) {
    // PE2 through the bit set/reset register.
    write_volatile((GPIOE + 0x18) as *mut u32, if high { 1 << 2 } else { 1 << (2 + 16) });
}

/// Bring the memory up as the stock app does.
pub async fn init() {
    unsafe {
        modify(RCC_AHB2ENR, 0, 1 << 1 | 1 << 3 | 1 << 4 | 1 << 6); // GPIO B, D, E, G
        modify(RCC_AHB3ENR, 0, 1); // FMC
        let _ = read_volatile(RCC_AHB3ENR);

        // CRE: output, low.
        cre(false);
        modify(GPIOE as *mut u32, 0b11 << 4, 0b01 << 4);

        fmc_pins(GPIOD, 0xFFFB, 3);
        fmc_pins(GPIOB, 1 << 7, 3);
        fmc_pins(GPIOE, 0xFFF8, 3);
        fmc_pins(GPIOE, 0x0003, 1);
        fmc_pins(GPIOG, 1 << 9, 3);

        // Step 1: asynchronous.
        write_volatile(FMC_BTR1, TIMING);
        write_volatile(FMC_BCR1, ASYNC);
    }
    Timer::after_millis(100).await;
    unsafe {
        // Step 2: the memory's configuration register, then burst mode.
        cre(true);
        write_volatile((BASE + 0x10_3A3E) as *mut u16, 0x1D1F);
        let _ = read_volatile((BASE + 0x10_0000) as *const u16);
        cre(false);
        write_volatile(FMC_BTR1, TIMING);
        write_volatile(FMC_BCR1, BURST);
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
                crate::watchdog::feed();
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
                crate::watchdog::feed();
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
