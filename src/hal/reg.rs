//! Register access shared by the register-level code: the HAL has no driver
//! for several of this chip's peripherals (SDMMC, FMC, the Apollo's SPI
//! slave with DMA), and some set-up has to happen around the HAL.

use core::ptr::{read_volatile, write_volatile};

/// Clear the bits in `clear`, then set the bits in `set`.
///
/// # Safety
/// `reg` must be a memory-mapped register that may be read and written.
pub unsafe fn modify(reg: *mut u32, clear: u32, set: u32) {
    write_volatile(reg, (read_volatile(reg) & !clear) | set);
}

// Reset and clock control (RCC, base 0x4002_1000).
pub const RCC_CR: *mut u32 = 0x4002_1000 as *mut u32;
pub const RCC_CFGR: *mut u32 = 0x4002_1008 as *mut u32;
pub const RCC_PLLCFGR: *mut u32 = 0x4002_100C as *mut u32;
pub const RCC_AHB2RSTR: *mut u32 = 0x4002_102C as *mut u32;
pub const RCC_APB2RSTR: *mut u32 = 0x4002_1040 as *mut u32;
pub const RCC_AHB1ENR: *mut u32 = 0x4002_1048 as *mut u32;
pub const RCC_AHB2ENR: *mut u32 = 0x4002_104C as *mut u32;
pub const RCC_AHB3ENR: *mut u32 = 0x4002_1050 as *mut u32;
pub const RCC_APB1ENR1: *mut u32 = 0x4002_1058 as *mut u32;
pub const RCC_APB2ENR: *mut u32 = 0x4002_1060 as *mut u32;
pub const RCC_BDCR: *mut u32 = 0x4002_1090 as *mut u32;
