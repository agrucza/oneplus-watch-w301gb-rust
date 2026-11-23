//! The application: bring the hardware up in a safe order, then run.

mod face;
mod motion;
mod pages;
mod panels;
mod shared;
mod status;

use core::fmt::Write;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};

use crate::board::Board;
use crate::boot::{faults, ticket, watchdog};
use crate::drivers::apollo::messages::{KeyEvent, Readings, WristGesture};
use crate::drivers::buttons::ButtonEvent;
use crate::drivers::cypress::Link;
use crate::services::gestures::{Gesture, Gestures};
use motion::Motion;
use pages::Page;
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
            let frames = unsafe {
                [&mut *(psram::FRAMEBUFFER as *mut panel::Frame), &mut *(psram::FRAMEBUFFER_2 as *mut panel::Frame)]
            };
            let spare = unsafe { psram::SPARE_FRAMES.map(|address| &mut *(address as *mut panel::Frame)) };
            let mut panel = panel::Panel::start(board.display, frames, spare, &mut log).await;
            if let (true, Some(panel)) = (BLITTER_TIMING, panel.as_mut()) {
                time_blitter(panel, &mut log).await;
                let _ = log.flush();
            }
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
/// A press of the upper button shorter than this is a click.
const CLICK_MS: u64 = 800;
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

/// How often the watch face is looked at for something that moved: about
/// 15 times a second.
const FACE_PERIOD: Duration = Duration::from_millis(66);

/// Diagnosis: at start, fill and copy a frame with the processor and with
/// the hardware blitter, and log the times. Measured: a whole frame filled in
/// 23.8 ms by the processor and 4.6 ms by the blitter; copied in 50.5 ms
/// and 10.1 ms; 100 rectangles of 32 x 32 filled in 10.2 ms and 3.0 ms.
const BLITTER_TIMING: bool = false;

/// Diagnosis: what filling and copying cost, done by the processor and
/// done by the hardware blitter. Runs before the first picture; both
/// frames are left with rubbish in them, which the first two pictures
/// paint over completely.
async fn time_blitter(panel: &mut panel::Panel, log: &mut Log) {
    use crate::drivers::panel::{HEIGHT, WIDTH};
    start_cycle_counter();
    let us = |cycles: u64| cycles / 120;
    let (w, h) = (WIDTH as u16, HEIGHT as u16);
    let mut stage = cycles();

    // A whole frame filled.
    panel.back().fill(0x1234);
    let processor_fill = lap(&mut stage);
    let filled = panel.blitter_fill(0, 0, w, h, 0x07E0).await;
    let blitter_fill = lap(&mut stage);
    let back = panel.back();
    let fill_right = back[0] == 0x07E0 && back[WIDTH * HEIGHT / 2 + 7] == 0x07E0 && back[WIDTH * HEIGHT - 1] == 0x07E0;

    // A whole frame copied: the other frame gets a pattern first.
    for (i, pixel) in panel.back().iter_mut().enumerate() {
        *pixel = (i as u32).wrapping_mul(40503) as u16;
    }
    watchdog::feed();
    // Make that frame the "shown" one by copying it across with the
    // processor (timed), then wipe the target and copy with the blitter.
    panel.processor_copy_back_to_shown();
    let processor_copy = lap(&mut stage);
    panel.back().fill(0);
    lap(&mut stage);
    let copied = panel.blitter_copy().await;
    let blitter_copy = lap(&mut stage);
    let same = panel.frames_equal();
    watchdog::feed();

    // A hundred small rectangles, 32 x 32, filled.
    lap(&mut stage);
    for n in 0..100usize {
        let (x, y) = (n % 10 * 40 + 20, n / 10 * 40 + 20);
        let frame = panel.back();
        for row in 0..32 {
            frame[(y + row) * WIDTH + x..][..32].fill(0xF800);
        }
    }
    let processor_small = lap(&mut stage);
    let mut small_ok = true;
    for n in 0..100u16 {
        small_ok &= panel.blitter_fill(n % 10 * 40 + 20, n / 10 * 40 + 20, 32, 32, 0x001F).await;
    }
    let blitter_small = lap(&mut stage);

    let _ = writeln!(
        log.stamp(),
        "blitter: whole frame filled: processor {} us, blitter {} us ({}{})",
        us(processor_fill),
        us(blitter_fill),
        if filled { "no error" } else { "ERROR" },
        if fill_right { ", pixels right" } else { ", pixels WRONG" }
    );
    let _ = writeln!(
        log.stamp(),
        "blitter: whole frame copied: processor {} us, blitter {} us ({}{})",
        us(processor_copy),
        us(blitter_copy),
        if copied { "no error" } else { "ERROR" },
        if same { ", copy identical" } else { ", copy DIFFERENT" }
    );
    let _ = writeln!(
        log.stamp(),
        "blitter: 100 rectangles of 32 x 32 filled: processor {} us, blitter {} us ({})",
        us(processor_small),
        us(blitter_small),
        if small_ok { "no error" } else { "ERROR" }
    );
}

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
/// Kept small on purpose; see `Screen::left_keys`.
#[derive(Clone, Copy)]
struct Pull {
    /// The page being pulled in.
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
struct Tracking {
    /// What every element of the watch face showed when it was last
    /// looked at (`face::key`).
    face_keys: [Option<u32>; face::ELEMENTS],
    /// Where every element of the watch face was when it was last drawn.
    face_rects: [embedded_graphics::primitives::Rectangle; face::ELEMENTS],
    /// The lines of the status page as they were last composed.
    lines: Option<[status::Line; status::LINES]>,
    /// What has changed on the picture since each frame was drawn.
    damage: Damage,
    /// Where the status page was tapped, the latest 64 places; drawn as
    /// dots.
    dots: [(u16, u16); 64],
    dot_count: usize,
    /// What was known about the page a pull is leaving: enough to carry on
    /// redrawing only what changes, if that page comes back.
    left_page: Page,
    left_lines: Option<[status::Line; status::LINES]>,
    left_keys: [Option<u32>; face::ELEMENTS],
    left_rects: [embedded_graphics::primitives::Rectangle; face::ELEMENTS],
}

impl Tracking {
    const fn new() -> Self {
        Self {
            face_keys: [None; face::ELEMENTS],
            face_rects: [embedded_graphics::primitives::Rectangle::zero(); face::ELEMENTS],
            lines: None,
            damage: Damage::new(),
            dots: [(0, 0); 64],
            dot_count: 0,
            left_page: Page::Face,
            left_lines: None,
            left_keys: [None; face::ELEMENTS],
            left_rects: [embedded_graphics::primitives::Rectangle::zero(); face::ELEMENTS],
        }
    }
}

/// The panel and what is currently shown on it.
struct Screen {
    panel: panel::Panel,
    visible: bool,
    /// `None`: brightness follows the light. `Some(n)`: fixed at `BRIGHTNESS[n]`.
    fixed: Option<usize>,
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
    wear: WearView,
    last_button: &'static str,
    /// Where the glass was last touched, if that is newer than the button.
    last_touch: Option<(u16, u16)>,
    /// Which page is shown.
    page: Page,
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
            fixed: None,
            auto: AutoBrightness::new(START_BRIGHTNESS),
            level: START_BRIGHTNESS,
            level_logged: START_BRIGHTNESS,
            faded_at: Instant::now(),
            shown_by_raise: false,
            visible_since: Instant::now(),
            wear: WearView::new(),
            last_button: "no button yet",
            last_touch: None,
            page: Page::Face,
            pull: None,
            t: cortex_m::singleton!(: Tracking = Tracking::new()).expect("one screen"),
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

    /// React to a click: the upper button blanks and shows the picture,
    /// the lower one steps the brightness (automatic, then the fixed steps).
    async fn click(&mut self, upper: bool, log: &mut Log) {
        let taken = if upper {
            self.visible = !self.visible;
            self.visible_since = Instant::now();
            self.shown_by_raise = false;
            self.last_button = "upper: click";
            let _ = writeln!(log.stamp(), "display: {}", if self.visible { "shown" } else { "blanked" });
            self.panel.set_visible(self.visible).await
        } else {
            self.last_button = "lower: click";
            self.fixed = match self.fixed {
                None => Some(0),
                Some(n) if n + 1 < BRIGHTNESS.len() => Some(n + 1),
                Some(_) => None,
            };
            match self.fixed {
                Some(n) => {
                    let _ = writeln!(log.stamp(), "display: brightness fixed, step {} of {}", n + 1, BRIGHTNESS.len());
                    self.level = BRIGHTNESS[n];
                    self.level_logged = self.level;
                    self.panel.set_brightness(self.level).await
                }
                None => {
                    let _ = writeln!(log.stamp(), "display: brightness automatic");
                    true // `fade` takes it from here
                }
            }
        };
        if !taken {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.last_touch = None;
        self.drawn_for = None; // show the change at once
    }

    /// React to a wrist gesture from the Apollo. Raising the wrist shows a
    /// blanked display, as in the stock app. Lowering it blanks the
    /// display again only if it was a raise that showed it: the stock
    /// app's own condition for that is a setting not read yet, and a
    /// display switched on by hand should not go dark on every movement.
    /// Returns true if the display changed.
    async fn gesture(&mut self, gesture: WristGesture, log: &mut Log) -> bool {
        let show = match gesture {
            WristGesture::Raised if !self.visible => true,
            WristGesture::Lowered if self.visible && self.shown_by_raise => false,
            _ => return false,
        };
        self.visible = show;
        self.visible_since = Instant::now();
        self.shown_by_raise = show;
        let _ = writeln!(log.stamp(), "display: {} by wrist gesture", if show { "shown" } else { "blanked" });
        if !self.panel.set_visible(show).await {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.drawn_for = None;
        true
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
        if !self.panel.set_visible(false).await {
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
            Gesture::DragStart { direction, dx, dy, .. } if self.pull.is_none() => {
                let Some((target, motion)) = pages::drag(self.page, direction) else {
                    let _ = writeln!(log.stamp(), "touch: drag {:?}, nothing there", direction);
                    return;
                };
                self.pull = Some(Pull {
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
                if self.page == Page::Status {
                    self.dot(x as u16, y as u16);
                }
                if let (Some(page), None) = (pages::tap(self.page), self.pull) {
                    self.go_to(page, log);
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

    /// Show `page` from the next picture on, without a move in between.
    fn go_to(&mut self, page: Page, log: &mut Log) {
        let _ = writeln!(log.stamp(), "display: {:?} -> {:?}", self.page, page);
        self.page = page;
        // Nothing of the new page is on either frame.
        self.t.damage = Damage::new();
        self.t.lines = None;
        self.t.face_keys = [None; face::ELEMENTS];
        self.changed = true;
    }

    /// Leave a dot on the status page.
    fn dot(&mut self, x: u16, y: u16) {
        // Kept in a list, from which they are drawn. The oldest gives way
        // to the newest; both places have changed.
        let slot = self.t.dot_count % self.t.dots.len();
        if self.t.dot_count >= self.t.dots.len() {
            let (old_x, old_y) = self.t.dots[slot];
            self.t.damage.add(status::dot_rect(old_x, old_y));
        }
        self.t.dots[slot] = (x, y);
        self.t.dot_count += 1;
        self.t.damage.add(status::dot_rect(x, y));
        self.last_touch = Some((x, y));
        self.changed = true;
    }

    /// A new light reading, in lux.
    fn ambient(&mut self, lux: u32) {
        self.auto.feed(lux);
    }

    /// In automatic mode, move the panel one step towards the brightness
    /// the light asks for. Call on every pass; does nothing most of the time.
    async fn fade(&mut self, log: &mut Log) {
        let wanted = self.auto.wanted();
        if self.fixed.is_some() || !self.visible || self.level == wanted || self.faded_at.elapsed() < FADE_INTERVAL {
            return;
        }
        self.faded_at = Instant::now();
        let step = self.level.abs_diff(wanted).min(FADE_STEP);
        self.level = if wanted > self.level { self.level + step } else { self.level - step };
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
        let back = self.panel.back_index();
        if self.visible && self.pull.is_some() {
            self.pull_on(now, readings, lux, bluetooth_connected, measuring_heart, log).await;
            return;
        }
        // The watch face moves: it is looked at again every `FACE_PERIOD`.
        let frame_due = self.page == Page::Face && now >= self.next_frame;
        // Off the watch face a change is brought to the second frame at
        // once. The watch face is drawn again shortly anyway, and that
        // picture brings the second frame up to date as well.
        let catch_up = self.page != Page::Face && self.t.damage.pending(back);
        if !self.visible || !(new_second || self.changed || frame_due || catch_up) {
            return;
        }
        self.next_frame = now + FACE_PERIOD;
        self.panel.settle().await;
        if let Some((started, pixels)) = self.draw(now, new_second, readings, lux, bluetooth_connected, measuring_heart, log) {
            self.present(started, pixels, uptime_s, log).await;
        }
    }

    /// Draw what changed on the current page into the frame to draw into.
    /// Returns when drawing began and how many pixels were repainted, or
    /// `None` if nothing had changed.
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
        let back = self.panel.back_index();

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
                heart: readings.and_then(|r| r.heart_bpm).filter(|_| measuring_heart).map(|bpm| bpm.min(999) as u16),
                measuring: measuring_heart,
                bluetooth: bluetooth_connected,
                worn,
                temperature: readings.and_then(|r| r.temperatures).map(|t| (t[0] * 10.0) as i16),
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
                        self.t.damage.add(face::around(&old, &rect));
                    } else {
                        if self.t.face_keys[n].is_some() {
                            self.t.damage.add(old);
                        }
                        self.t.damage.add(rect);
                    }
                    self.t.face_keys[n] = key;
                    self.t.face_rects[n] = rect;
                }
            }
            let changes = self.t.damage.take(back);
            self.stages[0] += lap(&mut stage); // comparing, listing changes
            if changes.is_empty() {
                return None;
            }
            // Repaint each changed rectangle: background, then every
            // element that reaches into it, cut off at its edge.
            let mut canvas = self.panel.canvas();
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                let clipped = &mut canvas;
                status::background(clipped, &area);
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
            return Some((started, changes.area(face::whole())));
        }
        if panels::draws(self.page) {
            // These pages always look the same: one is drawn where a
            // frame does not have it yet, and never again.
            let changes = self.t.damage.take(back);
            if changes.is_empty() {
                return None;
            }
            let mut canvas = self.panel.canvas();
            for area in changes.rects(face::whole()) {
                canvas.clip_to(&area);
                panels::draw(&mut canvas, &area, self.page);
            }
            return Some((started, changes.area(face::whole())));
        }
        // The status page. What the lines say now; those that differ from
        // what is on the screen have changed.
        let lines = status::compose(&status::Status {
            uptime_s,
            readings,
            lux,
            bluetooth_connected,
            worn,
            measuring_heart,
            brightness_step: self.fixed.map(|n| (n + 1, BRIGHTNESS.len())),
            brightness: self.level,
            last_button: self.last_button,
            last_touch: self.last_touch,
        });
        for row in 0..status::LINES {
            if self.t.lines.map_or(true, |old| old[row] != lines[row]) {
                self.t.damage.add(status::line_rect(row));
            }
        }
        self.t.lines = Some(lines);

        let changes = self.t.damage.take(back);
        if changes.is_empty() {
            return None;
        }
        // Repaint each changed rectangle: background, then whatever
        // reaches into it, in the order it lies on the screen. Drawing is
        // cut off at the rectangle's edge.
        let dots = &self.t.dots[..self.t.dot_count.min(self.t.dots.len())];
        let mut canvas = self.panel.canvas();
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
        Some((started, changes.area(status::whole())))
    }

    /// Carry a change of page forward.
    ///
    /// On the first call the picture on the screen is kept, the page being
    /// pulled in is drawn completely (into the frame to draw into, which is
    /// not shown) and kept as well. From then on every picture is put
    /// together from the two with two blitter copies (`move_to`): while
    /// the finger is down, at wherever the finger is; once it has let go,
    /// in a short run to the end or back to the start.
    #[allow(clippy::too_many_arguments)]
    async fn pull_on(&mut self, now: Instant, readings: Option<&Readings>, lux: Option<u32>, bluetooth_connected: bool, measuring_heart: bool, log: &mut Log) {
        let Some(mut pull) = self.pull.take() else {
            return;
        };
        let gives = matches!(pull.motion, Motion::Give { .. });
        if pull.shown.is_none() && gives {
            // Only the picture on the screen is needed, and the page
            // stays the one it is.
            pull.good &= self.panel.keep(0, false).await;
            pull.shown = Some(0);
        }
        if pull.shown.is_none() {
            let began = Instant::now();
            pull.good &= self.panel.keep(0, false).await;
            // What is known about the page being left, in case the finger
            // lets it fall back. Copied field by field inside the screen's
            // own memory: these are several kilobytes, and a value of that
            // size passed around while a picture is being presented has to
            // fit on the stack at every level of the call (the first
            // version did that, and the firmware died at its first
            // picture).
            self.t.left_page = self.page;
            self.t.left_lines = self.t.lines;
            self.t.left_keys = self.t.face_keys;
            self.t.left_rects = self.t.face_rects;
            self.page = pull.target;
            // Nothing of the new page is on either frame.
            self.t.damage = Damage::new();
            self.t.lines = None;
            self.t.face_keys = [None; face::ELEMENTS];
            self.drawn_for = Some(now.as_secs());
            let _ = self.draw(now, true, readings, lux, bluetooth_connected, measuring_heart, log);
            pull.good &= self.panel.keep(1, true).await;
            pull.shown = Some(0);
            pull.prepared = began.elapsed();
        }
        let shown = pull.shown.unwrap_or(0);
        let Some(finish) = pull.release else {
            // Under the finger: one picture, where the finger is now.
            if pull.wanted != shown {
                pull.good &= self.move_to(pull.motion, pull.wanted).await;
                pull.good &= self.panel.present().await.in_step;
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
            pull.good &= self.move_to(pull.motion, offset).await;
            pull.good &= self.panel.present().await.in_step;
            watchdog::feed();
        }
        // The frame shown is right. Bring the other one to the same
        // picture; then both are as the page was drawn.
        pull.good &= self.move_to(pull.motion, end).await;
        self.t.damage.clear();
        if !finish && !gives {
            self.page = self.t.left_page;
            self.t.lines = self.t.left_lines;
            self.t.face_keys = self.t.left_keys;
            self.t.face_rects = self.t.left_rects;
        }
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

    /// Put together the picture of a change of page that has got `offset`
    /// pixels on its way, from the kept pictures of the page being left
    /// (0) and the page coming (1), in the frame to draw into.
    async fn move_to(&mut self, motion: Motion, offset: usize) -> bool {
        match motion {
            Motion::Pages { leftwards } => self.panel.slide(offset, leftwards).await,
            // The panel is the page coming: it reaches `offset` rows in.
            Motion::PanelIn { from_top } => self.panel.cover(offset, from_top, 1).await,
            // The panel is the page being left: so much of it remains.
            Motion::PanelOut { to_top } => self.panel.cover(panel::HEIGHT.saturating_sub(offset), to_top, 0).await,
            Motion::Give { leftwards } => self.panel.shift(offset, leftwards).await,
        }
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
        let r = self.panel.present().await;
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
        if self.fixed.is_none() && self.level == self.auto.wanted() && self.level != self.level_logged {
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
    loop {
        let state = shared::state();
        while let Some(event) = shared::next_event() {
            match event {
                shared::Event::Click { upper } => {
                    let _turn = crate::services::turn::take().await;
                    screen.click(upper, &mut log).await
                }
                shared::Event::Wrist(gesture) => {
                    let _turn = crate::services::turn::take().await;
                    screen.gesture(gesture, &mut log).await;
                }
                shared::Event::Light(lux) => screen.ambient(lux),
            }
        }
        shared::set_visible(screen.visible);
        if state.transferring {
            Timer::after_millis(20).await;
            continue;
        }
        let touched = touch.as_mut().and_then(|touch| touch.poll(&mut log));
        if touch.as_mut().is_some_and(|touch| touch.take_large_object()) {
            let _turn = crate::services::turn::take().await;
            screen.covered(&mut log).await;
        }
        let gesture = gestures.feed(touched, Instant::now());
        if screen.visible {
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
            screen.update(state.readings.as_ref(), state.lux, state.bluetooth_connected, state.measuring_heart, &mut log).await;
        }
        // Let the main task have its turn even when nothing was presented.
        Timer::after_millis(1).await;
    }
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
    loop {
        // The Apollo's handshake cannot wait long, so look after it on every
        // pass - but not while a package is streaming in.
        if !server.transferring() {
            if let Some(apollo) = apollo.as_mut() {
                apollo.service(&mut log).await;
            }
        }

        let lux = if server.transferring() { None } else { sensors.poll(&mut log) };

        // Buttons: the upper one is ours, the lower one comes through the Apollo.
        let upper_click = matches!(buttons.poll(&mut log), Some(ButtonEvent::Released { held_ms }) if held_ms < CLICK_MS);
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
        // Feedback through the vibration motor: one short pulse for a
        // click, three for a long press.
        if let Some(apollo) = apollo.as_mut() {
            if lower_presses > 0 {
                apollo.vibrate(200, 0, 1, &mut log).await;
            }
            match lower {
                Some(KeyEvent::LongPressed) => {
                    // A long press starts and stops a heart-rate
                    // measurement: three pulses for start, two for stop.
                    measuring_heart = !measuring_heart;
                    apollo.measure_heart_rate(measuring_heart, &mut log).await;
                    apollo.vibrate(200, 100, if measuring_heart { 3 } else { 2 }, &mut log).await;
                }
                _ => {}
            }
        }
        // What the display task needs to know, and what happened for it.
        if has_screen {
            shared::set_state(shared::State {
                readings: apollo.as_ref().map(|a| a.readings),
                lux: sensors.last_lux,
                bluetooth_connected: server.connected,
                measuring_heart,
                transferring: server.transferring(),
            });
            if let Some(lux) = lux {
                shared::send(shared::Event::Light(lux));
            }
            if upper_click {
                shared::send(shared::Event::Click { upper: true });
            }
            for _ in 0..lower_presses {
                shared::send(shared::Event::Click { upper: false });
            }
            if let Some(apollo) = apollo.as_mut() {
                if let Some(gesture) = apollo.take_gesture() {
                    shared::send(shared::Event::Wrist(gesture));
                }
                // The Apollo is told whenever this changes.
                apollo.set_screen(shared::visible());
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
