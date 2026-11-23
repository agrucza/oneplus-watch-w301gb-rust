//! What the firmware's two tasks tell each other.
//!
//! The main task looks after the Apollo, Bluetooth, the buttons, the
//! sensors and the log file; the display task reads the touch controller
//! and draws. They run in turn on one executor: whichever is waiting for
//! something lets the other one work. Neither holds any of the other's
//! hardware, so all they share is here:
//!
//! - `State`: what the main task knows that the display shows. The main
//!   task puts a fresh copy here on every pass; the display task takes a
//!   copy when it draws.
//! - Events for the screen (clicks, wrist gestures, light readings), in a
//!   short queue. A full queue drops the newest event.
//! - Whether the picture is visible, for the main task to tell the Apollo.
//! - A request to switch the watch off, which the main task passes to the
//!   Apollo.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::channel::Channel;

use super::controls::Action;
use crate::drivers::apollo::messages::{Readings, WristGesture};

#[derive(Clone, Copy)]
pub struct State {
    /// The Apollo's latest sensor values; `None` without an Apollo.
    pub readings: Option<Readings>,
    /// The last light reading, in lux.
    pub lux: Option<u32>,
    pub bluetooth_connected: bool,
    /// A heart-rate measurement is running.
    pub measuring_heart: bool,
    /// A package is streaming in over Bluetooth: the display task keeps
    /// still, so that the transfer has the processor to itself.
    pub transferring: bool,
    /// A reset comes back to this firmware (`Server::staying`).
    pub stays: bool,
}

#[derive(Clone, Copy)]
pub enum Event {
    /// A button was pressed, and this is what that press does
    /// (`controls::Bindings`).
    Act(Action),
    Wrist(WristGesture),
    /// A new light reading, in lux.
    Light(u32),
}

static STATE: Mutex<CriticalSectionRawMutex, Cell<State>> = Mutex::new(Cell::new(State {
    readings: None,
    lux: None,
    bluetooth_connected: false,
    measuring_heart: false,
    transferring: false,
    stays: false,
}));
static EVENTS: Channel<CriticalSectionRawMutex, Event, 8> = Channel::new();
static VISIBLE: AtomicBool = AtomicBool::new(true);
static POWER_OFF: AtomicBool = AtomicBool::new(false);

pub fn set_state(state: State) {
    STATE.lock(|cell| cell.set(state));
}

pub fn state() -> State {
    STATE.lock(|cell| cell.get())
}

/// Hand an event to the display task.
pub fn send(event: Event) {
    let _ = EVENTS.try_send(event);
}

/// The next event for the screen, if there is one.
pub fn next_event() -> Option<Event> {
    EVENTS.try_receive().ok()
}

pub fn set_visible(visible: bool) {
    VISIBLE.store(visible, Ordering::Relaxed);
}

/// Whether the picture is visible (not blanked).
pub fn visible() -> bool {
    VISIBLE.load(Ordering::Relaxed)
}

/// Ask the main task to have the Apollo switch the watch off.
pub fn request_power_off() {
    POWER_OFF.store(true, Ordering::Relaxed);
}

/// Whether switching off was asked for since the last call.
pub fn take_power_off() -> bool {
    POWER_OFF.swap(false, Ordering::Relaxed)
}
