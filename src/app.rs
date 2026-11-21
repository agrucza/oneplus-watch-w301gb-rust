//! The application: bring the hardware up in a safe order, then run.

use core::fmt::Write;

use embassy_time::{Duration, Instant};

use crate::board::Board;
use crate::boot::{faults, ticket, watchdog};
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
        psram::init().await;
        if psram::quick_check(&mut log) {
            let _ = writeln!(log.stamp(), "psram: ready");
            // The 2 MB at FRAMEBUFFER belong to the display alone.
            let frame = unsafe { &mut *(psram::FRAMEBUFFER as *mut panel::Frame) };
            if let Some(mut panel) = panel::Panel::start(board.display, frame, &mut log).await {
                panel.test_picture();
                panel.refresh(&mut log).await;
                core::mem::forget(panel); // keep the panel powered
            }
        } else {
            let _ = writeln!(log.stamp(), "display: not started, no memory for the picture");
        }
        let _ = log.flush();
        let buttons = buttons::Buttons::start(board.upper_button);
        if let Some(server) = Server::new(image_partition) {
            main_loop(link, server, apollo, buttons).await;
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

/// Everything that happens once the hardware is up. Each pass takes at most
/// about 20 ms unless a packet is being dealt with. Ends only in a reset:
/// an installed update, a return to the stock firmware, or the watchdog.
async fn main_loop(mut link: Link, mut server: Server, mut apollo: Option<apollo::Apollo>, mut buttons: buttons::Buttons) -> ! {
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
        buttons.poll(&mut log);

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
