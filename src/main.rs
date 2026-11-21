//! OnePlus Watch W301 (STM32L4R9) - Rust application on the Embassy HAL.
//!
//! Loaded by the stock bootloader (0x0800_0000), which jumps to the vector
//! table at 0x0801_E000.
//!
//! There is no debug probe: images arrive over Bluetooth, through the stock
//! firmware's updater or this app's own (`ota_server`). Every boot starts
//! the same way:
//!
//!   1. first thing after boot, book the "return ticket" (ota_flag): the next
//!      reset makes the bootloader re-install the stock app from eMMC;
//!   2. start the watchdog, so a hang also ends in that reset;
//!   3. bring up storage and Bluetooth and serve firmware updates until one
//!      is installed or the PC asks for the stock firmware back.

#![no_std]
#![no_main]

mod apollo;
mod apollo_messages;
mod buttons;
mod clocks;
mod cypress;
mod display;
mod emmc;
mod flash;
mod log;
mod ota_flag;
mod ota_server;
mod package;
mod psram;
#[allow(dead_code)] // read helpers are kept for the upcoming OTA code
mod storage;
mod watchdog;

use core::panic::PanicInfo;

use cortex_m::peripheral::SCB;
use cortex_m_rt::{exception, ExceptionFrame};
use defmt_rtt as _; // log transport; harmless without a probe attached
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::rcc::{LsConfig, LseConfig, LseDrive, LseMode, RtcClockSource};
use embassy_stm32::time::Hertz;
use core::fmt::Write;
use log::Log;
use embassy_stm32::{Config, Peripherals};
use embassy_time::{Duration, Timer};

/// Image the bootloader restores when we leave.
const RETURN_TO: ota_flag::Source = ota_flag::Source::PacketA;
/// Watchdog period; a hang returns to the stock app after this long.
const WATCHDOG_SECS: u32 = 20;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // 1. Return ticket - before anything that could go wrong.
    let ticket = report(STAGE_BOOTED, 0);

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
    config.rcc.ls = adopt_low_speed_clocks();
    clocks::configure(&mut config);
    let p = embassy_stm32::init(config);

    match ticket {
        Ok(existing) => defmt::info!("return ticket booked (existing flag block: {})", existing),
        Err(e) => defmt::error!("return ticket FAILED: {}", e),
    }

    // 3. The experiment.
    spawner.spawn(watchdog_feeder().unwrap());
    experiment(p).await;
    Timer::after_millis(100).await;

    SCB::sys_reset();
}

// Report byte (travels in the bootloader log as `err=`): 0x80 | stage << 4 | detail.
// The ticket is rewritten before every risky step, so after a hang (watchdog
// reset) the log still shows the last stage that was reached.
const STAGE_BOOTED: u8 = 0; // nothing else done yet
const STAGE_POWER: u8 = 1; // power pins set, about to talk to the card
const STAGE_INIT_FAILED: u8 = 2; // detail = emmc::Error value
const STAGE_LOG: u8 = 7; // detail 0 = log works, experiment running (or hung); 1..=7 = log failed at step; 15 = finished, log written

fn report(stage: u8, detail: u8) -> Result<bool, flash::Error> {
    let byte = 0x80 | (stage & 0x7) << 4 | (detail & 0xF);
    ota_flag::request_install(RETURN_TO, byte).or_else(|_| ota_flag::request_install(RETURN_TO, byte))
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

/// Bring up storage and the log, then listen to the Bluetooth chip.
async fn experiment(p: Peripherals) {
    // --- bsp_gpio_power_init (bootloader): same pins, same levels, same order.
    // Which of these feeds the eMMC is not known. The bootloader resets
    // ports B, C and D before jumping to us, so at least PC3 is off again.
    embassy_stm32::pac::PWR.cr2().modify(|w| w.set_iosv(true)); // port G supply valid
    let pc3 = Output::new(p.PC3, Level::High, Speed::VeryHigh);
    let pf3 = Output::new(p.PF3, Level::High, Speed::VeryHigh);
    let pa1 = Output::new(p.PA1, Level::High, Speed::VeryHigh);
    let mut pg4 = Output::new(p.PG4, Level::High, Speed::VeryHigh);
    pg4.set_low(); // 100 ms low pulse: looks like a reset line
    Timer::after_millis(100).await;
    pg4.set_high();
    let pa12 = Output::new(p.PA12, Level::High, Speed::VeryHigh);
    core::mem::forget((pc3, pf3, pa1, pg4, pa12)); // stay driven
    // PG5, PF12, PF5 and PA11 from that list are the display's supplies,
    // power and reset; `display` drives them.

    Timer::after_millis(50).await;
    let _ = report(STAGE_POWER, 0);

    // --- Storage and log ----------------------------------------------------
    watchdog::feed();
    if let Err(e) = emmc::init() {
        let _ = report(STAGE_INIT_FAILED, e as u8);
        return;
    }
    let Ok(log_partition) = storage::inspect(3) else {
        let _ = report(STAGE_LOG, 1);
        return;
    };
    let Some(mut log) = Log::take() else {
        return;
    };
    let _ = writeln!(log.stamp(), "--- w301 rust app: ota server ---");
    log_reset_cause(&mut log);
    clocks::log_state(&mut log);
    if let Err(step) = log.flush(log_partition) {
        let _ = report(STAGE_LOG, step);
        return;
    }
    let _ = report(STAGE_LOG, 0); // log works; the experiment follows

    // --- Bluetooth: become the watch the PC is paired with -----------------
    let cypress = cypress::Resources {
        usart: p.USART3,
        tx: p.PC4,
        rx: p.PC5,
        cts: p.PB13,
        rts: p.PB14,
        power: p.PB2,
        reset: p.PC6,
        wake: p.PB9,
        irq: p.PC7,
        rx_dma: p.DMA1_CH1,
        tx_dma: p.DMA1_CH2,
    };
    let Ok(image_partition) = storage::inspect(1) else {
        let _ = writeln!(log.stamp(), "volume 1 not readable");
        let _ = log.flush(log_partition);
        return;
    };
    // Serve updates until one is installed or the PC asks for the stock
    // firmware. If Bluetooth could not be started there is no way to reach
    // us, so fall through and reset (back to the stock firmware).
    if let Some(mut link) = bluetooth_start(cypress, &mut log).await {
        let _ = log.flush(log_partition);
        // Bluetooth first, then the Apollo: once released it may reset us
        // again, and by then the PC must be able to reach the watch.
        let apollo = apollo::Apollo::start(apollo::Resources {
            request: p.PG1,
            ready: p.PA15,
            have_data: p.PF10,
            reset: p.PG12,
            pf7: p.PF7,
            _sck: p.PA5,
            _miso: p.PA6,
            _mosi: p.PA7,
        })
        .await;
        let _ = writeln!(log.stamp(), "apollo: {}", if apollo.is_some() { "released from reset" } else { "not started" });

        // External memory, then the display with its framebuffer in it.
        // Last, so that a problem here cannot keep Bluetooth from coming up.
        // Should the memory bus hang, the watchdog resets us - and since no
        // PC has connected to this boot yet, that reset returns to the stock
        // firmware instead of looping.
        let _ = log.flush(log_partition);
        psram::init().await;
        if psram::quick_check(&mut log) {
            let _ = writeln!(log.stamp(), "psram: ready");
            // The 2 MB at FRAMEBUFFER belong to the display alone.
            let frame = unsafe { &mut *(psram::FRAMEBUFFER as *mut [u16; display::WIDTH * display::HEIGHT]) };
            let display = display::Display::start(
                display::Resources {
                    supply_pg5: p.PG5,
                    supply_pf12: p.PF12,
                    power: p.PF5,
                    reset: p.PA11,
                    _te: p.PF11,
                    _ltdc: p.LTDC,
                    _dsi: p.DSIHOST,
                },
                frame,
                &mut log,
            )
            .await;
            if let Some(mut display) = display {
                display.test_picture();
                display.refresh(&mut log).await;
                core::mem::forget(display); // keep the panel powered
            }
        } else {
            let _ = writeln!(log.stamp(), "display: not started, no memory for the picture");
        }
        let _ = log.flush(log_partition);
        let buttons = buttons::Buttons::start(p.PC13);
        ota_server::run(&mut link, &mut log, apollo, buttons, image_partition, log_partition).await;
    }

    match log.flush(log_partition) {
        Ok(()) => {
            let _ = report(STAGE_LOG, 15);
        }
        Err(step) => {
            let _ = report(STAGE_LOG, step);
        }
    }
}

/// Log why the processor was last reset, then clear the record so the next
/// boot shows only its own cause. Every reset sets "pin"; the other flags
/// say what pulled it. "pin" alone means the reset line was driven from
/// outside the processor.
fn log_reset_cause(log: &mut Log) {
    const RCC_CSR: *mut u32 = 0x4002_1094 as *mut u32;
    let csr = unsafe { RCC_CSR.read_volatile() };
    unsafe { RCC_CSR.write_volatile(csr | 1 << 23) }; // RMVF: clear the flags
    let _ = write!(log.stamp(), "reset cause:");
    let names = [
        (31, " low-power"),
        (30, " window-watchdog"),
        (29, " watchdog"),
        (28, " software"),
        (27, " brown-out"),
        (26, " pin"),
        (25, " option-bytes"),
        (24, " firewall"),
    ];
    for (bit, name) in names {
        if csr & 1 << bit != 0 {
            let _ = log.write_str(name);
        }
    }
    let _ = writeln!(log, " (RCC_CSR={:#010x})", csr);
}

/// The watch's Bluetooth address. The chip starts on a default address;
/// the stock app sets this one from its factory data (not located yet).
/// Here it comes from `OPW_MAC` at build time (see `build.rs`), because it
/// is personal to one watch and does not belong in the sources.
const BLUETOOTH_ADDRESS: [u8; 6] = parse_address(env!("W301_BT_ADDRESS"));

/// Parse "AA:BB:CC:DD:EE:FF" (already validated by `build.rs`).
const fn parse_address(text: &str) -> [u8; 6] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex digit in the Bluetooth address"),
        }
    }
    let t = text.as_bytes();
    assert!(t.len() == 17, "Bluetooth address must look like AA:BB:CC:DD:EE:FF");
    let mut out = [0u8; 6];
    let mut i = 0;
    while i < 6 {
        out[i] = nibble(t[i * 3]) << 4 | nibble(t[i * 3 + 1]);
        i += 1;
    }
    out
}

/// Stored pairings: `2:bluethooth_pair.cfg`, a raw copy of the stock app's
/// table of nine 175-byte slots (loaded at 0x0802AEEE). A slot is in use when
/// its first two bytes (the record id) are not zero; the stock app pushes
/// the id and the following 136 bytes to the chip (0x0802BB8A).
const PAIR_FILE: &str = "bluethooth_pair.cfg";
const PAIR_FILE_LEN: usize = 1575;
const PAIR_SLOT_LEN: usize = 175;
const PAIR_RECORD_LEN: usize = 2 + 136;

/// Start the chip, give it the watch's address and the stored pairing keys,
/// and make it connectable. Returns the link only if all of that worked.
///
/// The order matters: answering on the watch's address without the keys
/// would make the PC's connection fail authentication, which could cost the
/// PC its pairing with the stock firmware. So every step is checked and the
/// chip only becomes connectable if all of them worked.
async fn bluetooth_start(r: cypress::Resources, log: &mut Log) -> Option<cypress::Link> {
    let Some(mut link) = cypress::Link::power_up(r).await else {
        let _ = writeln!(log.stamp(), "cypress: could not open the link");
        return None;
    };
    let _ = writeln!(log.stamp(), "cypress: powered up");

    // 1. Wait for the chip's start-up packet.
    let started = loop {
        match link.receive(Duration::from_secs(3)).await {
            Some(p) if p.opcode == cypress::EVT_DEVICE_STARTED => break true,
            Some(p) => cypress::log_packet(log, &p),
            None => break false,
        }
    };
    if !started {
        let _ = writeln!(log.stamp(), "cypress: no start-up packet");
        return None;
    }
    let _ = writeln!(log.stamp(), "cypress: started");

    let connectable = 'setup: {
        // 2. Address: set it (last byte first), then read it back.
        let mut reversed = BLUETOOTH_ADDRESS;
        reversed.reverse();
        if !link.command(log, cypress::CMD_SET_LOCAL_BDA, &reversed).await {
            break 'setup false;
        }
        match link.request(log, cypress::CMD_READ_LOCAL_BDA, &[], cypress::EVT_LOCAL_BDA).await {
            Some((address, 6)) if address[..6] == BLUETOOTH_ADDRESS => {
                let _ = writeln!(log.stamp(), "cypress: address set and read back correctly");
            }
            Some((address, n)) => {
                let _ = writeln!(log.stamp(), "cypress: address reads back differently:");
                log.hex(&address[..n]);
                break 'setup false;
            }
            None => break 'setup false,
        }

        // 3. Pairing keys from the stock app's file on volume 2.
        let Some(table) = cortex_m::singleton!(: [u8; PAIR_FILE_LEN] = [0; PAIR_FILE_LEN]) else {
            break 'setup false;
        };
        let read = storage::inspect(2).map_err(|_| 1u8).and_then(|p| storage::read_root_file(p, PAIR_FILE, table));
        match read {
            Ok(PAIR_FILE_LEN) => {}
            Ok(len) => {
                let _ = writeln!(log.stamp(), "pairing file has {} bytes, expected {}", len, PAIR_FILE_LEN);
                break 'setup false;
            }
            Err(step) => {
                let _ = writeln!(log.stamp(), "pairing file could not be read (step {})", step);
                break 'setup false;
            }
        }
        let mut pushed = 0;
        for (i, slot) in table.chunks_exact(PAIR_SLOT_LEN).enumerate() {
            if slot[0] == 0 && slot[1] == 0 {
                continue;
            }
            // Log who the record is for, never the key itself.
            let _ = writeln!(
                log.stamp(),
                "pairing slot {}: id {:#06x}, peer {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                i,
                u16::from_le_bytes([slot[0], slot[1]]),
                slot[2], slot[3], slot[4], slot[5], slot[6], slot[7]
            );
            if !link.command(log, cypress::CMD_PUSH_NVRAM_DATA, &slot[..PAIR_RECORD_LEN]).await {
                break 'setup false;
            }
            pushed += 1;
        }
        let _ = writeln!(log.stamp(), "cypress: {} pairing records accepted", pushed);
        if pushed == 0 {
            break 'setup false;
        }

        // 4. Connectable for already paired devices, not discoverable.
        link.command(log, cypress::CMD_SET_VISIBILITY, &[0, 1]).await
    };
    let _ = writeln!(
        log.stamp(),
        "cypress: {}",
        if connectable { "connectable as the watch" } else { "NOT made connectable" }
    );

    connectable.then_some(link)
}

/// Describe the RTC/LSE setup that is already running, instead of choosing one.
///
/// The HAL resets the whole backup domain (RTC time and backup registers) when
/// RCC_BDCR differs from its configuration. The first round trip did exactly
/// that: the watch came back with its clock at 2000-01-01. Reading BDCR and
/// handing the HAL the same settings leaves the stock firmware's RTC alone.
fn adopt_low_speed_clocks() -> LsConfig {
    const RCC_BDCR: *const u32 = 0x4002_1090 as *const u32;
    let bdcr = unsafe { core::ptr::read_volatile(RCC_BDCR) };
    let lse_on = bdcr & 1 != 0;
    let lse_bypass = bdcr & (1 << 2) != 0;
    let lse_drive = match (bdcr >> 3) & 0b11 {
        0 => LseDrive::Low,
        1 => LseDrive::MediumLow,
        2 => LseDrive::MediumHigh,
        _ => LseDrive::High,
    };
    let rtc_enabled = bdcr & (1 << 15) != 0;
    let rtc = match (bdcr >> 8) & 0b11 {
        1 if rtc_enabled && lse_on => RtcClockSource::LSE,
        2 if rtc_enabled => RtcClockSource::LSI,
        _ => RtcClockSource::DISABLE,
    };
    LsConfig {
        rtc,
        lsi: true, // the watchdog runs from LSI anyway
        lse: lse_on.then_some(LseConfig {
            frequency: Hertz(32_768),
            mode: if lse_bypass { LseMode::Bypass } else { LseMode::Oscillator(lse_drive) },
        }),
    }
}

// Without a debugger every failure must end in a reset, never in a hang:
// the reset is what takes us back to the stock firmware. Before resetting,
// each handler rewrites the return ticket with its own report byte, so the
// bootloader log (`err=` in 3:log/ota.log) shows how the app ended:
//   142 panic, 143 hard fault, 141 unexpected interrupt.
// A watchdog reset or a power loss leaves the last regular report (240).

fn leave(report_byte: u8) -> ! {
    let _ = ota_flag::request_install(RETURN_TO, report_byte);
    SCB::sys_reset()
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    leave(0x8E)
}

#[exception]
unsafe fn HardFault(_ef: &ExceptionFrame) -> ! {
    leave(0x8F)
}

#[exception]
unsafe fn DefaultHandler(_irqn: i16) {
    leave(0x8D)
}
