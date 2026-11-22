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
//!   - 0x01: touches (`0x08078DCC`). Byte 5 holds the number of touches in
//!     bits 4:0; ten bytes per touch follow from byte 7: byte 1 is the
//!     touch number (bits 4:0) and the event (bits 6:5, 3 = finger lifted),
//!     bytes 2-3 are X and 4-5 are Y, low byte first.
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

use crate::services::log::Log;

const ADDRESS: u8 = 0x24;
/// The longest report the stock app accepts.
const MAX_REPORT: usize = 0x108;

const REPORT_TOUCH: u8 = 0x01;
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
    logged: u32,
    failures: u32,
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

        let mut touch = Self { bus, irq, _pins: [enable, reset], buffer: [0; MAX_REPORT], touching: false, lift_pending: false, logged: 0, failures: 0 };
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
            match kind {
                REPORT_TOUCH if len >= 7 => {
                    let count = (self.buffer[5] & 0x1F) as usize;
                    let lifted = count == 0 || len < 17 || (self.buffer[8] >> 5) & 3 == 3;
                    if lifted {
                        if self.touching {
                            self.touching = false;
                            let _ = writeln!(log.stamp(), "touch: finger lifted");
                            if event.is_some() {
                                self.lift_pending = true;
                                break;
                            }
                            event = Some(TouchEvent::Up);
                        }
                    } else {
                        let x = u16::from_le_bytes([self.buffer[9], self.buffer[10]]);
                        let y = u16::from_le_bytes([self.buffer[11], self.buffer[12]]);
                        if !self.touching {
                            self.touching = true;
                            let _ = writeln!(log.stamp(), "touch: finger down at {}, {}", x, y);
                        }
                        event = Some(TouchEvent::Down { x, y });
                    }
                }
                REPORT_WAKE => {
                    let _ = writeln!(log.stamp(), "touch: wake-up report");
                }
                _ => {}
            }
        }
        event
    }
}
