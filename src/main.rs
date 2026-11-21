//! OnePlus Watch W301 (STM32L4R9) - Rust application on the Embassy HAL.
//!
//! Loaded by the stock bootloader (0x0800_0000), which jumps to the vector
//! table at 0x0801_E000.
//!
//! There is no debug probe: images arrive over Bluetooth, through the stock
//! firmware's updater or this app's own (`services::update`). Every boot
//! starts the same way:
//!
//!   1. first thing after boot, book the "return ticket" (`boot::ticket`):
//!      the next reset makes the bootloader re-install the stock app from
//!      eMMC;
//!   2. start the watchdog, so a hang also ends in that reset;
//!   3. run the application (`app`).
//!
//! Layers, each using only the ones before it: `hal` (the STM32 where the
//! HAL has gaps), `drivers` (one module per chip), `services` (what the
//! firmware does with them), `app`. `boot` stands beside them and depends on
//! nothing; `board` is the one place that assigns pins.

#![no_std]
#![no_main]

mod app;
mod board;
mod boot;
mod drivers;
mod hal;
mod services;

use cortex_m::peripheral::SCB;
use defmt_rtt as _; // log transport; harmless without a probe attached
use embassy_executor::Spawner;
use embassy_stm32::Config;
use embassy_time::Timer;

use boot::{ticket, watchdog};

/// Watchdog period; a hang returns to the stock app after this long.
const WATCHDOG_SECS: u32 = 20;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // 1. Return ticket - before anything that could go wrong.
    let booked = ticket::report(ticket::STAGE_BOOTED, 0);

    // 2. Dead-man switch - also before the HAL, so a hang in its start-up
    //    code is covered too.
    watchdog::start(WATCHDOG_SECS);
    // Freeze the watchdog while a debugger has the core halted
    // (DBGMCU_APB1FZR1.DBG_IWDG_STOP), so attaching does not end in a reset.
    unsafe {
        const DBGMCU_APB1FZR1: *mut u32 = 0xE004_2008 as *mut u32;
        DBGMCU_APB1FZR1.write_volatile(DBGMCU_APB1FZR1.read_volatile() | 1 << 12);
    }

    let mut config = Config::default();
    config.rcc.ls = hal::clocks::adopt_low_speed_clocks();
    hal::clocks::configure(&mut config);
    let p = embassy_stm32::init(config);

    match booked {
        Ok(existing) => defmt::info!("return ticket booked (existing flag block: {})", existing),
        Err(e) => defmt::error!("return ticket FAILED: {}", e),
    }

    // 3. The application.
    spawner.spawn(watchdog_feeder().unwrap());
    app::run(board::Board::new(p)).await;
    Timer::after_millis(100).await;

    SCB::sys_reset();
}

/// Feeds the watchdog for as long as the executor is alive. If the app hangs
/// in a busy loop, this task no longer runs and the watchdog resets the
/// watch - which, with the return ticket booked, brings the stock firmware
/// back.
#[embassy_executor::task]
async fn watchdog_feeder() {
    loop {
        watchdog::feed();
        Timer::after_secs(2).await;
    }
}
