//! Flexible memory controller (FMC), bank 1 at 0x6000_0000, for a 16-bit
//! memory with address and data on the same lines.
//!
//! The HAL has neither a driver nor register definitions for the FMC. Pin
//! set-up follows the stock app (0x0808F860): all alternate function 12 -
//! port D except PD2, PB7 (address latch), PE0/PE1 (byte lanes), PE3..PE15,
//! PG9.

use core::ptr::{read_volatile, write_volatile};

use crate::hal::reg::{modify, RCC_AHB2ENR, RCC_AHB3ENR};

const FMC_BCR1: *mut u32 = 0xA000_0000 as *mut u32;
const FMC_BTR1: *mut u32 = 0xA000_0004 as *mut u32;

const GPIOB: usize = 0x4800_0400;
const GPIOD: usize = 0x4800_0C00;
const GPIOE: usize = 0x4800_1000;
const GPIOG: usize = 0x4800_1800;

// Bank control register bits.
const MBKEN: u32 = 1 << 0; // bank enabled
pub const MUXEN: u32 = 1 << 1; // address/data multiplexed
pub const MTYP_PSRAM: u32 = 1 << 2;
pub const MWID_16: u32 = 1 << 4;
const RESERVED_ONE: u32 = 1 << 7;
pub const BURSTEN: u32 = 1 << 8; // synchronous reads
pub const WAITPOL: u32 = 1 << 9; // wait signal active high
pub const WREN: u32 = 1 << 12;
pub const WAITEN: u32 = 1 << 13;
pub const CBURSTRW: u32 = 1 << 19; // synchronous writes

/// Clocks for the FMC and the ports its pins are on (B, D, E, G).
///
/// # Safety
/// Writes RCC registers; call from one place during start-up.
pub unsafe fn enable() {
    modify(RCC_AHB2ENR, 0, 1 << 1 | 1 << 3 | 1 << 4 | 1 << 6);
    modify(RCC_AHB3ENR, 0, 1);
    let _ = read_volatile(RCC_AHB3ENR);
}

/// Put the pins in `mask` of a port on alternate function 12, no pull.
unsafe fn pins(port: usize, mask: u32, speed: u32) {
    for n in (0..16).filter(|n| mask & 1 << n != 0) {
        modify(port as *mut u32, 0b11 << (2 * n), 0b10 << (2 * n));
        modify((port + 0x08) as *mut u32, 0b11 << (2 * n), speed << (2 * n));
        modify((port + 0x0C) as *mut u32, 0b11 << (2 * n), 0);
        let afr = (port + 0x20 + 4 * (n as usize / 8)) as *mut u32;
        modify(afr, 0xF << (4 * (n % 8)), 12 << (4 * (n % 8)));
    }
}

/// Hand the bus pins to the FMC.
///
/// # Safety
/// Reconfigures about 30 pins; nothing else may be using them.
pub unsafe fn multiplexed_16bit_pins() {
    pins(GPIOD, 0xFFFB, 3);
    pins(GPIOB, 1 << 7, 3);
    pins(GPIOE, 0xFFF8, 3);
    pins(GPIOE, 0x0003, 1);
    pins(GPIOG, 1 << 9, 3);
}

/// Set bank 1's timing and control registers and enable the bank.
/// `control` is a combination of the bit constants above.
///
/// # Safety
/// The memory must be able to work with the given settings.
pub unsafe fn configure_bank1(control: u32, timing: u32) {
    write_volatile(FMC_BTR1, timing);
    write_volatile(FMC_BCR1, control | RESERVED_ONE | MBKEN);
}
