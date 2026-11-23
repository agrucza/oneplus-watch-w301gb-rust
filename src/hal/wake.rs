//! The lines that say "something wants attention", as interrupts: the
//! upper button, the Apollo's request line and the Bluetooth chip's
//! interrupt line. A change on one of them ends a wait (`edge`) at once,
//! so that the tasks need not look at the lines every few milliseconds;
//! the same interrupts can end the processor's Stop mode.
//!
//! The drivers keep their pins as plain inputs and read the levels
//! themselves. This module only adds the edge interrupts on top
//! (RM0432 ch. 14, "EXTI"): which port a line listens to (SYSCFG_EXTICR),
//! which edges count (EXTI_RTSR1 / FTSR1) and the line's enable
//! (EXTI_IMR1).

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_stm32::interrupt;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

use super::reg::{modify, RCC_APB2ENR};

const SYSCFG_EXTICR: *mut u32 = 0x4001_0008 as *mut u32;
const EXTI_IMR1: *mut u32 = 0x4001_0400 as *mut u32;
const EXTI_RTSR1: *mut u32 = 0x4001_0408 as *mut u32;
const EXTI_FTSR1: *mut u32 = 0x4001_040C as *mut u32;
const EXTI_PR1: *mut u32 = 0x4001_0414 as *mut u32;

/// A line: its number (the pin number), its port (0 = A, 2 = C, 6 = G)
/// and the edges that count.
struct Line {
    pin: u32,
    port: u32,
    rising: bool,
    falling: bool,
}

/// Upper button, PC13: pressed and released.
const BUTTON: Line = Line { pin: 13, port: 2, rising: true, falling: true };
/// Apollo request, PG1: goes high when the Apollo has a block for us.
const APOLLO: Line = Line { pin: 1, port: 6, rising: true, falling: false };
/// Bluetooth chip's interrupt line, PC7. What it says is not read yet;
/// both edges count, and they are counted for the log.
const BLUETOOTH: Line = Line { pin: 7, port: 2, rising: true, falling: true };

static EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static BUTTON_EDGES: AtomicU32 = AtomicU32::new(0);
static APOLLO_EDGES: AtomicU32 = AtomicU32::new(0);
static BLUETOOTH_EDGES: AtomicU32 = AtomicU32::new(0);
static BLUETOOTH_CHANGED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Switch the edge interrupts on. The pins must already be inputs.
pub fn start() {
    unsafe {
        modify(RCC_APB2ENR, 0, 1); // SYSCFG clock
        let _ = read_volatile(RCC_APB2ENR);
        for line in [BUTTON, APOLLO, BLUETOOTH] {
            let shift = line.pin % 4 * 4;
            modify(SYSCFG_EXTICR.add(line.pin as usize / 4), 0xF << shift, line.port << shift);
            let bit = 1 << line.pin;
            modify(EXTI_RTSR1, bit, if line.rising { bit } else { 0 });
            modify(EXTI_FTSR1, bit, if line.falling { bit } else { 0 });
            write_volatile(EXTI_PR1, bit);
            modify(EXTI_IMR1, 0, bit);
        }
        cortex_m::peripheral::NVIC::unmask(interrupt::EXTI1);
        cortex_m::peripheral::NVIC::unmask(interrupt::EXTI9_5);
        cortex_m::peripheral::NVIC::unmask(interrupt::EXTI15_10);
    }
}

/// Wait until one of the lines changes. A change since the last call
/// counts: nothing is lost between looking at the lines and waiting.
pub async fn edge() {
    EDGE.wait().await
}

/// End a wait for an edge from inside the firmware: something has
/// changed that the waiting task should look at.
pub fn nudge() {
    EDGE.signal(());
}

/// How often each line changed since this was last asked: button,
/// Apollo, Bluetooth.
pub fn take_counts() -> (u32, u32, u32) {
    (BUTTON_EDGES.swap(0, Ordering::Relaxed), APOLLO_EDGES.swap(0, Ordering::Relaxed), BLUETOOTH_EDGES.swap(0, Ordering::Relaxed))
}

/// Whether the Bluetooth chip's line changed since this was last asked.
pub fn take_bluetooth_changed() -> bool {
    BLUETOOTH_CHANGED.swap(false, Ordering::Relaxed)
}

/// Acknowledge the pending lines among `mask` and pass the news on.
fn taken(mask: u32) {
    let pending = unsafe { read_volatile(EXTI_PR1) } & mask;
    unsafe { write_volatile(EXTI_PR1, pending) };
    if pending & 1 << BUTTON.pin != 0 {
        BUTTON_EDGES.fetch_add(1, Ordering::Relaxed);
    }
    if pending & 1 << APOLLO.pin != 0 {
        APOLLO_EDGES.fetch_add(1, Ordering::Relaxed);
    }
    if pending & 1 << BLUETOOTH.pin != 0 {
        BLUETOOTH_EDGES.fetch_add(1, Ordering::Relaxed);
        BLUETOOTH_CHANGED.store(true, Ordering::Relaxed);
    }
    if pending != 0 {
        EDGE.signal(());
    }
}

#[interrupt]
unsafe fn EXTI1() {
    taken(1 << 1);
}

#[interrupt]
unsafe fn EXTI9_5() {
    taken(0x03E0);
}

#[interrupt]
unsafe fn EXTI15_10() {
    taken(0xFC00);
}
