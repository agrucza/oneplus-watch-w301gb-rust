//! Independent watchdog, straight on the registers (RM0432 ch. 41), so it can
//! be armed before the HAL is initialised. Once started it cannot be stopped
//! until the next reset - which is exactly what a dead-man switch should do.

use core::ptr::{read_volatile, write_volatile};

const KR: *mut u32 = 0x4000_3000 as *mut u32;
const PR: *mut u32 = 0x4000_3004 as *mut u32;
const RLR: *mut u32 = 0x4000_3008 as *mut u32;
const SR: *const u32 = 0x4000_300C as *const u32;

/// Start the watchdog with a timeout of about `secs` seconds (max ~32).
/// LSI (32 kHz) / 256 = 125 Hz counter.
pub fn start(secs: u32) {
    let reload = (secs * 125).clamp(1, 0xFFF);
    unsafe {
        write_volatile(KR, 0xCCCC); // start (also turns LSI on)
        write_volatile(KR, 0x5555); // unlock PR/RLR
        write_volatile(PR, 6); // divide by 256
        write_volatile(RLR, reload);
        // Wait for the update to land, but never hang here: if it doesn't,
        // the watchdog still runs with its (shorter) reset-default period.
        let mut tries = 1_000_000u32;
        while read_volatile(SR) & 0b11 != 0 && tries > 0 {
            tries -= 1;
        }
        write_volatile(KR, 0xAAAA);
    }
}

/// Restart the countdown.
pub fn feed() {
    unsafe { write_volatile(KR, 0xAAAA) };
}
