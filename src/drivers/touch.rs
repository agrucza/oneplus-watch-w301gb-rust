//! Touch controller: a Cypress/Parade TrueTouch chip on I2C3, address 0x24.
//!
//! From the stock app (driver at 0x08077C74 to 0x08079270):
//!
//!   - reset is PF13, active low (`0x08077D88`: high 1 ms, low 10 ms, high);
//!   - PF4 is switched with it (low before the reset, high after, at
//!     `0x0805D5CC` and `0x08077DC0`); what it feeds is not known, it is
//!     high whenever the chip is in use;
//!   - the interrupt line is PA0, low while the chip has a report waiting
//!     (the stock app takes a falling-edge interrupt and also reads the
//!     level, `0x08078002`);
//!   - I2C3 is PC0 (clock) and PB4 (data), set up without pull-ups, so the
//!     board has its own.
//!
//! The chip speaks the family's packet protocol. Everything it has to say
//! is a report: read two bytes for the length (low byte first, counting
//! itself), then read that many bytes again from the start (`0x08077CE4`).
//! A length of 0 or 2 means "nothing". Byte 2 is the report's kind:
//!
//!   - 0x01: touches (`0x08078DCC`); see `touch_report` for what is in it.
//!   - 0x04: wake-up gesture while the chip sleeps.
//!   - 0x1F: answer to a command.
//!   - 0xF7 / 0xFF: the chip's description, asked for by writing register 1.
//!     0xF7 means the touch firmware is running, 0xFF that the chip sits in
//!     its bootloader (`0x08078288`).
//!
//! Start-up follows the stock app (`0x08078700`): reset, wait 120 ms, read
//! away whatever is waiting, ask for the description. If the chip is in its
//! bootloader it is told to start the firmware.

use core::fmt::Write;

use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::i2c::{self, I2c, Master};
use embassy_stm32::mode::Blocking;
use embassy_stm32::time::Hertz;
use embassy_stm32::{peripherals, Peri};
use embassy_time::{Duration, Instant, Timer};

use super::touch_report;
use crate::services::log::Log;

const ADDRESS: u8 = 0x24;
/// The longest report the stock app accepts.
const MAX_REPORT: usize = 0x108;

const REPORT_WAKE: u8 = 0x04;
const MODE_FIRMWARE: u8 = 0xF7;
const MODE_BOOTLOADER: u8 = 0xFF;

// The three messages below are byte for byte what the stock app sends.

/// Ask for the chip's description (register 1; the two bytes the stock app
/// keeps at 0x081F5506).
const ASK_DESCRIPTION: [u8; 2] = [0x01, 0x00];
/// Command 2, "system information": firmware version and panel size. The
/// stock app builds commands at 0x08077E6C as: register 4 (04 00), length
/// (5 + parameters, low byte first), 2F 00, the command, the parameters.
const ASK_SYSTEM_INFORMATION: [u8; 7] = [0x04, 0x00, 0x05, 0x00, 0x2F, 0x00, 0x02];
/// Bootloader command 0x3B, "start the firmware" (stock: 0x0807813C).
/// Bootloader commands are built at 0x08077F12 as: register 4 (04 00),
/// length (0x0B + data), 40 00, start mark 01, the command, the data
/// length, the data, a checksum and the end mark 17. The checksum is
/// CRC-16 with polynomial 0x1021, starting at 0xFFFF, over the bytes from
/// the start mark to the end of the data (stock: 0x08077E00, table at
/// 0x081F00C8); for this command it comes to 0xC720.
const START_FIRMWARE: [u8; 13] = [0x04, 0x00, 0x0B, 0x00, 0x40, 0x00, 0x01, 0x3B, 0x00, 0x00, 0x20, 0xC7, 0x17];

/// The power command (stock: 0x0807865A; the bytes at 0x081F15E4 with the
/// state put into the third): 1 puts the chip into its deep sleep, 0
/// wakes it.
const POWER_SLEEP: [u8; 4] = [0x05, 0x00, 0x01, 0x08];
const POWER_ON: [u8; 4] = [0x05, 0x00, 0x00, 0x08];

/// How many reports are written to the log in full.
const LOGGED_REPORTS: u32 = 40;

pub struct Resources {
    pub i2c: Peri<'static, peripherals::I2C3>,
    pub scl: Peri<'static, peripherals::PC0>,
    pub sda: Peri<'static, peripherals::PB4>,
    pub reset: Peri<'static, peripherals::PF13>,
    pub enable: Peri<'static, peripherals::PF4>,
    pub irq: Peri<'static, peripherals::PA0>,
}

/// What the finger did, in the chip's coordinates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TouchEvent {
    /// A finger is on the glass at this place (sent again as it moves).
    Down { x: u16, y: u16 },
    /// The finger was lifted.
    Up,
}

pub struct Touch {
    bus: I2c<'static, Blocking, Master>,
    irq: Input<'static>,
    _pins: [Output<'static>; 2],
    buffer: [u8; MAX_REPORT],
    touching: bool,
    /// The finger lifted in the same call that reported its last place:
    /// the lift is handed out by the next call.
    lift_pending: bool,
    /// A report said that something large lies on the glass, and nobody
    /// has asked since (`take_large_object`).
    large_object: bool,
    /// The latest touch report said so.
    large_object_lies: bool,
    logged: u32,
    failures: u32,
    asleep: bool,
    /// How many sleeps and wakes were written to the log in full.
    power_logged: u32,
}

impl Touch {
    /// Reset the chip, find out what state it is in and log what it says
    /// about itself. `None` if it does not answer at all.
    pub async fn start(r: Resources, log: &mut Log) -> Option<Self> {
        let mut config = i2c::Config::default();
        config.frequency = Hertz(400_000);
        config.timeout = Duration::from_millis(20);
        let bus = I2c::new_blocking(r.i2c, r.scl, r.sda, config);
        let irq = Input::new(r.irq, Pull::Up);

        // As the stock app: PF4 low, reset pulse, PF4 high.
        let mut enable = Output::new(r.enable, Level::Low, Speed::Low);
        let mut reset = Output::new(r.reset, Level::High, Speed::Low);
        Timer::after_millis(1).await;
        reset.set_low();
        Timer::after_millis(10).await;
        reset.set_high();
        enable.set_high();
        Timer::after_millis(120).await;

        let mut touch = Self { bus, irq, _pins: [enable, reset], buffer: [0; MAX_REPORT], touching: false, lift_pending: false, large_object: false, large_object_lies: false, logged: 0, failures: 0, asleep: false, power_logged: 0 };
        let _ = writeln!(log.stamp(), "touch: reset done, interrupt line is {}", if touch.irq.is_low() { "low" } else { "high" });

        // Whatever the chip has to say after its reset.
        for _ in 0..3 {
            if touch.irq.is_high() {
                break;
            }
            touch.log_report("after reset", log);
            Timer::after_millis(20).await;
        }

        let mut mode = touch.ask_mode(log).await;
        if mode == Some(MODE_BOOTLOADER) {
            let _ = writeln!(log.stamp(), "touch: chip is in its bootloader; starting the firmware");
            if touch.bus.blocking_write(ADDRESS, &START_FIRMWARE).is_err() {
                let _ = writeln!(log.stamp(), "touch: start command not accepted");
            }
            touch.wait_for_irq(500).await;
            touch.log_report("answer to start", log);
            Timer::after_millis(120).await;
            mode = touch.ask_mode(log).await;
        }
        match mode {
            Some(MODE_FIRMWARE) => {
                let _ = writeln!(log.stamp(), "touch: firmware is running");
            }
            Some(other) => {
                let _ = writeln!(log.stamp(), "touch: unexpected mode {:#04x}; trying to read touches anyway", other);
            }
            None => {
                let _ = writeln!(log.stamp(), "touch: no answer from the chip");
                return None;
            }
        }

        // Firmware version and panel size, for the log.
        if touch.bus.blocking_write(ADDRESS, &ASK_SYSTEM_INFORMATION).is_ok() {
            touch.wait_for_irq(1000).await; // the stock app allows a second
            touch.log_report("system information", log);
        } else {
            let _ = writeln!(log.stamp(), "touch: system information not accepted");
        }
        Some(touch)
    }

    /// Ask for the chip's description and return its mode byte.
    async fn ask_mode(&mut self, log: &mut Log) -> Option<u8> {
        if let Err(e) = self.bus.blocking_write(ADDRESS, &ASK_DESCRIPTION) {
            let _ = writeln!(log.stamp(), "touch: nothing at {:#04x} on I2C3 ({:?})", ADDRESS, e);
            return None;
        }
        if !self.wait_for_irq(800).await {
            let _ = writeln!(log.stamp(), "touch: no interrupt after asking for the description");
        }
        let len = self.log_report("description", log)?;
        (len >= 3).then(|| self.buffer[2])
    }

    /// Wait until the interrupt line is low; false after `ms` without it.
    async fn wait_for_irq(&mut self, ms: u64) -> bool {
        let started = Instant::now();
        while self.irq.is_high() {
            if started.elapsed() > Duration::from_millis(ms) {
                return false;
            }
            Timer::after_millis(1).await;
        }
        true
    }

    /// Read one report into `buffer`. `Some(0)` if the chip has nothing,
    /// `None` if the bus failed or the length makes no sense.
    fn read_report(&mut self) -> Option<usize> {
        let mut head = [0u8; 2];
        self.bus.blocking_read(ADDRESS, &mut head).ok()?;
        let len = u16::from_le_bytes(head) as usize;
        if len == 0 || len == 2 {
            return Some(0);
        }
        if len > MAX_REPORT {
            return None;
        }
        self.bus.blocking_read(ADDRESS, &mut self.buffer[..len]).ok()?;
        Some(len)
    }

    /// Read a report and write it to the log. Returns its length.
    fn log_report(&mut self, what: &str, log: &mut Log) -> Option<usize> {
        match self.read_report() {
            Some(0) => {
                let _ = writeln!(log.stamp(), "touch: {}: empty report", what);
                Some(0)
            }
            Some(len) => {
                let _ = writeln!(log.stamp(), "touch: {}: {} bytes", what, len);
                log.hex(&self.buffer[..len]);
                Some(len)
            }
            None => {
                let _ = writeln!(log.stamp(), "touch: {}: could not be read", what);
                None
            }
        }
    }

    /// Fetch what the chip has waiting. Returns the latest finger event, if
    /// there was one. Call often; costs nothing while the line is high.
    ///
    /// A lift never swallows the place reported just before it: if both
    /// come in one call, the place is returned now and the lift by the
    /// next call. (Otherwise the last stretch of a quick swipe is lost,
    /// and with it the speed the finger had.)
    pub fn poll(&mut self, log: &mut Log) -> Option<TouchEvent> {
        if core::mem::take(&mut self.lift_pending) {
            return Some(TouchEvent::Up);
        }
        let mut event = None;
        // A few reports at most per call, so that a line stuck low cannot
        // hold up the caller.
        for _ in 0..4 {
            if self.irq.is_high() {
                break;
            }
            let Some(len) = self.read_report() else {
                self.failures += 1;
                if self.failures <= 5 {
                    let _ = writeln!(log.stamp(), "touch: report could not be read");
                }
                break;
            };
            if len < 3 {
                continue;
            }
            let kind = self.buffer[2];
            if self.logged < LOGGED_REPORTS {
                self.logged += 1;
                let _ = writeln!(log.stamp(), "touch: report kind {:#04x}, {} bytes", kind, len);
                log.hex(&self.buffer[..len]);
            }
            match touch_report::parse(&self.buffer[..len]) {
                Some(report) => {
                    if report.large_object {
                        // Logged once for as long as it lies there.
                        if !self.large_object_lies {
                            let _ = writeln!(log.stamp(), "touch: something large on the glass");
                        }
                        self.large_object = true;
                    } else if self.large_object_lies {
                        let _ = writeln!(log.stamp(), "touch: the large thing is gone");
                    }
                    self.large_object_lies = report.large_object;
                    match report.finger {
                        None => {
                            if self.touching {
                                self.touching = false;
                                let _ = writeln!(log.stamp(), "touch: finger lifted");
                                if event.is_some() {
                                    self.lift_pending = true;
                                    break;
                                }
                                event = Some(TouchEvent::Up);
                            }
                        }
                        Some((x, y)) => {
                            if !self.touching {
                                self.touching = true;
                                let _ = writeln!(log.stamp(), "touch: finger down at {}, {}", x, y);
                            }
                            event = Some(TouchEvent::Down { x, y });
                        }
                    }
                }
                None if kind == REPORT_WAKE => {
                    let _ = writeln!(log.stamp(), "touch: wake-up report");
                }
                None => {}
            }
        }
        event
    }

    /// Whether the chip was put to sleep (`sleep`) and not woken since.
    pub fn asleep(&self) -> bool {
        self.asleep
    }

    /// Put the chip into its deep sleep, as the stock app does whenever
    /// the display goes dark (0x08042648, 0x08079392): the power command
    /// with state 1, up to three times, each time waiting up to 500 ms
    /// for the chip's answer (F0, the state, 08 from the third byte on).
    /// The stock app also gives up the interrupt pin while the chip
    /// sleeps; here it stays an input with its pull-up. False if the chip
    /// did not confirm; it is then taken as awake.
    pub async fn sleep(&mut self, log: &mut Log) -> bool {
        if self.asleep {
            return true;
        }
        // A finger that is on the glass now is gone for us.
        self.touching = false;
        self.lift_pending = false;
        self.large_object_lies = false;
        let started = Instant::now();
        for attempt in 1..=3 {
            // What is waiting would be taken for the answer.
            for _ in 0..4 {
                if self.irq.is_high() {
                    break;
                }
                let _ = self.read_report();
            }
            if self.bus.blocking_write(ADDRESS, &POWER_SLEEP).is_err() {
                let _ = writeln!(log.stamp(), "touch: sleep command not accepted (try {})", attempt);
                continue;
            }
            // Touch reports may still come before the answer.
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                if !self.wait_for_irq(deadline.saturating_duration_since(Instant::now()).as_millis()).await {
                    break;
                }
                let Some(len) = self.read_report() else { break };
                if len >= 5 && self.buffer[2] == 0xF0 && self.buffer[3] & 3 == 1 && self.buffer[4] & 0x0F == 8 {
                    self.asleep = true;
                    if self.power_logged < 6 {
                        self.power_logged += 1;
                        let _ = writeln!(log.stamp(), "touch: asleep after {} ms (try {}), answer:", started.elapsed().as_millis(), attempt);
                        log.hex(&self.buffer[..len]);
                    }
                    return true;
                }
            }
            let _ = writeln!(log.stamp(), "touch: no answer to the sleep command (try {})", attempt);
        }
        false
    }

    /// Wake the chip from its deep sleep (stock: 0x08042670, 0x080793CC):
    /// the power command with state 0, sent once there and not waited
    /// for. Here it is sent again if the bus refuses it, and if it is
    /// refused three times the chip is reset, which wakes it in any case.
    pub async fn wake(&mut self, log: &mut Log) {
        if !self.asleep {
            return;
        }
        self.asleep = false;
        let started = Instant::now();
        for attempt in 1..=3 {
            if self.bus.blocking_write(ADDRESS, &POWER_ON).is_ok() {
                if self.power_logged < 12 {
                    self.power_logged += 1;
                    // Not waited for in normal use; here to see what the
                    // chip says and how long it takes.
                    let answered = self.wait_for_irq(100).await;
                    let _ = writeln!(log.stamp(), "touch: woken (try {}), {} after {} ms", attempt, if answered { "report" } else { "no report" }, started.elapsed().as_millis());
                    if answered {
                        self.log_report("after waking", log);
                    }
                }
                return;
            }
            Timer::after_millis(2).await;
        }
        let _ = writeln!(log.stamp(), "touch: wake command refused three times; resetting the chip");
        self._pins[1].set_low();
        Timer::after_millis(10).await;
        self._pins[1].set_high();
        Timer::after_millis(120).await;
    }

    /// True if a report since the last call said that something large
    /// lies on the glass (a covering hand).
    pub fn take_large_object(&mut self) -> bool {
        core::mem::take(&mut self.large_object)
    }
}
