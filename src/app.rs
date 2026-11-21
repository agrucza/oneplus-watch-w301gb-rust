//! The application: bring the hardware up in a safe order, then run.

mod status;

use core::fmt::Write;

use embassy_time::{Duration, Instant};

use crate::board::Board;
use crate::boot::{faults, ticket, watchdog};
use crate::drivers::apollo::messages::{KeyEvent, Readings};
use crate::drivers::buttons::ButtonEvent;
use crate::drivers::cypress::Link;
use crate::drivers::{apollo, buttons, panel, psram};
use crate::hal::{clocks, sdmmc};
use crate::services::log::Log;
use crate::services::update::server::{Activity, Server};
use crate::services::{bluetooth, storage};

/// Volume the update package and the installed image file live on.
const IMAGE_VOLUME: usize = 1;
/// Volume with the log folder.
const LOG_VOLUME: usize = 3;

/// Returns only if the watch cannot be reached or served; the caller then
/// resets, which takes the watch back to the stock firmware.
pub async fn run(board: Board) {
    board.power.switch_on().await;
    let _ = ticket::report(ticket::STAGE_POWER, 0);

    // --- Storage and log ----------------------------------------------------
    watchdog::feed();
    if let Err(e) = sdmmc::init() {
        let _ = ticket::report(ticket::STAGE_INIT_FAILED, e as u8);
        return;
    }
    let Ok(log_partition) = storage::inspect(LOG_VOLUME) else {
        let _ = ticket::report(ticket::STAGE_LOG, 1);
        return;
    };
    let mut log = Log::start(log_partition);
    let _ = writeln!(log.stamp(), "--- w301 rust app: ota server ---");
    faults::write_reset_cause(log.stamp());
    clocks::log_state(&mut log);
    if let Err(step) = log.flush() {
        let _ = ticket::report(ticket::STAGE_LOG, step);
        return;
    }
    let _ = ticket::report(ticket::STAGE_LOG, 0); // log works; the rest follows

    let Ok(image_partition) = storage::inspect(IMAGE_VOLUME) else {
        let _ = writeln!(log.stamp(), "volume 1 not readable");
        let _ = log.flush();
        return;
    };

    // --- Bluetooth: become the watch the PC is paired with -----------------
    // If Bluetooth cannot be started there is no way to reach us, so fall
    // through and reset (back to the stock firmware).
    if let Some(link) = bluetooth::start(board.bluetooth, &mut log).await {
        let _ = log.flush();
        // Bluetooth first, then the Apollo: once released it may reset us
        // again, and by then the PC must be able to reach the watch.
        let apollo = apollo::Apollo::start(board.apollo).await;
        let _ = writeln!(log.stamp(), "apollo: {}", if apollo.is_some() { "released from reset" } else { "not started" });

        // External memory, then the display with its framebuffer in it.
        // Last, so that a problem here cannot keep Bluetooth from coming up.
        // Should the memory bus hang, the watchdog resets us - and since no
        // PC has connected to this boot yet, that reset returns to the stock
        // firmware instead of looping.
        let _ = log.flush();
        let mut screen = None;
        psram::init().await;
        if psram::quick_check(&mut log) {
            let _ = writeln!(log.stamp(), "psram: ready");
            // The 2 MB at FRAMEBUFFER belong to the display alone.
            let frame = unsafe { &mut *(psram::FRAMEBUFFER as *mut panel::Frame) };
            screen = panel::Panel::start(board.display, frame, &mut log).await.map(Screen::new);
        } else {
            let _ = writeln!(log.stamp(), "display: not started, no memory for the picture");
        }
        let _ = log.flush();
        let buttons = buttons::Buttons::start(board.upper_button);
        if let Some(server) = Server::new(image_partition) {
            main_loop(link, server, apollo, buttons, screen).await;
        }
    }

    match log.flush() {
        Ok(()) => {
            let _ = ticket::report(ticket::STAGE_LOG, 15);
        }
        Err(step) => {
            let _ = ticket::report(ticket::STAGE_LOG, step);
        }
    }
}

/// Brightness steps the lower button cycles through (panel values).
const BRIGHTNESS: [u8; 4] = [0x20, 0x60, 0x85, 0xFF];
/// A press of the upper button shorter than this is a click.
const CLICK_MS: u64 = 800;

/// The panel and what is currently shown on it.
struct Screen {
    panel: panel::Panel,
    visible: bool,
    brightness: usize,
    last_button: &'static str,
    /// Uptime second the screen was last drawn for.
    drawn_for: Option<u64>,
    refresh_logged: bool,
}

impl Screen {
    fn new(panel: panel::Panel) -> Self {
        status::clear(panel.frame);
        // 2 = the stock app's start-up brightness, which the panel is on now.
        Self { panel, visible: true, brightness: 2, last_button: "no button yet", drawn_for: None, refresh_logged: false }
    }

    /// React to a click: the upper button blanks and shows the picture,
    /// the lower one steps the brightness.
    async fn click(&mut self, upper: bool, log: &mut Log) {
        let taken = if upper {
            self.visible = !self.visible;
            self.last_button = "upper: click";
            let _ = writeln!(log.stamp(), "display: {}", if self.visible { "shown" } else { "blanked" });
            self.panel.set_visible(self.visible).await
        } else {
            self.brightness = (self.brightness + 1) % BRIGHTNESS.len();
            self.last_button = "lower: click";
            let _ = writeln!(log.stamp(), "display: brightness step {} of {}", self.brightness + 1, BRIGHTNESS.len());
            self.panel.set_brightness(BRIGHTNESS[self.brightness]).await
        };
        if !taken {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.drawn_for = None; // show the change at once
    }

    /// Redraw and send the picture if a second has passed or something
    /// changed. Takes about 40 ms when it does.
    async fn update(&mut self, readings: Option<&Readings>, bluetooth_connected: bool, log: &mut Log) {
        let uptime_s = Instant::now().as_secs();
        if !self.visible || self.drawn_for == Some(uptime_s) {
            return;
        }
        self.drawn_for = Some(uptime_s);
        status::draw(
            self.panel.frame,
            &status::Status {
                uptime_s,
                readings,
                bluetooth_connected,
                brightness: (self.brightness + 1, BRIGHTNESS.len()),
                last_button: self.last_button,
            },
        );
        let started = Instant::now();
        let r = self.panel.refresh().await;
        // The first refresh and every failed one go to the log.
        if !r.done || !self.refresh_logged {
            self.refresh_logged = true;
            let _ = writeln!(
                log.stamp(),
                "display: refresh {} after {} ms; WISR={:#010x} ISR0={:#010x} ISR1={:#010x} PSR={:#010x}",
                if r.done { "done" } else { "NOT finished" },
                started.elapsed().as_millis(),
                r.wisr,
                r.isr0,
                r.isr1,
                r.psr
            );
        }
    }
}

/// Everything that happens once the hardware is up. Each pass takes at most
/// about 20 ms unless a packet is being dealt with or the screen is sent.
/// Ends only in a reset: an installed update, a return to the stock
/// firmware, or the watchdog.
async fn main_loop(
    mut link: Link,
    mut server: Server,
    mut apollo: Option<apollo::Apollo>,
    mut buttons: buttons::Buttons,
    mut screen: Option<Screen>,
) -> ! {
    let mut log = Log::get();
    let mut last_save = Instant::now();
    loop {
        // The Apollo's handshake cannot wait long, so look after it on every
        // pass - but not while a package is streaming in.
        if !server.transferring() {
            if let Some(apollo) = apollo.as_mut() {
                apollo.service(&mut log).await;
            }
        }

        // Buttons: the upper one is ours, the lower one comes through the Apollo.
        let upper_click = matches!(buttons.poll(&mut log), Some(ButtonEvent::Released { held_ms }) if held_ms < CLICK_MS);
        let lower_click = apollo.as_mut().and_then(|a| a.take_key()) == Some(KeyEvent::Clicked);
        if let Some(screen) = screen.as_mut() {
            if upper_click {
                screen.click(true, &mut log).await;
            }
            if lower_click {
                screen.click(false, &mut log).await;
            }
            if !server.transferring() {
                screen.update(apollo.as_ref().map(|a| &a.readings), server.connected, &mut log).await;
            }
        }

        let activity = server.step(&mut link, &mut log).await;

        // Nothing is arriving: a good moment to save what was logged, so
        // that it survives if the app dies. Not during a transfer, where the
        // sender is waiting for us.
        if activity == Activity::Idle
            && !server.transferring()
            && log.pending()
            && last_save.elapsed() > Duration::from_secs(2)
        {
            let _ = log.flush();
            last_save = Instant::now();
        }
    }
}
