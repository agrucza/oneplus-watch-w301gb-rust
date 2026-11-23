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
use core::fmt;

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
pub fn allow() {
    GUARD.lock(|guard| drop(guard.borrow_mut().take()));
}

/// What the HAL would do if the processor had nothing to run now, for
/// the log: "Stop 2", "Stop 1" (something it manages needs more than
/// Stop 2 keeps) or "no stop".
pub fn write_outlook(out: &mut impl fmt::Write) {
    let mode = critical_section::with(embassy_stm32::rcc::get_stop_mode);
    let _ = out.write_str(match mode {
        Some(StopMode::Stop2) | Some(StopMode::Standby) => "Stop 2",
        Some(StopMode::Stop1) => "Stop 1",
        None => "no stop",
    });
}
