//! The application: bring the hardware up in a safe order, then run.

mod alerts;
mod controls;
mod face;
mod launcher;
mod menu;
mod motion;
mod pages;
mod picker;
mod panels;
mod power;
mod settings;
mod shared;
mod status;
mod theme;

use core::fmt::Write;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};

use crate::board::Board;
use crate::boot::{faults, ticket, watchdog};
use crate::drivers::apollo::messages::{KeyEvent, Readings, WristGesture};
use crate::drivers::cypress::Link;
use crate::services::gestures::{Gesture, Gestures};
use controls::{Action, Bindings, Button, Hold, Press, Taps};
use settings::Settings;
use motion::Motion;
use pages::Page;

// The pages share out exactly the frames the panel has.
const _: () = assert!(pages::FRAMES == panel::FRAMES);
use crate::drivers::{apollo, buttons, light, panel, psram, touch};
use crate::hal::{clocks, sdmmc};
use crate::services::log::Log;
use crate::services::update::server::{Activity, Server};
use crate::services::brightness::AutoBrightness;
use crate::services::damage::Damage;
use crate::services::{bluetooth, clock, factory, storage};

/// Volume the update package and the installed image file live on.
const IMAGE_VOLUME: usize = 1;
/// Volume with the log folder.
const LOG_VOLUME: usize = 3;

/// Returns only if the watch cannot be reached or served; the caller then
/// resets, which takes the watch back to the stock firmware.
pub async fn run(spawner: Spawner, board: Board) {
    crate::boot::stack::paint();
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
    let rotated = rotate_log(log_partition);
    let mut log = Log::start(log_partition);
    let _ = writeln!(log.stamp(), "--- w301 rust app: ota server ---");
    shared::load_settings(settings_load(log_partition, &mut log));
    match rotated {
        Some(Ok(len)) => {
            let _ = writeln!(log.stamp(), "log: the previous log ({} bytes) is now log/rust.old; this file starts afresh", len);
        }
        Some(Err(step)) => {
            let _ = writeln!(log.stamp(), "log: could not move the old log aside (step {}); it stays as it is", step);
        }
        None => {}
    }
    faults::write_reset_cause(log.stamp());
    clocks::log_state(&mut log);
    clock::start(&mut log);
    if let Err(step) = log.flush() {
        let _ = ticket::report(ticket::STAGE_LOG, step);
        return;
    }
    let _ = ticket::report(ticket::STAGE_LOG, 0); // log works; the rest follows

    if EMMC_HIGH_SPEED || EMMC_FOUR_LINES {
        if !speed_up_card(&mut log) {
            // No storage: leave, which resets and returns to the stock
            // firmware, as for a card that never started.
            let _ = ticket::report(ticket::STAGE_INIT_FAILED, 14);
            return;
        }
        let _ = log.flush();
    }

    let Ok(image_partition) = storage::inspect(IMAGE_VOLUME) else {
        let _ = writeln!(log.stamp(), "volume 1 not readable");
        let _ = log.flush();
        return;
    };
    if !EMMC_TIMED_FILE.is_empty() {
        time_read_ahead(image_partition, &mut log);
        let _ = log.flush();
    }
    if EMMC_TIMED_WRITE != 0 {
        // The same file twice: every sector on its own, then with
        // consecutive sectors collected into one command.
        storage::set_write_collecting(false);
        let _ = writeln!(log.stamp(), "storage: write timing, one sector per command");
        time_file_write(log_partition, &mut log);
        storage::set_write_collecting(true);
        let _ = writeln!(log.stamp(), "storage: write timing, consecutive sectors collected");
        if !time_file_write(log_partition, &mut log) {
            storage::set_write_collecting(false);
            let _ = writeln!(log.stamp(), "storage: collecting writes switched off");
        }
        let _ = log.flush();
    }

    // --- Bluetooth: become the watch the PC is paired with -----------------
    // If Bluetooth cannot be started there is no way to reach us, so fall
    // through and reset (back to the stock firmware).
    if let Some(link) = bluetooth::start(board.bluetooth, &mut log).await {
        let _ = log.flush();
        // Bluetooth first, then the Apollo: once released it may reset us
        // again, and by then the PC must be able to reach the watch.
        // The board revision, measured once as in the stock app; the Apollo
        // asks for it.
        // The board revision, measured as the stock app does; the Apollo
        // asks for it after its start.
        let _ = log.flush();
        let (revision, reading) = board.revision.measure();
        let _ = writeln!(log.stamp(), "board revision {} (reading {} of 4095, {} mV)", revision, reading, reading as u32 * 1800 / 4096);
        if board.codec_supply.off(revision) {
            let _ = writeln!(log.stamp(), "audio codec supply (PB1) held off");
        }
        let _ = log.flush();
        let mut apollo = apollo::Apollo::start(board.apollo).await;
        if let Some(apollo) = apollo.as_mut() {
            apollo.set_revision(revision);
        }
        let _ = writeln!(log.stamp(), "apollo: {}", if apollo.is_some() { "released from reset" } else { "not started" });

        // External memory, then the display with its framebuffer in it.
        // Last, so that a problem here cannot keep Bluetooth from coming up.
        // Should the memory bus hang, the watchdog resets us - and since no
        // PC has connected to this boot yet, that reset returns to the stock
        // firmware instead of looping.
        let _ = log.flush();
        let mut has_screen = false;
        psram::init().await;
        if PSRAM_FULL_TEST {
            // Before the display uses the memory: the test overwrites it all.
            psram::full_test(&mut log);
            let _ = log.flush();
        }
        if psram::quick_check(&mut log) {
            let _ = writeln!(log.stamp(), "psram: ready");
            // The 2 MB at FRAMEBUFFER belong to the display alone.
            let frames: [&'static mut panel::Frame; panel::FRAMES] =
                core::array::from_fn(|n| unsafe { &mut *((psram::FRAMEBUFFER + n * psram::FRAME_SPACING) as *mut panel::Frame) });
            let panel = panel::Panel::start(board.display, frames, &mut log).await;
            // The display has a task of its own from here on, and the
            // touch controller goes with it (see `display_task`).
            let touch = touch::Touch::start(board.touch, &mut log).await;
            if let Some(panel) = panel {
                has_screen = true;
                spawner.spawn(display_task(Screen::new(panel), touch).unwrap());
            }
        } else {
            let _ = writeln!(log.stamp(), "display: not started, no memory for the picture");
        }
        let _ = log.flush();
        // Sensor bus and light sensor. Nothing depends on them.
        let mut bus = board.sensor_bus.open();
        light::scan(&mut bus, &mut log);
        let factor = factory::light_calibration(&mut log).unwrap_or_else(|| {
            let _ = writeln!(log.stamp(), "light sensor: using the stock default factor {}", light::DEFAULT_FACTOR);
            light::DEFAULT_FACTOR
        });
        let light = light::LightSensor::start(&mut bus, factor, &mut log).await;
        let _ = log.flush();
        let sensors = Sensors {
            bus,
            light,
            last_raw: None,
            last_lux: None,
            ready_at: Instant::now() + LIGHT_FIRST_READING,
            read_at: None,
            logged_for: None,
            logged: 0,
            brightest: 0,
        };
        let buttons = buttons::Buttons::start(board.upper_button);
        if let Some(server) = Server::new(image_partition) {
            main_loop(link, server, apollo, buttons, has_screen, sensors).await;
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

/// Fixed brightness steps (panel values). The lower button goes from
/// automatic through these and back to automatic.
const BRIGHTNESS: [u8; 4] = [0x20, 0x60, 0x85, 0xFF];
/// What the stock app's panel set-up leaves the brightness at.
const START_BRIGHTNESS: u8 = 0x85;
/// Automatic brightness moves the panel by at most `FADE_STEP` every
/// `FADE_INTERVAL`: from darkest to brightest in about two seconds.
const FADE_STEP: u8 = 4;
const FADE_INTERVAL: Duration = Duration::from_millis(30);

/// Run the card at 48 MHz instead of 12 (`sdmmc::high_speed`).
const EMMC_HIGH_SPEED: bool = true;
/// Run the card on four data lines instead of one (`sdmmc::four_data_lines`).
const EMMC_FOUR_LINES: bool = true;
/// Diagnosis: time reading this many sectors from the card at start, before
/// and after each change; 0 = no timing (each change is then still checked
/// by reading eight sectors before and after). Measured with 2048 sectors:
/// 1049 ms at 12 MHz on one data line, 443 ms at 48 MHz, 312 ms on four
/// data lines, 64 ms with 64 sectors per command.
const EMMC_TIMED_SECTORS: u32 = 0;

/// Read `EMMC_TIMED_SECTORS` sectors from the start of the card; returns
/// the milliseconds it took and a sum over the data.
fn time_card_read() -> Result<(u64, u32), sdmmc::Error> {
    let mut buffer = [0u32; 128];
    let mut sum = 0u32;
    let start = Instant::now();
    for sector in 0..EMMC_TIMED_SECTORS.max(8) {
        sdmmc::read_sector(sector, &mut buffer)?;
        sum = buffer.iter().fold(sum, |s, &w| s.rotate_left(1) ^ w);
        if sector % 256 == 0 {
            watchdog::feed();
        }
    }
    Ok((start.elapsed().as_millis(), sum))
}

/// Diagnosis: a large file to read through the filesystem at start, once
/// without and once with reading ahead; empty = no timing. Measured with
/// `packetA/stm32/stm32_app.bin` (1,974,264 bytes): 944 ms sector by
/// sector, 335 ms reading ahead.
const EMMC_TIMED_FILE: &str = "";

/// Read the whole of `EMMC_TIMED_FILE`; returns its length, a sum over its
/// bytes and the milliseconds it took.
fn time_file_read(partition: storage::Partition) -> Result<(u32, u32, u64), u8> {
    let mut reader = storage::FileReader::open(partition, EMMC_TIMED_FILE)?;
    let mut buffer = [0u8; 2048];
    let (mut total, mut sum) = (0u32, 0u32);
    let start = Instant::now();
    loop {
        let n = reader.read(&mut buffer).map_err(|()| 6u8)?;
        if n == 0 {
            break;
        }
        sum = buffer[..n].iter().fold(sum, |s, &b| s.rotate_left(1) ^ b as u32);
        total += n as u32;
        watchdog::feed();
    }
    reader.close();
    Ok((total, sum, start.elapsed().as_millis()))
}

/// Diagnosis: time the same file without and with reading ahead and
/// compare what was read.
fn time_read_ahead(partition: storage::Partition, log: &mut Log) {
    storage::set_read_ahead(false);
    let plain = time_file_read(partition);
    storage::set_read_ahead(true);
    let ahead = time_file_read(partition);
    match (plain, ahead) {
        (Ok((len, sum, ms)), Ok((len_ahead, sum_ahead, ms_ahead))) => {
            let _ = writeln!(
                log.stamp(),
                "storage: {} bytes of {} read in {} ms sector by sector, {} ms reading ahead, data {}",
                len,
                EMMC_TIMED_FILE,
                ms,
                ms_ahead,
                if len == len_ahead && sum == sum_ahead { "identical" } else { "DIFFERENT" }
            );
            if len != len_ahead || sum != sum_ahead {
                storage::set_read_ahead(false);
                let _ = writeln!(log.stamp(), "storage: reading ahead switched off");
            }
        }
        (plain, ahead) => {
            let _ = writeln!(log.stamp(), "storage: file timing failed (plain {:?}, ahead {:?})", plain.err(), ahead.err());
        }
    }
}

/// Diagnosis: write a scratch file of this many bytes to the log volume at
/// start, time it, read it back and empty it again; 0 = no write timing.
/// Measured with 256 KB: 397 ms one sector per command, 100 ms with
/// consecutive sectors collected.
const EMMC_TIMED_WRITE: usize = 0;

/// Diagnosis: how fast writing through the filesystem is, and in what
/// pattern the filesystem crate writes its sectors. True if the file was
/// written and read back identical.
fn time_file_write(partition: storage::Partition, log: &mut Log) -> bool {
    const DIR: &str = "LOG";
    const NAME: &str = "WTEST.BIN";
    static mut PIECE: [u8; 16384] = [0; 16384];
    // Safety: used only here, from the one task that starts the card.
    let piece = unsafe { &mut *core::ptr::addr_of_mut!(PIECE) };
    for (i, b) in piece.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(2654435761).to_le_bytes()[3];
    }
    // The file has to exist before it can be rewritten.
    if let Err(step) = storage::append(partition, DIR, NAME, &[]) {
        let _ = writeln!(log.stamp(), "storage: scratch file could not be made (step {})", step);
        return false;
    }
    let _ = storage::take_write_stats();
    let start = Instant::now();
    let written = (|| {
        let mut writer = storage::FileWriter::rewrite(partition, &[DIR], NAME).map_err(|_| ())?;
        for _ in 0..EMMC_TIMED_WRITE / piece.len() {
            writer.write(piece)?;
            watchdog::feed();
        }
        writer.finish()
    })();
    let ms = start.elapsed().as_millis();
    let stats = storage::take_write_stats();
    let _ = writeln!(
        log.stamp(),
        "storage: {} bytes written in {} ms ({}); {} sector writes, {} right after the one before, longest run {}",
        EMMC_TIMED_WRITE,
        ms,
        if written.is_ok() { "ok" } else { "FAILED" },
        stats.sectors,
        stats.in_sequence,
        stats.longest_run
    );
    // Read it back and compare.
    let mut same = true;
    let mut total = 0usize;
    match storage::FileReader::open(partition, "log/WTEST.BIN") {
        Ok(mut reader) => {
            let mut buffer = [0u8; 2048];
            loop {
                let Ok(n) = reader.read(&mut buffer) else {
                    same = false;
                    break;
                };
                if n == 0 {
                    break;
                }
                same &= buffer[..n] == piece[total % piece.len()..][..n];
                total += n;
                watchdog::feed();
            }
            reader.close();
        }
        Err(step) => {
            let _ = writeln!(log.stamp(), "storage: scratch file could not be opened (step {})", step);
            same = false;
        }
    }
    let _ = writeln!(
        log.stamp(),
        "storage: {} bytes read back, {}",
        total,
        if same && total == EMMC_TIMED_WRITE { "identical" } else { "DIFFERENT" }
    );
    // Leave it empty.
    let emptied = storage::FileWriter::rewrite(partition, &[DIR], NAME).map_err(|_| ()).and_then(|w| w.finish());
    if emptied.is_err() {
        let _ = writeln!(log.stamp(), "storage: scratch file could not be emptied");
    }
    written.is_ok() && same && total == EMMC_TIMED_WRITE && emptied.is_ok()
}

/// Sectors per command in the multi-sector timing.
const EMMC_CHUNK: usize = 64;

/// Diagnosis: read the timed sectors again, `EMMC_CHUNK` per command, and
/// compare with what the single reads delivered. False if a read failed:
/// the card may then be stuck in the middle of a transfer.
fn time_card_read_chunked(reference: u32, log: &mut Log) -> bool {
    static mut BUFFER: [u32; EMMC_CHUNK * 128] = [0; EMMC_CHUNK * 128];
    // Safety: used only here, from the one task that starts the card.
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(BUFFER) };
    let mut sum = 0u32;
    let start = Instant::now();
    let mut sector = 0;
    while sector < EMMC_TIMED_SECTORS {
        if let Err(e) = sdmmc::read_sectors(sector, buffer) {
            let _ = writeln!(log.stamp(), "emmc: {} sectors per command failed at sector {} (error {})", EMMC_CHUNK, sector, e as u8);
            return false;
        }
        sum = buffer.iter().fold(sum, |s, &w| s.rotate_left(1) ^ w);
        sector += EMMC_CHUNK as u32;
        watchdog::feed();
    }
    let _ = writeln!(
        log.stamp(),
        "emmc: {} sectors per command: read in {} ms, data {}",
        EMMC_CHUNK,
        start.elapsed().as_millis(),
        if sum == reference { "identical" } else { "DIFFERENT" }
    );
    true
}

/// Apply one change to the card's bus and check it: the same sectors are
/// read before and after, timed, and compared. False if the change failed
/// or the data differs; the card is then in an unknown state.
fn card_step(what: &str, reference: u32, change: fn() -> Result<(), sdmmc::Error>, log: &mut Log) -> bool {
    match change().and_then(|()| time_card_read()) {
        Ok((ms, sum)) => {
            let same = sum == reference;
            if EMMC_TIMED_SECTORS != 0 || !same {
                let _ = writeln!(log.stamp(), "emmc: {}: read in {} ms, data {}", what, ms, if same { "identical" } else { "DIFFERENT" });
            }
            same
        }
        Err(e) => {
            let _ = writeln!(log.stamp(), "emmc: {} failed (error {})", what, e as u8);
            false
        }
    }
}

/// Bring the card's bus up to the stock firmware's speed, one step at a
/// time. A step that fails is taken back by starting the card again and
/// repeating the steps that worked. False only if the card could not be
/// brought back: then there is no storage and the caller must give up.
fn speed_up_card(log: &mut Log) -> bool {
    let Ok((ms, reference)) = time_card_read() else {
        let _ = writeln!(log.stamp(), "emmc: reading failed before any change");
        return sdmmc::init().is_ok();
    };
    if EMMC_TIMED_SECTORS != 0 {
        let _ = writeln!(log.stamp(), "emmc: 12 MHz, one data line: {} sectors read in {} ms", EMMC_TIMED_SECTORS, ms);
    }
    let steps: [(bool, &str, fn() -> Result<(), sdmmc::Error>); 2] =
        [(EMMC_HIGH_SPEED, "48 MHz", sdmmc::high_speed), (EMMC_FOUR_LINES, "four data lines", sdmmc::four_data_lines)];
    let mut done = 0;
    for (n, &(wanted, what, change)) in steps.iter().enumerate() {
        if !wanted {
            break;
        }
        if card_step(what, reference, change, log) {
            done = n + 1;
            continue;
        }
        // Start the card again and repeat what worked before this step.
        let _ = writeln!(log.stamp(), "emmc: taking \"{}\" back", what);
        if sdmmc::init().is_err() {
            let _ = writeln!(log.stamp(), "emmc: CARD RESTART FAILED");
            return false;
        }
        for &(_, _, earlier) in &steps[..done] {
            if earlier().is_err() {
                // Even that fails now: stay at the slow settings.
                return sdmmc::init().is_ok();
            }
        }
        break;
    }
    // What the card says about itself, which also shows that it took the
    // settings above.
    let info_read = match sdmmc::card_info() {
        Ok(info) => {
            let _ = writeln!(
                log.stamp(),
                "emmc: {} sectors ({} MB), settings revision {}, speeds {:#04x}, timing {}, bus width {}, power notice {}",
                info.sectors,
                info.sectors / 2048,
                info.revision,
                info.device_type,
                info.timing,
                info.bus_width,
                info.power_notice
            );
            true
        }
        Err(e) => {
            let _ = writeln!(log.stamp(), "emmc: extended settings could not be read (error {})", e as u8);
            false
        }
    };
    if !info_read || (EMMC_TIMED_SECTORS != 0 && !time_card_read_chunked(reference, log)) {
        // Start the card again and repeat the steps that had worked.
        let _ = writeln!(log.stamp(), "emmc: restarting the card after the failed read");
        if sdmmc::init().is_err() {
            let _ = writeln!(log.stamp(), "emmc: CARD RESTART FAILED");
            return false;
        }
        for &(_, _, step) in &steps[..done] {
            if step().is_err() {
                return sdmmc::init().is_ok();
            }
        }
    }
    true
}

/// The settings file, beside the log.
const SETTINGS_DIR: &str = "LOG";
const SETTINGS_FILE: &str = "RUST.CFG";

/// Read the settings from their file. A watch without the file, or with
/// one that cannot be read, has the start values.
fn settings_load(partition: storage::Partition, log: &mut Log) -> Settings {
    let mut text = [0u8; settings::FILE_SIZE];
    match storage::read_tail(partition, SETTINGS_DIR, SETTINGS_FILE, &mut text) {
        Ok((_, n)) => {
            let settings = Settings::parse(&text[..n]);
            let _ = writeln!(log.stamp(), "settings: read from the file ({} bytes): {:?}", n, settings);
            settings
        }
        Err(step) => {
            let _ = writeln!(log.stamp(), "settings: no file to read (step {}), start values", step);
            Settings::START
        }
    }
}

/// Write the settings to their file. True if they are stored. Not
/// folded into the main loop, whose stack frame it would enlarge for good.
#[inline(never)]
fn settings_save(partition: storage::Partition, settings: &Settings, log: &mut Log) -> bool {
    let mut text = [0u8; settings::FILE_SIZE];
    let n = settings.text(&mut text);
    let written = (|| {
        // The file has to exist before it can be rewritten.
        storage::append(partition, SETTINGS_DIR, SETTINGS_FILE, &[]).map_err(|_| 1u8)?;
        let mut writer = storage::FileWriter::rewrite(partition, &[SETTINGS_DIR], SETTINGS_FILE).map_err(|_| 2u8)?;
        writer.write(&text[..n]).map_err(|()| 3u8)?;
        writer.finish().map_err(|()| 4u8)
    })();
    match written {
        Ok(()) => {
            let _ = writeln!(log.stamp(), "settings: stored: {:?}", settings);
        }
        Err(step) => {
            let _ = writeln!(log.stamp(), "settings: could NOT be stored (step {})", step);
        }
    }
    written.is_ok()
}

/// The log file is started afresh at a start that finds it larger than
/// this; what was in it is kept as `log/rust.old` (one generation).
const LOG_LIMIT: u32 = 256 * 1024;

/// Start the log file afresh if it has grown past `LOG_LIMIT`. Returns
/// the size of the file that was moved aside, for the first log line.
fn rotate_log(partition: storage::Partition) -> Option<Result<u32, u8>> {
    static mut PIECE: [u8; 16384] = [0; 16384];
    let mut probe = [0u8; 1];
    let (len, _) = storage::read_tail(partition, "LOG", "RUST.LOG", &mut probe).ok()?;
    if len <= LOG_LIMIT {
        return None;
    }
    // Safety: used only here, once, before anything else runs.
    let piece = unsafe { &mut *core::ptr::addr_of_mut!(PIECE) };
    let copied = (|| {
        storage::append(partition, "LOG", "RUST.OLD", &[]).map_err(|_| 1u8)?; // make sure it exists
        let mut reader = storage::FileReader::open(partition, "log/RUST.LOG").map_err(|_| 2u8)?;
        let mut writer = storage::FileWriter::rewrite(partition, &["LOG"], "RUST.OLD").map_err(|_| 3u8)?;
        loop {
            let n = reader.read(piece).map_err(|()| 4u8)?;
            if n == 0 {
                break;
            }
            writer.write(&piece[..n]).map_err(|()| 5u8)?;
            watchdog::feed();
        }
        reader.close();
        writer.finish().map_err(|()| 6u8)
    })();
    // Only once the copy is safely there is the log emptied.
    Some(copied.and_then(|()| {
        storage::FileWriter::rewrite(partition, &["LOG"], "RUST.LOG").map_err(|_| 7u8)?.finish().map_err(|()| 8u8)?;
        Ok(len)
    }))
}

/// Diagnosis: write and read back the whole PSRAM at every start (about
/// 4 s). On while the memory bus speed is being changed.
const PSRAM_FULL_TEST: bool = false;

/// How long a "not on the wrist" report has to stand, with the display on,
/// before the status screen shows it.
const NOT_WORN_HOLD: Duration = Duration::from_secs(3);

/// The wear state as the status screen shows it.
///
/// The Apollo's report is taken as it is everywhere else. On the screen a
/// "not on the wrist" is held back for a moment, because of how the
/// Apollo's check behaves (measured): while the arm hangs down, its
/// infrared check loses the skin and it reports "not on the wrist"; about
/// 1.5 s after the arm is raised it finds the skin again. Without the hold
/// the display, switched on by that same raise, would always come up
/// reading "not worn".
struct WearView {
    /// The Apollo's latest report and when the display side first saw it.
    reported: Option<bool>,
    since: Instant,
    shown: Option<bool>,
}

impl WearView {
    fn new() -> Self {
        Self { reported: None, since: Instant::now(), shown: None }
    }

    /// The state to show, given the Apollo's report and since when the
    /// display has been on.
    fn update(&mut self, reported: Option<bool>, display_on_since: Instant) -> Option<bool> {
        if reported != self.reported {
            self.reported = reported;
            self.since = Instant::now();
        }
        match reported {
            Some(false) => {
                // Counted from the report or from the display coming on,
                // whichever is later.
                let standing = self.since.max(display_on_since).elapsed();
                if self.shown.is_none() || standing >= NOT_WORN_HOLD {
                    self.shown = Some(false);
                }
            }
            other => self.shown = other,
        }
        self.shown
    }
}

/// How long the main loop waits for something from the Bluetooth chip
/// before its next pass: while anything is under way, and while the
/// display is dark and nothing is. The second is bounded by what the
/// loop does by the clock: the light sensor every 250 ms and the
/// Apollo's heartbeat every second.
/// How long the display task waits for news while the display is dark.
/// Long, so that it does not end a stop for nothing: news ends the wait.
const DARK_WAIT: Duration = Duration::from_secs(10);
/// Whether the Bluetooth link sleeps and the processor may stop while it
/// is dark and quiet.
const STOP_WHILE_DARK: bool = true;
/// How long it has to be quiet, with nobody connected, before the
/// Bluetooth link is closed and the processor may stop. It was 10 s
/// while the sleeping was being tried out. Worn for a day (2026-11-24)
/// the display went dark about 700 times, and those waits came to 1.4 h
/// of running at full power in the dark, about a third of the day's
/// drain. The stock app waits 6 or 24 ms (0x08096946 in B.70).
const STOP_AFTER: Duration = Duration::from_secs(2);
/// How long the main loop waits between passes while the processor may
/// stop; a button or the Apollo ends the wait at once.
const STOP_WAIT: Duration = Duration::from_secs(5);
const BUSY_WAIT: Duration = Duration::from_millis(20);
const QUIET_WAIT: Duration = Duration::from_millis(200);

/// The display goes dark after this long without a finger or a button.
/// A value of our own; it becomes a setting once there are settings.
const _: () = assert!(settings::LEVELS as usize == BRIGHTNESS.len());

/// How often the watch face is looked at for something that moved: about
/// 15 times a second.
const FACE_PERIOD: Duration = Duration::from_millis(66);

/// The processor's cycle counter, for timing short stretches of code.
fn cycles() -> u32 {
    cortex_m::peripheral::DWT::cycle_count()
}

/// Cycles since `*since`, which is moved on to now.
fn lap(since: &mut u32) -> u64 {
    let now = cycles();
    let passed = now.wrapping_sub(*since);
    *since = now;
    passed as u64
}

/// Switch the cycle counter on.
fn start_cycle_counter() {
    // Safety: only these two debug registers are touched, and nothing else
    // in this firmware uses them.
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    core.DCB.enable_trace();
    core.DWT.enable_cycle_counter();
}

/// Pictures a change of page takes when it runs the whole width by
/// itself; a shorter run takes fewer in proportion.
const SLIDE_STEPS: usize = 14;

/// A change of page under way: one page moves out, the other follows in.
/// Kept small on purpose: it is passed about by value.
#[derive(Clone, Copy)]
struct Pull {
    /// The page being left, and the one being pulled in (the same page
    /// if it only gives).
    from: Page,
    target: Page,
    motion: Motion,
    /// How far the picture should be moved, in pixels: where the finger is.
    wanted: usize,
    /// How far they were moved in the last picture; `None` until the two
    /// pages' pictures have been made.
    shown: Option<usize>,
    /// The finger has let go: finish the change, or let it fall back.
    release: Option<bool>,
    began: Instant,
    /// How long keeping the old picture and drawing the new page took.
    prepared: Duration,
    /// Pictures presented while the finger was down.
    pictures: u32,
    /// No blitter error and no missed pulse so far.
    good: bool,
    /// `dsi::starved_ever` when the change began.
    starved: u32,
}

/// What the screen keeps track of to redraw only what changes: several
/// kilobytes of rectangles and keys.
///
/// It lives in memory of its own, not inside `Screen`, because `Screen` is
/// handed about by value while the firmware starts, and every such
/// hand-over needs room for the whole of it on the stack. With these
/// tables inside it, the function that starts the firmware needed 54 KB
/// of the 64 KB stack for itself, and the first page pull ran out of it.
///
/// Every page has frames of its own (`Page::frames`) and keeps its
/// picture while another page is shown. So everything here that says
/// what is in a page's frames stays valid while the page is away, and
/// coming back to a page means drawing only what changed meanwhile.
struct Tracking {
    /// What every element of the watch face showed when it was last
    /// looked at (`face::key`).
    face_keys: [Option<u32>; face::ELEMENTS],
    /// Where every element of the watch face was when it was last drawn.
    face_rects: [embedded_graphics::primitives::Rectangle; face::ELEMENTS],
    /// The lines of the status page as they were last composed.
    lines: Option<[status::Line; status::LINES]>,
    /// For every page (`Page::index`): what has changed on its picture
    /// since each of its frames was drawn. At first that is everything.
    damage: [Damage; Page::COUNT],
    /// For every page: which of its frames holds its latest picture
    /// (0 or 1; always 0 for a page with one frame).
    front: [usize; Page::COUNT],
    /// Where the status page was tapped, the latest 64 places; drawn as
    /// dots.
    dots: [(u16, u16); 64],
    dot_count: usize,
}

impl Tracking {
    /// The one there is. It has its start values from the outset, in
    /// memory of its own: built while the firmware runs it would pass
    /// over the stack, all of it, and it is some 30 KB.
    fn take() -> &'static mut Self {
        static TAKEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        static mut THE_ONE: Tracking = Tracking::new();
        assert!(!TAKEN.swap(true, core::sync::atomic::Ordering::Relaxed), "one screen");
        // Handed out once only (checked above), so nothing else refers
        // to it.
        unsafe { &mut *core::ptr::addr_of_mut!(THE_ONE) }
    }

    const fn new() -> Self {
        Self {
            face_keys: [None; face::ELEMENTS],
            face_rects: [embedded_graphics::primitives::Rectangle::zero(); face::ELEMENTS],
            lines: None,
            damage: [const { Damage::new() }; Page::COUNT],
            front: [0; Page::COUNT],
            dots: [(0, 0); 64],
            dot_count: 0,
        }
    }
}

/// Which of the settings' times the page for setting a time is open for;
/// the second of a pair carries the first, already set.
#[derive(Clone, Copy)]
enum PickFor {
    QuietFrom,
    QuietTo(u16),
    MoveFrom,
    MoveTo(u16),
}

/// The panel and what is currently shown on it.
struct Screen {
    panel: panel::Panel,
    visible: bool,
    /// What the user has set (`settings`): the brightness, the time until
    /// the display goes dark, whether raising the wrist lights it.
    settings: Settings,
    auto: AutoBrightness,
    /// What the panel is set to now.
    level: u8,
    /// The last level written to the log.
    level_logged: u8,
    faded_at: Instant,
    /// The display was shown by a wrist raise, not by the button.
    shown_by_raise: bool,
    /// Since when the display has been on.
    visible_since: Instant,
    /// When a finger or a button was last used.
    active_at: Instant,
    wear: WearView,
    last_button: &'static str,
    /// Where the glass was last touched, if that is newer than the button.
    last_touch: Option<(u16, u16)>,
    /// What the alerts page lists.
    alerts: alerts::Alerts,
    /// The launcher, and whether the page shown was opened from it: the
    /// button then leads back to it.
    launcher: launcher::Launcher,
    from_launcher: bool,
    /// The page for setting a time, while it is open, and what for; and
    /// the page it was opened from, to go back to.
    picker: Option<(picker::Picker, PickFor)>,
    picker_from: Page,
    /// How far the settings app's list is moved.
    scroll: menu::Scroll,
    /// Which page is shown.
    page: Page,
    /// The frame the panel was sent last, whole; `None` before the first
    /// picture.
    on_screen: Option<usize>,
    /// The power page: the row tapped once, the row tapped twice (to be
    /// done by `display_task`), and whether a restart comes back here.
    power_chosen: Option<power::Choice>,
    power_do: Option<power::Choice>,
    stays: bool,
    /// A change of page under the finger or running out by itself.
    pull: Option<Pull>,
    /// The bulky part of what the screen keeps track of (see `Tracking`).
    t: &'static mut Tracking,
    /// When the watch face is next looked at.
    next_frame: Instant,
    /// The second the clock showed last, and when it was seen to begin.
    clock_second: Option<u8>,
    second_began: Instant,
    /// Diagnosis: pictures drawn, time spent drawing and pixels repainted
    /// since the last log line, and when the next line is due.
    pictures: u32,
    drawing_us: u64,
    drawing_max_us: u64,
    pixels: u64,
    /// Rectangles repainted and elements drawn into them (watch face).
    areas: u32,
    draws: u32,
    /// Processor cycles spent in each stage of drawing the watch face.
    stages: [u64; 9],
    stats_due: u64,
    /// The picture has changed since it was last sent to the panel.
    changed: bool,
    /// Uptime second the screen was last drawn for.
    drawn_for: Option<u64>,
    refresh_logged: bool,
}

impl Screen {
    fn new(panel: panel::Panel) -> Self {
        start_cycle_counter();
        Self {
            panel,
            visible: true,
            settings: shared::settings(),
            auto: AutoBrightness::new(START_BRIGHTNESS),
            level: START_BRIGHTNESS,
            level_logged: START_BRIGHTNESS,
            faded_at: Instant::now(),
            shown_by_raise: false,
            visible_since: Instant::now(),
            active_at: Instant::now(),
            wear: WearView::new(),
            last_button: "no button yet",
            last_touch: None,
            alerts: alerts::Alerts::new(),
            launcher: launcher::Launcher::new(),
            from_launcher: false,
            picker: None,
            picker_from: Page::Above,
            scroll: menu::Scroll::new(),
            page: Page::Face,
            on_screen: None,
            power_chosen: None,
            power_do: None,
            stays: false,
            pull: None,
            t: Tracking::take(),
            next_frame: Instant::now(),
            clock_second: None,
            second_began: Instant::now(),
            pictures: 0,
            drawing_us: 0,
            drawing_max_us: 0,
            pixels: 0,
            areas: 0,
            draws: 0,
            stages: [0; 9],
            stats_due: 30,
            changed: false,
            drawn_for: None,
            refresh_logged: false,
        }
    }

    /// Do what a press of a button stands for (`controls::Bindings`).
    async fn act(&mut self, action: Action, log: &mut Log) {
        self.active_at = Instant::now();
        let taken = match action {
            Action::HomeOrSleep => {
                self.last_button = "button: home / sleep";
                if self.visible && self.page != Page::Face {
                    // Lit, on another page: back to the launcher if the
                    // page was opened from it, to the watch face otherwise.
                    self.pull = None;
                    let back = if self.from_launcher && self.page != Page::Launcher { Page::Launcher } else { Page::Face };
                    self.go_to(back, log);
                    true
                } else {
                    self.visible = !self.visible;
                    self.visible_since = Instant::now();
                    self.shown_by_raise = false;
                    let _ = writeln!(log.stamp(), "display: {}", if self.visible { "shown" } else { "blanked" });
                    self.panel_visible(self.visible, log).await
                }
            }
            Action::PowerMenu => {
                self.last_button = "button: power page";
                self.open(Page::Power, log).await
            }
            Action::Open(page) => {
                self.last_button = "button: open a page";
                // On that page already: back to the watch face.
                let page = if self.visible && self.page == page { Page::Face } else { page };
                self.open(page, log).await
            }
            Action::StepBrightness => {
                self.last_button = "button: brightness";
                self.set(settings::Tap::NextBrightness, log);
                true // `fade` takes it from here
            }
            // Done by the main task, never sent here.
            Action::Nothing | Action::ToggleHeartRate | Action::Restart => true,
        };
        if !taken {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.last_touch = None;
        self.drawn_for = None; // show the change at once
    }

    /// Something for the wearer: it goes on the alerts page, and with
    /// `show` that page is brought up, lighting the display if it is dark.
    async fn alert(&mut self, alert: alerts::Alert, show: bool, log: &mut Log) {
        let _ = writeln!(log.stamp(), "alert: {:?}{}", alert.kind, if show { "" } else { ", not shown: quiet hours" });
        self.alerts.add(alert);
        self.t.damage[Page::Below.index()].add(face::whole());
        self.changed = true;
        if show {
            self.active_at = Instant::now();
            if !self.open(Page::Below, log).await {
                let _ = writeln!(log.stamp(), "display: the panel did not take the command");
            }
            self.drawn_for = None;
        }
    }

    /// Show `page`, lighting the display if it is dark. False if the
    /// panel did not take the command to light up.
    async fn open(&mut self, page: Page, log: &mut Log) -> bool {
        self.pull = None;
        self.go_to(page, log);
        if self.visible {
            return true;
        }
        self.visible = true;
        self.visible_since = Instant::now();
        self.shown_by_raise = false;
        let _ = writeln!(log.stamp(), "display: shown");
        self.panel_visible(true, log).await
    }

    /// React to a wrist gesture from the Apollo. Raising the wrist shows a
    /// blanked display, as in the stock app. Lowering it blanks the
    /// display again only if it was a raise that showed it: the stock
    /// app's own condition for that is a setting not read yet, and a
    /// display switched on by hand should not go dark on every movement.
    /// Returns true if the display changed.
    async fn gesture(&mut self, gesture: WristGesture, log: &mut Log) -> bool {
        let show = match gesture {
            WristGesture::Raised if !self.visible && self.settings.raise_to_wake => true,
            WristGesture::Lowered if self.visible && self.shown_by_raise => false,
            _ => return false,
        };
        self.visible = show;
        self.visible_since = Instant::now();
        self.shown_by_raise = show;
        let _ = writeln!(log.stamp(), "display: {} by wrist gesture", if show { "shown" } else { "blanked" });
        if !self.panel_visible(show, log).await {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.drawn_for = None;
        true
    }

    /// Switch the panel off or on (`Panel::set_visible`). A panel coming
    /// on has lost its picture: the page's frame is sent again by the
    /// next `update`.
    async fn panel_visible(&mut self, visible: bool, log: &mut Log) -> bool {
        if visible {
            self.on_screen = None;
        }
        self.panel.set_visible(visible, log).await
    }

    /// The display's switch-off timer: dark after the time set
    /// (`Settings::display_off_s`) without a finger or a button, counted from the display coming on
    /// if that was later. Not while a page is under the finger.
    async fn idle(&mut self, log: &mut Log) {
        let timeout = Duration::from_secs(self.settings.display_off_s as u64);
        if !self.visible || self.pull.is_some() || self.visible_since.max(self.active_at).elapsed() < timeout {
            return;
        }
        self.visible = false;
        self.visible_since = Instant::now();
        self.shown_by_raise = false;
        let _ = writeln!(log.stamp(), "display: blanked after {} s without a finger or a button", timeout.as_secs());
        if !self.panel_visible(false, log).await {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.drawn_for = None;
    }

    /// A hand covers the glass: blank the display, as the stock app does
    /// on the touch controller's "large object" (its routine `0x8033DB0`,
    /// which logs "cover off" and suspends the display). Stock does this
    /// once each time the display has been lit, and with one of its
    /// settings on only on one particular screen; that setting was not
    /// read further and is not kept here.
    async fn covered(&mut self, log: &mut Log) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.visible_since = Instant::now();
        self.shown_by_raise = false;
        let _ = writeln!(log.stamp(), "display: blanked by a covering hand");
        if !self.panel_visible(false, log).await {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.drawn_for = None;
    }

    /// The finger no longer counts: a move it had begun goes back.
    fn let_go(&mut self) {
        if let Some(pull) = self.pull.as_mut() {
            pull.release.get_or_insert(false);
        }
    }

    /// React to what the finger did.
    ///
    /// Dragging sideways pulls the next page in: to the left on the watch
    /// face brings the status page, to the right on the status page
    /// brings the face back. Dragging down on the watch face pulls a
    /// panel in over it from the top edge, dragging up one from the
    /// bottom edge; each panel is pushed back out the way it came. The
    /// picture follows the finger; letting go far enough or with a flick
    /// (`Motion::finishes`) finishes the change, anything less lets it
    /// fall back. Where no page lies sideways (to the right on the face,
    /// to the left on the status page) the page gives `motion::GIVE`
    /// pixels and goes back. All of that is laid down in `pages::drag`.
    /// A long press on the face or the status page opens a page of its
    /// own, which a tap leaves again. A tap on the status page leaves a
    /// dot.
    fn touch_gesture(&mut self, gesture: Gesture, log: &mut Log) {
        match gesture {
            // The page for setting a time and the settings list follow
            // the finger themselves (`touch_raw`); only taps reach them
            // as gestures.
            Gesture::DragStart { .. } | Gesture::Drag { .. } | Gesture::DragEnd { .. } if matches!(self.page, Page::Picker | Page::Settings) => {}
            Gesture::DragStart { direction, dx, dy, .. } if self.pull.is_none() => {
                let Some((target, motion)) = pages::drag(self.page, direction) else {
                    let _ = writeln!(log.stamp(), "touch: drag {:?}, nothing there", direction);
                    return;
                };
                self.pull = Some(Pull {
                    from: self.page,
                    target,
                    motion,
                    wanted: motion.along(dx, dy).clamp(0, motion.extent() as i32) as usize,
                    shown: None,
                    release: None,
                    began: Instant::now(),
                    prepared: Duration::from_ticks(0),
                    pictures: 0,
                    good: true,
                    starved: crate::hal::dsi::starved_ever(),
                });
            }
            Gesture::Drag { dx, dy } => {
                if let Some(pull) = self.pull.as_mut() {
                    pull.wanted = pull.motion.along(dx, dy).clamp(0, pull.motion.extent() as i32) as usize;
                }
            }
            Gesture::DragEnd { dx, dy, speed_x, speed_y } => {
                if let Some(pull) = self.pull.as_mut() {
                    let (travel, speed) = (pull.motion.along(dx, dy), pull.motion.along(speed_x, speed_y));
                    pull.wanted = travel.clamp(0, pull.motion.extent() as i32) as usize;
                    pull.release = Some(pull.motion.finishes(travel, speed));
                    let _ = writeln!(log.stamp(), "touch: let go after {} px at {} px/s", travel, speed);
                } else {
                    let _ = writeln!(log.stamp(), "touch: drag ended {}, {} from its start", dx, dy);
                }
            }
            Gesture::Tap { x, y } => {
                let _ = writeln!(log.stamp(), "touch: tap at {}, {}", x, y);
                if self.page == Page::Launcher {
                    if self.pull.is_some() {
                        return;
                    }
                    match self.launcher.tap(x, y) {
                        launcher::Outcome::Nothing => {}
                        launcher::Outcome::Chosen => {
                            self.t.damage[Page::Launcher.index()].add(face::whole());
                            self.changed = true;
                        }
                        launcher::Outcome::Open(app) => {
                            let page = match app {
                                launcher::App::Settings => Page::Settings,
                                launcher::App::Alerts => Page::Below,
                                launcher::App::Status => Page::Status,
                                launcher::App::Power => Page::Power,
                            };
                            self.go_to(page, log);
                            self.from_launcher = true;
                        }
                    }
                    // The tap is used up: it is not also a tap on the
                    // page it opened.
                    return;
                }
                if self.page == Page::Status {
                    self.dot(x as u16, y as u16);
                }
                if let (Some(page), None) = (pages::tap(self.page), self.pull) {
                    self.go_to(page, log);
                }
                if self.page == Page::Above && self.pull.is_none() {
                    match settings::tap_at(x, y) {
                        Some(settings::Tap::PickQuietTime) => self.pick(PickFor::QuietFrom, log),
                        Some(settings::Tap::PickMoveTime) => self.pick(PickFor::MoveFrom, log),
                        Some(tap) => self.set(tap, log),
                        None => {}
                    }
                } else if self.page == Page::Settings {
                    match menu::tap_at(x, y, self.scroll.moved()) {
                        Some(menu::Hit::Quick) => self.go_to(Page::Above, log),
                        Some(menu::Hit::Setting(settings::Tap::PickQuietTime)) => self.pick(PickFor::QuietFrom, log),
                        Some(menu::Hit::Setting(settings::Tap::PickMoveTime)) => self.pick(PickFor::MoveFrom, log),
                        Some(menu::Hit::Setting(tap)) => self.set(tap, log),
                        None => {}
                    }
                } else if self.page == Page::Picker {
                    let outcome = self.picker.as_mut().map_or(picker::Outcome::Nothing, |(picker, _)| picker.tap(x, y));
                    let what = self.picker.as_ref().map(|&(_, what)| what);
                    match (outcome, what) {
                        (picker::Outcome::Stepped, _) => self.wheels_turned(),
                        // The first of a pair leads on to the second.
                        (picker::Outcome::Save(seconds), Some(what)) => {
                            // These are times of day in minutes.
                            let minutes = (seconds / 60) as u16;
                            match what {
                                PickFor::QuietFrom => self.pick(PickFor::QuietTo(minutes), log),
                                PickFor::MoveFrom => self.pick(PickFor::MoveTo(minutes), log),
                                PickFor::QuietTo(from) => {
                                    self.set(settings::Tap::QuietTime((from, minutes)), log);
                                    self.go_to(self.picker_from, log);
                                }
                                PickFor::MoveTo(from) => {
                                    self.set(settings::Tap::MoveTime((from, minutes)), log);
                                    self.go_to(self.picker_from, log);
                                }
                            }
                        }
                        (picker::Outcome::Cancel, _) => self.go_to(self.picker_from, log),
                        _ => {}
                    }
                }
                if self.page == Page::Power {
                    // The first tap on a row chooses it, the second
                    // does it; a tap anywhere else takes the choice back.
                    let hit = power::choice_at(x, y);
                    if hit.is_some() && hit == self.power_chosen {
                        self.power_do = hit;
                    } else {
                        self.power_chosen = hit;
                    }
                    self.t.damage[Page::Power.index()].add(face::whole());
                    self.changed = true;
                }
            }
            Gesture::LongPress { x, y } => {
                let _ = writeln!(log.stamp(), "touch: long press at {}, {}", x, y);
                if let (Some(page), None) = (pages::long_press(self.page), self.pull) {
                    self.go_to(page, log);
                }
            }
            _ => {}
        }
    }

    /// Open the page for setting a time, for `what`, with the wheels on
    /// the time that is set now.
    fn pick(&mut self, what: PickFor, log: &mut Log) {
        let (title, minutes) = match what {
            PickFor::QuietFrom => ("QUIET FROM", self.settings.quiet_time.0),
            PickFor::QuietTo(_) => ("QUIET UNTIL", self.settings.quiet_time.1),
            PickFor::MoveFrom => ("MOVE ALERT FROM", self.settings.move_time.0),
            PickFor::MoveTo(_) => ("MOVE ALERT UNTIL", self.settings.move_time.1),
        };
        self.picker = Some((picker::Picker::new(title, minutes), what));
        self.t.damage[Page::Picker.index()].add(face::whole());
        if self.page != Page::Picker {
            self.picker_from = self.page;
            self.go_to(Page::Picker, log);
        }
        self.changed = true;
    }

    /// Where the finger is, as the touch controller reports it (`None`:
    /// lifted), for the page that follows the finger itself.
    fn touch_raw(&mut self, at: Option<(i32, i32)>) {
        let now_ms = Instant::now().as_millis();
        if self.page == Page::Settings && self.scroll.touch(at, now_ms) {
            self.t.damage[Page::Settings.index()].add(menu::moving_rect());
            self.changed = true;
        }
        if self.page != Page::Picker {
            return;
        }
        if self.picker.as_mut().is_some_and(|(picker, _)| picker.touch(at, now_ms)) {
            self.wheels_turned();
        }
    }

    /// A wheel of the page for setting a time has moved.
    fn wheels_turned(&mut self) {
        self.t.damage[Page::Picker.index()].add(picker::wheels_rect());
        self.changed = true;
    }

    /// Change a setting: it holds from now on, the settings page shows
    /// it, and the main task writes it to the file.
    fn set(&mut self, tap: settings::Tap, log: &mut Log) {
        let new = self.settings.after(tap);
        if new == self.settings {
            return;
        }
        let _ = writeln!(log.stamp(), "settings: {:?}", tap);
        self.settings = new;
        shared::change_settings(new);
        self.t.damage[Page::Above.index()].add(face::whole());
        self.t.damage[Page::Settings.index()].add(menu::window());
        self.changed = true;
    }

    /// Show `page` from the next picture on, without a move in between.
    fn go_to(&mut self, page: Page, log: &mut Log) {
        let _ = writeln!(log.stamp(), "display: {:?} -> {:?}", self.page, page);
        // A row chosen on the power page does not outlast the page.
        if self.power_chosen.take().is_some() {
            self.t.damage[Page::Power.index()].add(face::whole());
        }
        // Back at the watch face or the launcher, the next page is not
        // one opened from the launcher until the launcher says so.
        if matches!(page, Page::Face | Page::Launcher) {
            self.from_launcher = false;
        }
        // Two pages share a frame: what it holds may be the other one.
        if page.shared_frame() {
            self.t.damage[page.index()].add(face::whole());
        }
        // A time being set is given up by leaving its page.
        if page != Page::Picker {
            self.picker = None;
        }
        // The page's own frames hold its picture as it was last drawn;
        // the next picture brings it up to date and shows it.
        self.page = page;
        self.changed = true;
    }

    /// The frame with `page`'s latest picture.
    fn front_frame(&self, page: Page) -> usize {
        page.frames().0 + self.t.front[page.index()]
    }

    /// Which of `page`'s frames to draw its next picture into, as 0 or
    /// 1: the one that is not its latest, if it has two.
    fn back_slot(&self, page: Page) -> usize {
        match page.frames().1 {
            2 => 1 - self.t.front[page.index()],
            _ => 0,
        }
    }

    /// Leave a dot on the status page.
    fn dot(&mut self, x: u16, y: u16) {
        // Kept in a list, from which they are drawn. The oldest gives way
        // to the newest; both places have changed.
        let slot = self.t.dot_count % self.t.dots.len();
        if self.t.dot_count >= self.t.dots.len() {
            let (old_x, old_y) = self.t.dots[slot];
            self.t.damage[Page::Status.index()].add(status::dot_rect(old_x, old_y));
        }
        self.t.dots[slot] = (x, y);
        self.t.dot_count += 1;
        self.t.damage[Page::Status.index()].add(status::dot_rect(x, y));
        self.last_touch = Some((x, y));
        self.changed = true;
    }

    /// A new light reading, in lux.
    fn ambient(&mut self, lux: u32) {
        self.auto.feed(lux);
    }

    /// Keep the panel at the brightness that is set: a fixed step at
    /// once, and in automatic mode one step at a time towards what the
    /// light asks for. Call on every pass; does nothing most of the time.
    async fn fade(&mut self, log: &mut Log) {
        if !self.visible {
            return;
        }
        let wanted = match self.settings.brightness {
            Some(step) => BRIGHTNESS[step as usize % BRIGHTNESS.len()],
            None => self.auto.wanted(),
        };
        if self.level == wanted {
            return;
        }
        if self.settings.brightness.is_some() {
            let _ = writeln!(log.stamp(), "display: brightness fixed at {}", wanted);
            self.level = wanted;
            self.level_logged = wanted;
        } else {
            if self.faded_at.elapsed() < FADE_INTERVAL {
                return;
            }
            self.faded_at = Instant::now();
            let step = self.level.abs_diff(wanted).min(FADE_STEP);
            self.level = if wanted > self.level { self.level + step } else { self.level - step };
        }
        if !self.panel.set_brightness(self.level).await {
            let _ = writeln!(log.stamp(), "display: the panel did not take brightness {}", self.level);
        }
    }

    /// Bring the picture up to date and present it, if anything on it
    /// changed. Only what changed is drawn (see `services::damage`).
    async fn update(
        &mut self,
        readings: Option<&Readings>,
        lux: Option<u32>,
        bluetooth_connected: bool,
        measuring_heart: bool,
        log: &mut Log,
    ) {
        let now = Instant::now();
        let uptime_s = now.as_secs();
        let new_second = self.drawn_for != Some(uptime_s);
        if self.visible && self.pull.is_some() {
            self.pull_on(now, readings, lux, bluetooth_connected, measuring_heart, log).await;
            return;
        }
        // The settings list let go runs on by itself.
        if self.page == Page::Settings && self.scroll.tick(now.as_millis()) {
            self.t.damage[Page::Settings.index()].add(menu::moving_rect());
            self.changed = true;
        }
        // A wheel let go runs out by itself.
        if self.page == Page::Picker && self.picker.as_mut().is_some_and(|(picker, _)| picker.tick(now.as_millis())) {
            self.wheels_turned();
        }
        // The watch face moves: it is looked at again every `FACE_PERIOD`.
        let frame_due = self.page == Page::Face && now >= self.next_frame;
        // Off the watch face a change is brought to the second frame at
        // once. The watch face is drawn again shortly anyway, and that
        // picture brings the second frame up to date as well.
        let catch_up = self.page != Page::Face && self.t.damage[self.page.index()].pending(self.back_slot(self.page));
        // The page was changed without a move: its picture is not on the
        // screen yet.
        let not_shown = self.on_screen != Some(self.front_frame(self.page));
        if !self.visible || !(new_second || self.changed || frame_due || catch_up || not_shown) {
            return;
        }
        self.next_frame = now + FACE_PERIOD;
        self.panel.settle().await;
        if let Some((started, pixels)) = self.draw(now, new_second, readings, lux, bluetooth_connected, measuring_heart, log) {
            self.present(started, pixels, uptime_s, log).await;
        } else if self.on_screen != Some(self.front_frame(self.page)) {
            // Nothing to draw, but the page's picture is not the one on
            // the screen.
            let frame = self.front_frame(self.page);
            let _ = self.panel.present(frame).await;
            self.on_screen = Some(frame);
        }
    }

    /// Draw what changed on the current page into the one of its frames
    /// that does not hold its latest picture; that frame holds it
    /// afterwards. Returns when drawing began and how many pixels were
    /// repainted, or `None` if nothing had changed.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        now: Instant,
        new_second: bool,
        readings: Option<&Readings>,
        lux: Option<u32>,
        bluetooth_connected: bool,
        measuring_heart: bool,
        log: &mut Log,
    ) -> Option<(Instant, u32)> {
        let uptime_s = now.as_secs();
        let page = self.page.index();
        let back = self.back_slot(self.page);
        let frame = self.page.frames().0 + back;

        self.changed = false;
        if new_second {
            self.drawn_for = Some(uptime_s);
            self.log_brightness(log);
        }

        let started = Instant::now();
        let worn = self.wear.update(readings.and_then(|r| r.wear).map(|w| w.on_wrist), self.visible_since);
        if self.page == Page::Face {
            // What every element shows now; those whose key differs from
            // what is on the screen have changed.
            // The clock counts whole seconds; the part of a second comes
            // from when it was last seen to step.
            let time = clock::now();
            let second = time.map(|t| t.second);
            if second != self.clock_second {
                self.clock_second = second;
                self.second_began = now;
            }
            let state = face::State {
                millis: now.duration_since(self.second_began).as_millis().min(999) as u16,
                ticks: now.as_millis() as u32,
                time: time.map(|t| face::Time {
                    year: t.year,
                    month: t.month,
                    day: t.day,
                    weekday: t.weekday,
                    hour: t.hour,
                    minute: t.minute,
                    second: t.second,
                }),
                battery: readings.and_then(|r| r.battery).map(|(percent, _, power)| (percent.min(100) as u8, power)),
                // While a measurement by hand runs, what it says right
                // now; otherwise, with the Apollo's monitoring on, the
                // latest value it called valid.
                heart: readings
                    .and_then(|r| {
                        if measuring_heart {
                            r.heart_bpm
                        } else if self.settings.heart_monitor != settings::HeartMonitor::Off {
                            r.heart_last
                        } else {
                            None
                        }
                    })
                    .map(|bpm| bpm.min(999) as u16),
                measuring: measuring_heart,
                bluetooth: bluetooth_connected,
                worn,
                temperature: readings.and_then(|r| r.temperatures).map(|t| (t[0] * 10.0) as i16),
                steps: readings.and_then(|r| r.steps),
                step_goal: self.settings.step_goal,
            };
            let mut stage = cycles();
            for n in 0..face::ELEMENTS {
                let key = Some(face::key(n, &state));
                if self.t.face_keys[n] != key {
                    // Where it was and where it is now: an element may
                    // have moved.
                    let rect = face::rect(n, &state);
                    let old = self.t.face_rects[n];
                    if self.t.face_keys[n].is_some() && face::overlap(&old, &rect) {
                        // Moved a little: one rectangle round both places
                        // is cheaper to repaint than two that overlap.
                        self.t.damage[page].add(face::around(&old, &rect));
                    } else {
                        if self.t.face_keys[n].is_some() {
                            self.t.damage[page].add(old);
                        }
                        self.t.damage[page].add(rect);
                    }
                    self.t.face_keys[n] = key;
                    self.t.face_rects[n] = rect;
                }
            }
            let changes = self.t.damage[page].take(back);
            self.stages[0] += lap(&mut stage); // comparing, listing changes
            if changes.is_empty() {
                return None;
            }
            // Repaint each changed rectangle: background, then every
            // element that reaches into it, cut off at its edge.
            let mut canvas = self.panel.canvas(frame);
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                let clipped = &mut canvas;
                theme::clear(clipped, &area);
                self.areas += 1;
                self.stages[1] += lap(&mut stage); // background
                for n in 0..face::ELEMENTS {
                    if face::overlap(&self.t.face_rects[n], &area) {
                        self.stages[2] += lap(&mut stage); // finding what overlaps
                        face::draw(clipped, &area, n, &state);
                        self.draws += 1;
                        self.stages[3 + face::kind(n)] += lap(&mut stage);
                    }
                }
                self.stages[2] += lap(&mut stage);
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        if self.page == Page::Above {
            // Changes on a tap only, like the power page.
            let changes = self.t.damage[page].take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas(frame);
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                settings::draw(&mut canvas, &area, &self.settings);
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        if self.page == Page::Settings {
            let changes = self.t.damage[page].take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas(frame);
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                menu::draw(&mut canvas, &area, &self.settings, self.scroll.moved());
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        if self.page == Page::Launcher {
            // Changes on a tap only.
            let changes = self.t.damage[page].take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas(frame);
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                launcher::draw(&mut canvas, &area, &self.launcher);
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        if self.page == Page::Picker {
            let changes = self.t.damage[page].take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas(frame);
            if let Some((picker, _)) = self.picker.as_ref() {
                for area in changes.rects(face::whole()) {
                    canvas.clip_to(&area);
                    picker::draw(&mut canvas, &area, picker);
                }
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        if self.page == Page::Below {
            // Changes when an alert comes.
            let changes = self.t.damage[page].take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas(frame);
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                alerts::draw(&mut canvas, &area, &self.alerts);
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        if self.page == Page::Power {
            let changes = self.t.damage[page].take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas(frame);
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                power::draw(&mut canvas, &area, self.power_chosen, self.stays);
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        if panels::draws(self.page) {
            // These pages always look the same: one is drawn where a
            // frame does not have it yet, and never again.
            let changes = self.t.damage[page].take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas(frame);
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                panels::draw(&mut canvas, &area, self.page);
            }
            self.t.front[page] = back;
            return Some((started, changes.area(face::whole())));
        }
        // The status page. What the lines say now; those that differ from
        // what is on the screen have changed.
        let lines = status::compose(&status::Status {
            time: clock::now().map(|t| face::Time {
                year: t.year,
                month: t.month,
                day: t.day,
                weekday: t.weekday,
                hour: t.hour,
                minute: t.minute,
                second: t.second,
            }),
            uptime_s,
            battery: readings.and_then(|r| r.battery),
            temperatures: readings.and_then(|r| r.temperatures),
            barometer: readings.and_then(|r| r.barometer),
            heart_bpm: readings.and_then(|r| r.heart_bpm),
            lux,
            bluetooth_connected,
            worn,
            measuring_heart,
            brightness_step: self.settings.brightness.map(|n| (n as usize + 1, BRIGHTNESS.len())),
            brightness: self.level,
            last_button: self.last_button,
            last_touch: self.last_touch,
        });
        for row in 0..status::LINES {
            if self.t.lines.map_or(true, |old| old[row] != lines[row]) {
                self.t.damage[page].add(status::line_rect(row));
            }
        }
        self.t.lines = Some(lines);

        let changes = self.t.damage[page].take(back);
        if changes.is_empty() {
            return None;
        }
        // Repaint each changed rectangle: background, then whatever
        // reaches into it, in the order it lies on the screen. Drawing is
        // cut off at the rectangle's edge.
        let dots = &self.t.dots[..self.t.dot_count.min(self.t.dots.len())];
        let mut canvas = self.panel.canvas(frame);
        for area in changes.rects(status::whole()) {
            canvas.clip_to(&area);
            status::background(&mut canvas, &area);
            for &(x, y) in dots {
                if face::overlap(&status::dot_rect(x, y), &area) {
                    status::dot(&mut canvas, x, y);
                }
            }
            for (row, line) in lines.iter().enumerate() {
                if face::overlap(&status::line_rect(row), &area) {
                    status::draw_line(&mut canvas, &area, row, line);
                }
            }
        }
        self.t.front[page] = back;
        Some((started, changes.area(status::whole())))
    }

    /// Carry a change of page forward.
    ///
    /// On the first call the picture of the page being pulled in is
    /// brought up to date in a frame of its own, which is not shown. From
    /// then on the display controller makes every picture from the two
    /// pages' frames as they are, a piece of each (`move_to`); nothing is
    /// copied. While the finger is down the picture is wherever the
    /// finger is; once it has let go, a short run takes it to the end or
    /// back to the start.
    #[allow(clippy::too_many_arguments)]
    async fn pull_on(&mut self, now: Instant, readings: Option<&Readings>, lux: Option<u32>, bluetooth_connected: bool, measuring_heart: bool, log: &mut Log) {
        let Some(mut pull) = self.pull.take() else {
            return;
        };
        let gives = matches!(pull.motion, Motion::Give { .. });
        if pull.shown.is_none() && gives {
            // The page stays the one it is; nothing is drawn.
            pull.shown = Some(0);
        }
        if pull.shown.is_none() {
            let began = Instant::now();
            // Bring the picture of the page coming up to date, in a frame
            // of its own: only what changed since that page was last
            // shown is drawn.
            self.page = pull.target;
            self.drawn_for = Some(now.as_secs());
            self.panel.settle().await;
            let _ = self.draw(now, true, readings, lux, bluetooth_connected, measuring_heart, log);
            pull.shown = Some(0);
            pull.prepared = began.elapsed();
        }
        let shown = pull.shown.unwrap_or(0);
        let Some(finish) = pull.release else {
            // Under the finger: one picture, where the finger is now.
            if pull.wanted != shown {
                pull.good &= self.move_to(&pull, pull.wanted).await;
                pull.shown = Some(pull.wanted);
                pull.pictures += 1;
            }
            self.pull = Some(pull);
            return;
        };
        // Let go: run to the end or back to the start, in one go without
        // going back to the rest of the firmware in between (that round
        // trip takes longer than a picture).
        let end = if finish { pull.motion.extent() } else { 0 };
        let distance = end.abs_diff(shown);
        let steps = (distance * SLIDE_STEPS / panel::WIDTH).max(2);
        let run_began = Instant::now();
        for step in 1..=steps {
            // Ease out: fast first, slowing into place.
            let t = step * 1024 / steps;
            let eased = 1024 - (1024 - t) * (1024 - t) / 1024;
            let offset = if step == steps { end } else if finish { shown + distance * eased / 1024 } else { shown - distance * eased / 1024 };
            pull.good &= self.move_to(&pull, offset).await;
            watchdog::feed();
        }
        // The screen shows one of the two pages whole, from its own
        // frame; the other page keeps its picture for the next time.
        if !finish {
            self.page = pull.from;
        }
        self.on_screen = Some(self.front_frame(self.page));
        let _ = writeln!(
            log.stamp(),
            "display: {:?} {}{}: {} ms to draw the other page, {} pictures under the finger in {} ms, then {} pictures in {} ms; {} frames starved",
            pull.motion,
            if finish { "finished" } else { "fell back" },
            if pull.good { "" } else { " (with an ERROR or a missed pulse)" },
            pull.prepared.as_millis(),
            pull.pictures,
            (run_began - pull.began).as_millis().saturating_sub(pull.prepared.as_millis()),
            steps,
            run_began.elapsed().as_millis(),
            crate::hal::dsi::starved_ever().wrapping_sub(pull.starved)
        );
    }

    /// Show the picture of a change of page that has got `offset` pixels
    /// on its way: pieces of the two pages' pictures, which the display
    /// controller shows from the frames they are in (`Motion::pieces`).
    /// False if the panel's pulse was missed.
    async fn move_to(&mut self, pull: &Pull, offset: usize) -> bool {
        let pieces = pull.motion.pieces(offset).map(|piece| {
            piece.map(|p| panel::Piece {
                coming: p.coming,
                from: (p.from_x as u32, p.from_y as u32),
                to: (p.to_x as u32, p.to_y as u32),
                size: (p.width as u32, p.height as u32),
            })
        });
        let (leaving, coming) = (self.front_frame(pull.from), self.front_frame(pull.target));
        self.panel.present_move(pieces, leaving, coming).await.in_step
    }

    /// Present the picture that was just drawn (drawing began at
    /// `started` and repainted `pixels` pixels), and keep the statistics.
    async fn present(&mut self, started: Instant, pixels: u32, uptime_s: u64, log: &mut Log) {
        let drawing = started.elapsed();
        self.pictures += 1;
        self.drawing_us += drawing.as_micros();
        self.drawing_max_us = self.drawing_max_us.max(drawing.as_micros());
        self.pixels += pixels as u64;

        let started = Instant::now();
        let frame = self.front_frame(self.page);
        let r = self.panel.present(frame).await;
        self.on_screen = Some(frame);
        // The first picture and every failure go to the log.
        if !r.done || !r.in_step || !self.refresh_logged {
            self.refresh_logged = true;
            let _ = writeln!(
                log.stamp(),
                "display: picture presented after {} ms{}{}; WISR={:#010x} ISR0={:#010x} ISR1={:#010x} PSR={:#010x}",
                started.elapsed().as_millis(),
                if r.done { "" } else { ", the frame before had NOT gone out" },
                if r.in_step { ", in step with the panel" } else { ", NO tearing-effect pulse" },
                r.wisr,
                r.isr0,
                r.isr1,
                r.psr
            );
        }
        // Diagnosis: what drawing costs, twice a minute.
        if uptime_s >= self.stats_due {
            self.stats_due = uptime_s + 30;
            let _ = writeln!(
                log.stamp(),
                "display: a picture every {} ms ({} in 30 s); {} us of each is drawing, {} us at most; a picture on average: {} pixels in {} rectangles, {} elements drawn",
                30_000 / self.pictures.max(1),
                self.pictures,
                self.drawing_us / self.pictures.max(1) as u64,
                self.drawing_max_us,
                self.pixels / self.pictures.max(1) as u64,
                self.areas / self.pictures.max(1),
                self.draws / self.pictures.max(1)
            );
            let _ = writeln!(log.stamp(), "stack: {} of {} bytes used at most", crate::boot::stack::most_used(), crate::boot::stack::SIZE);
            let _ = writeln!(log.stamp(), "display: {} frames in which the display controller ran short of pixels", crate::hal::dsi::take_starved());
            let (shortest, longest) = crate::hal::dsi::take_travel_us();
            let _ = writeln!(log.stamp(), "display: a frame took {} to {} us from its start to the end of refresh", shortest, longest);
            if !crate::services::canvas::blitter_usable() {
                let _ = writeln!(log.stamp(), "display: the blitter did NOT finish an operation once; the processor draws everything now");
            }
            // Processor cycles at 120 a microsecond.
            let us = |cycles: u64| cycles / 120 / self.pictures.max(1) as u64;
            let _ = writeln!(
                log.stamp(),
                "display: drawing by stage, us a picture: comparing {}, background {}, finding overlaps {}; drawing rings {}, digits {}, text {}, dials {}, marks {}, bars {}",
                us(self.stages[0]),
                us(self.stages[1]),
                us(self.stages[2]),
                us(self.stages[3]),
                us(self.stages[4]),
                us(self.stages[5]),
                us(self.stages[6]),
                us(self.stages[7]),
                us(self.stages[8])
            );
            self.stages = [0; 9];
            (self.pictures, self.drawing_us, self.drawing_max_us, self.pixels, self.areas, self.draws) = (0, 0, 0, 0, 0, 0);
        }
    }

    /// Where automatic brightness has arrived, at most once a second.
    fn log_brightness(&mut self, log: &mut Log) {
        if self.settings.brightness.is_none() && self.level == self.auto.wanted() && self.level != self.level_logged {
            let _ = writeln!(
                log.stamp(),
                "display: automatic brightness {} -> {} at {} lux",
                self.level_logged,
                self.level,
                self.auto.lux()
            );
            self.level_logged = self.level;
        }
    }
}

/// How often the light sensor is read.
/// How long after the message to the Apollo the STM32 switches itself
/// off: time for the Apollo's acknowledgement (31 ms when measured) and
/// for the log file.
const POWER_OFF_MS: u64 = 500;

const LIGHT_INTERVAL: Duration = Duration::from_millis(250);
/// The sensor reads 0 until its first measurement is finished (measured:
/// 0 after 22 ms, a real value after 270 ms), so wait this long after
/// switching it on.
const LIGHT_FIRST_READING: Duration = Duration::from_millis(400);

/// The sensor bus and what is on it.
struct Sensors {
    bus: light::Bus,
    light: Option<light::LightSensor>,
    last_raw: Option<light::Raw>,
    /// The last reading in lux.
    last_lux: Option<u32>,
    /// From when the light sensor's values can be used.
    ready_at: Instant,
    /// When the light sensor was last read.
    read_at: Option<Instant>,
    /// Uptime second of the last reading written to the log.
    logged_for: Option<u64>,
    logged: u32,
    /// Highest light channel value logged so far.
    brightest: u16,
}

impl Sensors {
    /// Read the light sensor four times a second; returns the brightness in
    /// lux when there is a new reading. The first values and then one
    /// reading every ten seconds go to the log.
    fn poll(&mut self, log: &mut Log) -> Option<u32> {
        let light = self.light.as_mut()?;
        if Instant::now() < self.ready_at || self.read_at.is_some_and(|at| at.elapsed() < LIGHT_INTERVAL) {
            return None;
        }
        self.read_at = Some(Instant::now());
        self.last_raw = light.read(&mut self.bus);
        self.last_lux = self.last_raw.map(|raw| light.lux(raw));
        let now = Instant::now().as_secs();
        if (self.logged < 5 || now % 10 == 0) && self.logged_for != Some(now) {
            self.logged_for = Some(now);
            self.logged += 1;
            match self.last_raw {
                Some(raw) => {
                    let _ = writeln!(
                        log.stamp(),
                        "light sensor: {} lux (raw {} / {})",
                        light.lux(raw),
                        raw.light,
                        raw.second
                    );
                }
                None => {
                    let _ = writeln!(log.stamp(), "light sensor: no answer");
                }
            }
        }
        // Every new maximum (by a quarter or more) goes to the log, to see
        // how the sensor behaves in bright light.
        if let Some(raw) = self.last_raw {
            if raw.light >= 1000 && raw.light as u32 * 4 >= self.brightest as u32 * 5 {
                self.brightest = raw.light;
                let _ = writeln!(
                    log.stamp(),
                    "light sensor: new maximum, {} lux (raw {} / {})",
                    light.lux(raw),
                    raw.light,
                    raw.second
                );
            }
        }
        self.last_lux
    }
}

/// The display's own task: reads the touch controller, draws and presents.
///
/// It waits only for the panel (its tearing pulse, a frame on its way) and
/// the blitter, and in those waits the main task does its work. So a
/// picture no longer has to wait for a pass through everything else, which
/// took longer than the panel takes for a frame.
#[embassy_executor::task]
async fn display_task(mut screen: Screen, mut touch: Option<touch::Touch>) -> ! {
    let mut log = Log::get();
    let mut gestures = Gestures::new();
    // How often the loop went round, for the log.
    let (mut display_passes, mut display_passes_since) = (0u32, Instant::now());
    // The touch controller did not confirm its sleep command.
    let mut touch_sleep_failed = false;
    loop {
        let state = shared::state();
        while let Some(event) = shared::next_event() {
            match event {
                shared::Event::Act(action) => {
                    let _turn = crate::services::turn::take().await;
                    screen.act(action, &mut log).await
                }
                shared::Event::Wrist(gesture) => {
                    let _turn = crate::services::turn::take().await;
                    screen.gesture(gesture, &mut log).await;
                }
                shared::Event::Light(lux) => screen.ambient(lux),
                shared::Event::Alert(alert, show) => {
                    let _turn = crate::services::turn::take().await;
                    screen.alert(alert, show, &mut log).await;
                }
            }
        }
        shared::set_visible(screen.visible);
        if screen.stays != state.stays {
            // The power page says where a restart leads.
            screen.stays = state.stays;
            screen.t.damage[Page::Power.index()].add(face::whole());
        }
        match screen.power_do.take() {
            Some(power::Choice::PowerOff) => {
                // The Apollo switches the watch off, and the main task
                // talks to the Apollo. Dark first: the picture would
                // otherwise stand until the power goes.
                let _ = writeln!(log.stamp(), "power: power off chosen on the watch");
                screen.power_chosen = None;
                screen.t.damage[Page::Power.index()].add(face::whole());
                screen.visible = false;
                screen.panel_visible(false, &mut log).await;
                shared::request_power_off();
            }
            Some(choice) => power_do(choice, &mut log),
            None => {}
        }
        if state.transferring {
            Timer::after_millis(20).await;
            continue;
        }
        // The touch controller sleeps while the display is dark.
        if let Some(touch) = touch.as_mut() {
            if screen.visible && touch.asleep() {
                touch.wake(&mut log).await;
            } else if !screen.visible && !touch.asleep() && !touch_sleep_failed {
                // Not tried again until the display has been lit once more.
                touch_sleep_failed = !touch.sleep(&mut log).await;
            }
            if screen.visible {
                touch_sleep_failed = false;
            }
        }
        let touched = touch.as_mut().and_then(|touch| touch.poll(&mut log));
        if touch.as_mut().is_some_and(|touch| touch.take_large_object()) {
            let _turn = crate::services::turn::take().await;
            screen.covered(&mut log).await;
        }
        let gesture = gestures.feed(touched, Instant::now());
        if screen.visible {
            if touched.is_some() {
                screen.active_at = Instant::now();
            }
            match touched {
                Some(touch::TouchEvent::Down { x, y }) => screen.touch_raw(Some((x as i32, y as i32))),
                Some(touch::TouchEvent::Up) => screen.touch_raw(None),
                None => {}
            }
            if let Some(gesture) = gesture {
                screen.touch_gesture(gesture, &mut log);
            }
        } else {
            // A dark display takes no gestures (in the stock app the
            // touch controller sleeps then), and a move the finger had
            // begun goes back when the display shows again.
            screen.let_go();
        }
        {
            // Not while a block is being exchanged with the Apollo; see
            // `services::turn`.
            let _turn = crate::services::turn::take().await;
            screen.fade(&mut log).await;
            screen.idle(&mut log).await;
            screen.update(state.readings.as_ref(), state.lux, state.bluetooth_connected, state.measuring_heart, &mut log).await;
        }
        display_passes += 1;
        if display_passes_since.elapsed() >= Duration::from_secs(30) {
            let _ = writeln!(log.stamp(), "display task: {} passes in 30 s", display_passes);
            display_passes = 0;
            display_passes_since = Instant::now();
        }
        // Dark, but the touch controller is still to be put to sleep:
        // that is the next pass's business.
        let touch_settled = touch_sleep_failed || touch.as_ref().is_none_or(|touch| touch.asleep());
        if screen.visible || !touch_settled {
            // Let the main task have its turn even when nothing was presented.
            Timer::after_millis(1).await;
        } else {
            // Dark: nothing to do until the main task has news (a button,
            // a wrist gesture, a request to switch off).
            shared::wait_event(DARK_WAIT).await;
        }
    }
}

/// Do what was chosen on the power page. Comes back only if it could
/// not be done.
fn power_do(choice: power::Choice, log: &mut Log) {
    match choice {
        power::Choice::Restart => {
            let _ = writeln!(log.stamp(), "power: restart chosen on the watch");
        }
        power::Choice::Stock => {
            let _ = writeln!(log.stamp(), "power: return to the stock firmware chosen on the watch");
            if ticket::request_install(ticket::Source::PacketA, 0x8C).is_err() {
                let _ = writeln!(log.stamp(), "power: could not book the stock firmware; staying");
                let _ = log.flush();
                return;
            }
        }
        // Done by the display task's loop: it needs the main task.
        power::Choice::PowerOff => return,
    }
    let _ = log.flush();
    crate::hal::rtc::reset_on_the_second();
}

/// Everything besides the display, once the hardware is up: the Apollo,
/// Bluetooth, the buttons, the sensors and the log file. Each pass takes
/// at most about 20 ms unless a packet is being dealt with. Ends only in a
/// reset: an installed update, a return to the stock firmware, or the
/// watchdog.
async fn main_loop(
    mut link: Link,
    mut server: Server,
    mut apollo: Option<apollo::Apollo>,
    mut buttons: buttons::Buttons,
    has_screen: bool,
    mut sensors: Sensors,
) -> ! {
    let mut log = Log::get();
    let mut last_save = Instant::now();
    let mut measuring_heart = false;
    let bindings = Bindings::new();
    let (mut hold, mut taps) = (Hold::new(), Taps::new());
    // When the STM32 switches itself off, after the Apollo was asked to.
    let mut power_off_at: Option<u64> = None;
    // How often the loop went round, for the log.
    let (mut passes, mut passes_since) = (0u32, Instant::now());
    // Since when it has been quiet with nobody connected, and how many
    // waits the processor could have stopped in.
    let mut quiet_since: Option<Instant> = None;
    let mut stop_waits = 0u32;
    // How often the Bluetooth link was put to sleep, since when it
    // sleeps, and when the chip last woke it.
    let mut link_sleeps = 0u32;
    let mut asleep_since = Instant::now();
    let mut chip_called_at: Option<Instant> = None;
    crate::hal::wake::start();
    loop {
        // The Apollo's handshake cannot wait long, so look after it on every
        // pass - but not while a package is streaming in.
        if !server.transferring() {
            if let Some(apollo) = apollo.as_mut() {
                apollo.service(&mut log).await;
            }
        }

        let lux = if server.transferring() { None } else { sensors.poll(&mut log) };

        // Buttons: the upper one is ours, the lower one comes through the
        // Apollo. Up to two presses can come out of one pass.
        let now_ms = Instant::now().as_millis();
        let mut presses: [Option<(Button, Press)>; 3] = [None; 3];
        let _ = buttons.poll(&mut log);
        presses[0] = hold.feed(buttons.upper_pressed(), now_ms).map(|press| (Button::Upper, press));
        if clock::take_changed() {
            if let Some(apollo) = apollo.as_mut() {
                apollo.time_changed();
            }
        }
        let lower = apollo.as_mut().and_then(|a| a.take_key());
        // Short presses are counted from the press and release reports;
        // the Apollo's own "clicked" comes 300 ms late and not for every
        // press (see `messages::KeyEvent`).
        let lower_presses = apollo.as_mut().map_or(0, |a| a.take_short_presses());
        // Short presses of the lower button are told apart as click and
        // double click here; long ones the Apollo reports.
        for _ in 0..lower_presses {
            if let Some(press) = taps.pressed(now_ms, bindings.waits_for_double(Button::Lower)) {
                presses[1] = Some((Button::Lower, press));
            }
        }
        if let (None, Some(press)) = (presses[1], taps.tick(now_ms)) {
            presses[1] = Some((Button::Lower, press));
        }
        presses[2] = match lower {
            Some(KeyEvent::LongPressed) => Some((Button::Lower, Press::Long)),
            Some(KeyEvent::VeryLongPressed) => Some((Button::Lower, Press::VeryLong)),
            _ => None,
        };
        // Feedback through the vibration motor: one short pulse for a
        // short press of the lower button.
        if let (Some(apollo), true) = (apollo.as_mut(), lower_presses > 0) {
            apollo.vibrate(200, 0, 1, &mut log).await;
        }
        for (button, press) in presses.into_iter().flatten() {
            let action = bindings.action(button, press);
            let _ = writeln!(log.stamp(), "button: {:?} {:?} -> {:?}", button, press, action);
            match action {
                Action::Nothing => {}
                Action::ToggleHeartRate => {
                    // Three pulses for start, two for stop.
                    if let Some(apollo) = apollo.as_mut() {
                        measuring_heart = !measuring_heart;
                        apollo.measure_heart_rate(measuring_heart, &mut log).await;
                        apollo.vibrate(200, 100, if measuring_heart { 3 } else { 2 }, &mut log).await;
                    }
                }
                Action::Restart => {
                    // The way out, done here and not by the display task.
                    let _ = writeln!(log.stamp(), "button: restarting");
                    let _ = log.flush();
                    crate::hal::rtc::reset_on_the_second();
                }
                _ if has_screen => shared::send(shared::Event::Act(action)),
                _ => {}
            }
        }
        if let (Some(settings), Some(partition)) = (shared::take_changed_settings(), log.partition()) {
            settings_save(partition, &settings, &mut log);
        }
        if shared::take_power_off() {
            let sent = match apollo.as_mut() {
                Some(apollo) => {
                    let _ = log.flush();
                    apollo.power_off(&mut log).await
                }
                None => false,
            };
            if sent {
                let _ = writeln!(log.stamp(), "power: the Apollo was asked to switch the watch off");
                // The Apollo acknowledges and then goes to sleep; the
                // STM32 follows a moment later (below).
                power_off_at = Some(now_ms + POWER_OFF_MS);
            } else {
                // Without the Apollo asleep nothing would start the watch
                // again: stay on.
                let _ = writeln!(log.stamp(), "power: the Apollo did not take the message; staying on");
                shared::send(shared::Event::Act(Action::PowerMenu));
            }
            let _ = log.flush();
        }
        if power_off_at.is_some_and(|at| now_ms >= at) {
            let _ = writeln!(log.stamp(), "power: entering the STM32's Shutdown mode");
            let _ = log.flush();
            crate::hal::power::shutdown();
        }
        // What the display task needs to know, and what happened for it.
        if has_screen {
            shared::set_state(shared::State {
                readings: apollo.as_ref().map(|a| a.readings),
                lux: sensors.last_lux,
                bluetooth_connected: server.connected,
                measuring_heart,
                transferring: server.transferring(),
                stays: server.staying(),
            });
            if let Some(lux) = lux {
                shared::send(shared::Event::Light(lux));
            }
            if let Some(apollo) = apollo.as_mut() {
                if let Some(gesture) = apollo.take_gesture() {
                    shared::send(shared::Event::Wrist(gesture));
                }
                // The Apollo is told whenever this changes.
                apollo.set_screen(shared::visible());
                let settings = shared::settings();
                apollo.set_raise_to_wake(settings.raise_to_wake);
                apollo.set_quiet(settings.quiet.then_some(settings.quiet_time));
                apollo.set_move_reminder(settings.move_reminder.then_some(settings.move_time));
                apollo.set_heart_monitor(settings.heart_monitor.mode());
                if apollo.take_move_reminder() {
                    // The Apollo only says so: the vibration is ours, and
                    // so is leaving the wearer alone in the quiet hours
                    // (the alert is listed all the same).
                    let now = clock::now();
                    let quiet = now.is_some_and(|t| settings.quiet_at(t.hour as u16 * 60 + t.minute as u16));
                    if !quiet {
                        apollo.vibrate(200, 150, 2, &mut log).await;
                    }
                    let alert = alerts::Alert { kind: alerts::Kind::Move, at: now.map(|t| (t.hour, t.minute)) };
                    shared::send(shared::Event::Alert(alert, !quiet));
                }
            }
        }

        // With the display dark and nothing under way, the pass after
        // this one can wait: a button, the Apollo or Bluetooth ends the
        // wait at once (`hal::wake`), as does anything arriving from the
        // Bluetooth chip.
        let quiet = has_screen
            && !shared::visible()
            && !buttons.upper_pressed()
            && buttons.settled()
            && !taps.waiting()
            && power_off_at.is_none()
            && !server.transferring();
        // The Bluetooth link sleeps and the processor may stop, the way
        // the stock app does it (its routines at 0x080969B6 and
        // 0x08096A58; see `cypress::Link::close`): after a while of dark
        // and quiet with nobody connected, and only while the chip's own
        // line says that it has nothing for us. The chip stays
        // connectable meanwhile, keeps what it has to say, and raises
        // that line, which wakes the link at once. So does the end of
        // the quiet: a button, the Apollo, a lit display.
        if quiet && !server.connected {
            quiet_since.get_or_insert(Instant::now());
        } else {
            quiet_since = None;
        }
        if STOP_WHILE_DARK
            && link.is_open()
            && !link.chip_wants_host()
            && quiet_since.is_some_and(|since| since.elapsed() >= STOP_AFTER)
        {
            link.close();
            let _ = crate::hal::wake::take_bluetooth_changed();
            if link_sleeps < 5 {
                crate::hal::stop::allow();
                let _ = write!(log.stamp(), "stop: Bluetooth link asleep after {} s of quiet; with nothing to run the HAL would enter ", STOP_AFTER.as_secs());
                crate::hal::stop::write_outlook(&mut log);
                crate::hal::stop::forbid();
                let _ = writeln!(log, "");
            }
            link_sleeps += 1;
            let _ = log.flush();
            last_save = Instant::now();
            stop_waits = 0;
            asleep_since = Instant::now();
        }
        if !link.is_open() {
            let edge = crate::hal::wake::take_bluetooth_changed();
            let chip_calls = link.chip_wants_host() || edge;
            if quiet_since.is_none() || chip_calls {
                let opened = link.open();
                let _ = writeln!(
                    log.stamp(),
                    "stop: Bluetooth link {} after {} s asleep ({} waits in which the processor could stop): {}",
                    if opened { "awake" } else { "could NOT be woken" },
                    asleep_since.elapsed().as_secs(),
                    stop_waits,
                    if chip_calls { "the chip called" } else { "the quiet ended" }
                );
                quiet_since = None;
                chip_called_at = chip_calls.then(Instant::now);
            } else {
                // The log is written out now and then, awake.
                if log.pending() && last_save.elapsed() > Duration::from_secs(120) {
                    let _ = log.flush();
                    last_save = Instant::now();
                }
                stop_waits += 1;
                crate::hal::stop::allow();
                let _ = embassy_time::with_timeout(STOP_WAIT, crate::hal::wake::edge()).await;
                crate::hal::stop::forbid();
                continue;
            }
        }
        passes += 1;
        if passes_since.elapsed() >= Duration::from_secs(30) {
            let (button, apollo_edges, bluetooth) = crate::hal::wake::take_counts();
            let _ = writeln!(log.stamp(), "main loop: {} passes in 30 s; line changes: button {}, Apollo {}, Bluetooth {}", passes, button, apollo_edges, bluetooth);
            passes = 0;
            passes_since = Instant::now();
        }
        let activity = server.step(&mut link, if quiet { QUIET_WAIT } else { BUSY_WAIT }, &mut log).await;
        // The chip woke the link and then said nothing: that is how the
        // chip looked when it was lost once (2026-11-23, with the "ready"
        // line not yet driven by hand). Switched off and on, it works
        // again.
        if activity == Activity::Handled {
            chip_called_at = None;
        } else if chip_called_at.is_some_and(|at| at.elapsed() > Duration::from_secs(5)) {
            chip_called_at = None;
            if link.chip_wants_host() {
                let _ = writeln!(log.stamp(), "stop: the chip has been calling for 5 s and said nothing; switching it off and on");
                let back = link.power_cycle().await && bluetooth::set_up(&mut link, &mut log).await;
                let _ = writeln!(log.stamp(), "stop: the chip is {}", if back { "back" } else { "NOT back" });
            } else {
                let _ = writeln!(log.stamp(), "stop: the chip called, said nothing and stopped calling");
            }
        }

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
