//! Whether the processor may go into its Stop mode.
//!
//! The HAL's executor (`embassy_stm32::executor`, feature `low-power`)
//! stops the processor when it has nothing to run for a while and no
//! peripheral it looks after is busy; the real-time clock wakes it for
//! the next thing that is due, and the HAL restores the clocks. What the
//! HAL cannot see are the drivers of this firmware that work on the
//! registers themselves. So Stop mode is forbidden here unless the
//! firmware has said that nothing of its own is under way.

use core::cell::RefCell;

use embassy_stm32::rcc::{StopMode, WakeGuard};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;

/// Held while stopping is forbidden; the HAL counts these.
static GUARD: Mutex<CriticalSectionRawMutex, RefCell<Option<WakeGuard>>> = Mutex::new(RefCell::new(None));

/// Forbid Stop mode from now on.
pub fn forbid() {
    GUARD.lock(|guard| {
        let mut guard = guard.borrow_mut();
        if guard.is_none() {
            *guard = Some(WakeGuard::new(StopMode::Stop1));
        }
    });
}

/// Allow Stop mode from now on, as far as the HAL's own peripherals let it.
#[allow(dead_code)] // nothing allows it yet
pub fn allow() {
    GUARD.lock(|guard| drop(guard.borrow_mut().take()));
}
