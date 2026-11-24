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
//! - The settings: read from their file before the tasks start, changed
//!   by the display task, written back to the file by the main task.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::channel::Channel;

use super::controls::Action;
use super::settings::Settings;
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

/// Wait until there is an event for the screen, `limit` at most.
pub async fn wait_event(limit: embassy_time::Duration) {
    let _ = embassy_time::with_timeout(limit, EVENTS.ready_to_receive()).await;
}

/// The next event for the screen, if there is one.
pub fn next_event() -> Option<Event> {
    EVENTS.try_receive().ok()
}

pub fn set_visible(visible: bool) {
    if VISIBLE.swap(visible, Ordering::Relaxed) != visible {
        // The main task may be waiting with the processor allowed to
        // stop; a lit display ends that.
        crate::hal::wake::nudge();
    }
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

static SETTINGS: Mutex<CriticalSectionRawMutex, Cell<Settings>> = Mutex::new(Cell::new(Settings::START));
static SETTINGS_CHANGED: AtomicBool = AtomicBool::new(false);

/// The settings as they are now.
pub fn settings() -> Settings {
    SETTINGS.lock(|cell| cell.get())
}

/// Take the settings read from the file at start.
pub fn load_settings(settings: Settings) {
    SETTINGS.lock(|cell| cell.set(settings));
}

/// Change the settings; the main task writes them to their file.
pub fn change_settings(settings: Settings) {
    SETTINGS.lock(|cell| cell.set(settings));
    SETTINGS_CHANGED.store(true, Ordering::Relaxed);
    // The main task may be waiting for a long while.
    crate::hal::wake::nudge();
}

/// The settings, if they changed since the last call.
pub fn take_changed_settings() -> Option<Settings> {
    SETTINGS_CHANGED.swap(false, Ordering::Relaxed).then(settings)
}
