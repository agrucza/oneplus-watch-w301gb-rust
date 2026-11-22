//! How much of the stack has been used.
//!
//! The stack is the 64 KB at the bottom of the RAM (see `memory.x`) and
//! grows downwards from its top, so running out of it means writing below
//! the start of the RAM, which stops the processor with a fault. To see
//! how close the firmware comes, the unused part is filled with a pattern
//! at start (`paint`); later the lowest place where the pattern is gone
//! shows the deepest the stack has reached (`most_used`).

use core::ptr::{read_volatile, write_volatile};

const BOTTOM: usize = 0x2000_0000;
pub const SIZE: usize = 64 * 1024;
const PATTERN: u32 = 0x5AC3_A55A;

/// Fill the unused part of the stack with the pattern. Call early.
pub fn paint() {
    // Leave what is in use, and some room for this function itself, alone.
    let here = cortex_m::register::msp::read() as usize;
    let mut address = BOTTOM;
    while address + 512 < here {
        unsafe { write_volatile(address as *mut u32, PATTERN) };
        address += 4;
    }
}

/// Bytes of the stack that have been used at some point since `paint`.
pub fn most_used() -> usize {
    let mut address = BOTTOM;
    while address < BOTTOM + SIZE && unsafe { read_volatile(address as *const u32) } == PATTERN {
        address += 4;
    }
    BOTTOM + SIZE - address
}
