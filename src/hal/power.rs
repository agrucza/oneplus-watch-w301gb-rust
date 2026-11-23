//! The STM32's own switch-off: Shutdown mode (RM0432 ch. 5.3.10), the
//! lowest mode the chip has. Nothing in it keeps running and no wake-up
//! pin is enabled: the chip starts again, from the bootloader, when the
//! Apollo pulls its reset line.

use core::ptr::write_volatile;

use super::reg::{self, RCC_APB1ENR1};

// Power control (PWR, base 0x4000_7000).
const PWR_CR1: *mut u32 = 0x4000_7000 as *mut u32;
const PWR_CR3: *mut u32 = 0x4000_7008 as *mut u32;
const PWR_PUCRF: *mut u32 = 0x4000_7048 as *mut u32;
const PWR_PDCRF: *mut u32 = 0x4000_704C as *mut u32;
// System control block and interrupt controller (ARMv7-M).
const SCB_SCR: *mut u32 = 0xE000_ED10 as *mut u32;
const SYSTICK_CTRL: *mut u32 = 0xE000_E010 as *mut u32;
const NVIC_ICER: *mut u32 = 0xE000_E180 as *mut u32;
const NVIC_ICPR: *mut u32 = 0xE000_E280 as *mut u32;
/// Debug control: cleared so that no debug setting keeps the clocks on.
const DBGMCU_CR: *mut u32 = 0xE004_2004 as *mut u32;

/// Enter Shutdown mode. Does not return: the chip leaves the mode through
/// a reset and starts from the bootloader.
///
/// PF3 is held low for as long as the mode lasts, as the stock app does
/// (0x08024776: PF3 as input with pull-down, the pull-down kept in
/// Shutdown through PWR_PDCRF and the APC bit, then 0x0804C9EA). PF3 is
/// one of the supply enables; which supply it switches is not known. All
/// other pins float in this mode.
///
/// No wake-up pin, as in the stock app. Measured with the upper button
/// as a wake-up pin: it starts the STM32, but the Apollo sleeps on (the
/// reset pulse on PG12 does not wake it), which leaves a watch without
/// sensors and lower button. Measured as well: on the charger the Apollo
/// wakes and restarts the STM32 through the reset line.
pub fn shutdown() -> ! {
    cortex_m::interrupt::disable();
    unsafe {
        // A pending interrupt would end the wait at once: none may be
        // left enabled or pending.
        write_volatile(SYSTICK_CTRL, 0);
        for n in 0..8 {
            write_volatile(NVIC_ICER.add(n), 0xFFFF_FFFF);
            write_volatile(NVIC_ICPR.add(n), 0xFFFF_FFFF);
        }
        reg::modify(RCC_APB1ENR1, 0, 1 << 28); // PWR clock
        reg::modify(PWR_PUCRF, 1 << 3, 0);
        reg::modify(PWR_PDCRF, 0, 1 << 3);
        // APC: apply the pull settings; no wake-up pin (EWUP bits).
        reg::modify(PWR_CR3, 0x1F, 1 << 10);
        reg::modify(PWR_CR1, 0b111, 0b100); // LPMS = Shutdown
        reg::modify(SCB_SCR, 0, 1 << 2); // SLEEPDEEP
        write_volatile(DBGMCU_CR, 0);
    }
    loop {
        cortex_m::asm::dsb();
        cortex_m::asm::wfi();
        // Still here: the mode was not entered. A reset is the nearest
        // thing to switching off that is left.
        cortex_m::peripheral::SCB::sys_reset();
    }
}
