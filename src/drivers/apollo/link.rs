//! The Apollo co-processor (the watch's sensor hub).
//!
//! The Apollo pulls the STM32's reset line when it is unhappy with us, which
//! it is after a few minutes of silence. Everything here mirrors the stock
//! app (factory build, addresses from its disassembly):
//!
//! * SPI1 with the STM32 as *slave* (0x0808FA84): 8-bit, mode 0, no hardware
//!   chip select; SCK PA5, MISO PA6, MOSI PA7 (alternate function 5); DMA1
//!   channel 3 receives (request 10), channel 6 transmits (request 11).
//! * Every transfer is a fixed block of 256 bytes, announced on handshake
//!   pins (task at 0x080B58FC, pins set up at 0x0805D806):
//!     PG1   input   Apollo: "I have a block for you"
//!     PA15  output  STM32:  "ready, clock it in"   (while receiving)
//!     PF10  output  STM32:  "I have a block for you" (while sending)
//!     PG12  output  Apollo reset, low = held in reset
//!     PF7   output  open-drain, left high (purpose unknown)
//! * A block carries frames (builder at 0x0806C682):
//!     0x7E, 0x00, body length, flags, two u32, body, crc8 over bytes 1..
//!   The body is a protobuf message (nanopb; descriptor at 0x081AC324):
//!   field 1 is a sequence number, followed by exactly one sub-message.
//!   Sub-message 20 is the heartbeat; the stock app sends it empty.
//!
//! Observed on the watch: after its reset the Apollo sends a heartbeat
//! (sub-message 20 with field 2 = 0xA5A5A5A5) about once a second until it
//! receives a heartbeat from us; then it starts streaming sensor messages.
//! As long as its blocks are taken and its heartbeat is answered it leaves
//! our reset line alone.
//!
//! Frames with flag bit 1 set want an acknowledgement: sub-message 3 with
//! the acknowledged frame's sequence number and kind. The Apollo repeats
//! such a frame every two seconds, about ten times, until it gets one (seen
//! on the watch; it acknowledges our heartbeats the same way).
//!
//! This driver answers the heartbeat, acknowledges, and takes every block. The messages
//! themselves are only counted for now: the first one of each kind is
//! logged in full, the rest show up in a summary line once a minute.

use core::fmt::Write;
use core::ptr::{read_volatile, write_volatile};

use embassy_stm32::gpio::{Input, Level, Output, OutputOpenDrain, Pull, Speed};
use embassy_stm32::{peripherals, Peri};
use embassy_time::{Duration, Instant, Timer};

use crate::hal::reg::{RCC_AHB1ENR, RCC_APB2ENR, modify};
use crate::hal::rtc;
use crate::drivers::apollo::messages::{self as messages, Fields, Readings, Value};
use crate::services::log::Log;
use crate::boot::ticket::crc8;

const BLOCK: usize = 256;
/// Sub-message numbers go up to 142 in the stock descriptor.
const KINDS: usize = 160;
const HEARTBEAT: u32 = messages::HEARTBEAT;
const ACK: u32 = messages::ACK;
/// Frame flag: the sender wants an acknowledgement.
const FLAG_WANTS_ACK: u8 = 0x02;
/// How often the setup status is tried, and the pause between tries.
const SETUP_TRIES: u32 = 10;
const SETUP_RETRY: Duration = Duration::from_secs(3);
// Parts of the stock start-up exchange that are written but switched off:
// each is sent by the stock app, but what the Apollo does with it has not
// been read through yet. One is switched on at a time, once it has been.

/// Time (kind 5) and version text (kind 6) after the Apollo's announcement.
const SEND_TIME_AND_VERSION: bool = false;
/// Do-not-disturb and sedentary settings after the setup status.
const SEND_SETTINGS: bool = false;
/// The answer to the Apollo's question for the board revision.
const ANSWER_REVISION: bool = true;
/// How this firmware names itself to the Apollo.
const VERSION_TEXT: &str = "W301_RUST";
/// The stock app's own heartbeat period.
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(30 * 60);

const GPIOA: usize = 0x4800_0000;

const SPI1: usize = 0x4001_3000;
const SPI_CR1: *mut u32 = SPI1 as *mut u32;
const SPI_CR2: *mut u32 = (SPI1 + 0x04) as *mut u32;
const SPI_SR: *const u32 = (SPI1 + 0x08) as *const u32;
const SPI_DR: *mut u8 = (SPI1 + 0x0C) as *mut u8;
/// Software slave management with the slave permanently selected.
const CR1_SLAVE: u32 = 1 << 9;
const CR1_SPE: u32 = 1 << 6;
/// 8-bit frames, receive threshold of one byte.
const CR2_8BIT: u32 = 0b0111 << 8 | 1 << 12;
const CR2_RXDMAEN: u32 = 1 << 0;
const CR2_TXDMAEN: u32 = 1 << 1;

const DMA1: usize = 0x4002_0000;
const DMA_IFCR: *mut u32 = (DMA1 + 0x04) as *mut u32;
const DMAMUX1: usize = 0x4002_0800;
const RX_CHANNEL: usize = 3;
const TX_CHANNEL: usize = 6;
const REQUEST_SPI1_RX: u32 = 10;
const REQUEST_SPI1_TX: u32 = 11;

fn dma(channel: usize, register: usize) -> *mut u32 {
    (DMA1 + 0x08 + 0x14 * (channel - 1) + register) as *mut u32
}
const CCR: usize = 0x0;
const CNDTR: usize = 0x4;
const CPAR: usize = 0x8;
const CMAR: usize = 0xC;

/// Pins of the Apollo link.
pub struct Resources {
    pub request: Peri<'static, peripherals::PG1>,
    pub ready: Peri<'static, peripherals::PA15>,
    pub have_data: Peri<'static, peripherals::PF10>,
    pub reset: Peri<'static, peripherals::PG12>,
    pub pf7: Peri<'static, peripherals::PF7>,
    // PA5, PA6 and PA7 are set up for SPI1 directly on the registers.
    pub _sck: Peri<'static, peripherals::PA5>,
    pub _miso: Peri<'static, peripherals::PA6>,
    pub _mosi: Peri<'static, peripherals::PA7>,
}

pub struct Apollo {
    request: Input<'static>,
    ready: Output<'static>,
    have_data: Output<'static>,
    _reset: Output<'static>,
    _pf7: OutputOpenDrain<'static>,
    rx: &'static mut [u8; BLOCK],
    tx: &'static mut [u8; BLOCK],
    sequence: u32,
    next_heartbeat: Instant,
    /// The Apollo asked for a heartbeat and has not had one yet.
    heartbeat_wanted: bool,
    /// Heart reports written to the log since the last start.
    heart_logged: u32,
    /// The board revision to answer the Apollo's question with, and
    /// whether the question is waiting for its answer.
    revision: u8,
    revision_asked: bool,
    revision_answers: u32,
    pin_logged: u32,
    /// Setup status: when to send it next, the sequence number it went
    /// out with, how often it was tried, and whether the Apollo has
    /// acknowledged it.
    setup_due: Option<Instant>,
    setup_sequence: u32,
    setup_tries: u32,
    setup_acknowledged: bool,
    settings_sent: bool,
    /// Frames to acknowledge: (sequence, kind).
    acks: [(u32, u32); 8],
    acks_pending: usize,
    blocks_received: u32,
    /// Messages received per kind since the last summary, and kinds already
    /// logged in full.
    counts: [u16; KINDS],
    seen: [bool; KINDS],
    next_summary: Instant,
    /// Latest sensor values taken from the messages.
    pub readings: Readings,
    /// The lower button's latest event (it is wired to the Apollo).
    pub last_key: Option<messages::KeyEvent>,
    /// Lower-button events not yet fetched with `take_key`.
    key_pending: Option<messages::KeyEvent>,
}

impl Apollo {
    /// Set up the link and let the Apollo run (reset pulse, then released).
    pub async fn start(r: Resources) -> Option<Self> {
        let rx = cortex_m::singleton!(: [u8; BLOCK] = [0; BLOCK])?;
        let tx = cortex_m::singleton!(: [u8; BLOCK] = [0; BLOCK])?;
        unsafe {
            modify(RCC_AHB1ENR, 0, 1 << 0 | 1 << 2); // DMA1, DMAMUX1
            modify(RCC_APB2ENR, 0, 1 << 12); // SPI1
            let _ = read_volatile(RCC_APB2ENR);
            for n in 5..=7u32 {
                modify(GPIOA as *mut u32, 0b11 << (2 * n), 0b10 << (2 * n)); // alternate function
                modify((GPIOA + 0x08) as *mut u32, 0, 0b11 << (2 * n)); // very high speed
                modify((GPIOA + 0x0C) as *mut u32, 0b11 << (2 * n), 0); // no pull
                modify((GPIOA + 0x20) as *mut u32, 0xF << (4 * n), 5 << (4 * n)); // AF5 = SPI1
            }
            write_volatile(SPI_CR1, CR1_SLAVE);
            write_volatile(SPI_CR2, CR2_8BIT);
        }
        let request = Input::new(r.request, Pull::None);
        let ready = Output::new(r.ready, Level::Low, Speed::VeryHigh);
        let have_data = Output::new(r.have_data, Level::Low, Speed::VeryHigh);
        let pf7 = OutputOpenDrain::new(r.pf7, Level::High, Speed::Low);
        // Reset pulse as in the stock app (0x0805D9B8), then let it run.
        let mut reset = Output::new(r.reset, Level::Low, Speed::Low);
        Timer::after_millis(2).await;
        reset.set_high();
        Some(Self {
            request,
            ready,
            have_data,
            _reset: reset,
            _pf7: pf7,
            rx,
            tx,
            sequence: 0,
            next_heartbeat: Instant::now() + HEARTBEAT_PERIOD,
            heartbeat_wanted: false,
            heart_logged: 0,
            revision: 0,
            revision_asked: false,
            revision_answers: 0,
            pin_logged: 0,
            setup_due: None,
            setup_sequence: 0,
            setup_tries: 0,
            setup_acknowledged: false,
            settings_sent: false,
            acks: [(0, 0); 8],
            acks_pending: 0,
            blocks_received: 0,
            counts: [0; KINDS],
            seen: [false; KINDS],
            next_summary: Instant::now() + Duration::from_secs(60),
            readings: Readings::default(),
            last_key: None,
            key_pending: None,
        })
    }

    /// Stop the SPI and both DMA channels and empty the receive FIFO.
    fn idle(&mut self) {
        unsafe {
            write_volatile(dma(RX_CHANNEL, CCR), 0);
            write_volatile(dma(TX_CHANNEL, CCR), 0);
            write_volatile(SPI_CR2, CR2_8BIT);
            write_volatile(SPI_CR1, CR1_SLAVE);
            for _ in 0..8 {
                let _ = read_volatile(SPI_DR);
            }
            let _ = read_volatile(SPI_SR);
            write_volatile(DMA_IFCR, 0xF << (4 * (RX_CHANNEL - 1)) | 0xF << (4 * (TX_CHANNEL - 1)));
        }
    }

    /// Wait until a DMA channel has moved all its bytes. False on timeout.
    async fn wait_done(channel: usize, limit: Duration) -> bool {
        let end = Instant::now() + limit;
        while unsafe { read_volatile(dma(channel, CNDTR)) } != 0 {
            if Instant::now() > end {
                return false;
            }
            Timer::after_micros(200).await;
        }
        true
    }

    /// Receive one block the Apollo has announced. False on timeout.
    async fn receive_block(&mut self) -> bool {
        self.idle();
        unsafe {
            write_volatile((DMAMUX1 + 4 * (RX_CHANNEL - 1)) as *mut u32, REQUEST_SPI1_RX);
            write_volatile(dma(RX_CHANNEL, CPAR), SPI_DR as u32);
            write_volatile(dma(RX_CHANNEL, CMAR), self.rx.as_mut_ptr() as u32);
            write_volatile(dma(RX_CHANNEL, CNDTR), BLOCK as u32);
            write_volatile(dma(RX_CHANNEL, CCR), 1 << 7 | 1); // memory increment, enable
            write_volatile(SPI_CR2, CR2_8BIT | CR2_RXDMAEN);
            write_volatile(SPI_CR1, CR1_SLAVE | CR1_SPE);
        }
        self.ready.set_high();
        let done = Self::wait_done(RX_CHANNEL, Duration::from_secs(1)).await;
        self.ready.set_low();
        self.idle();
        done
    }

    /// Send the block in `self.tx`. False on timeout.
    async fn send_block(&mut self) -> bool {
        self.idle();
        unsafe {
            write_volatile((DMAMUX1 + 4 * (TX_CHANNEL - 1)) as *mut u32, REQUEST_SPI1_TX);
            write_volatile(dma(TX_CHANNEL, CPAR), SPI_DR as u32);
            write_volatile(dma(TX_CHANNEL, CMAR), self.tx.as_ptr() as u32);
            write_volatile(dma(TX_CHANNEL, CNDTR), BLOCK as u32);
            write_volatile(dma(TX_CHANNEL, CCR), 1 << 7 | 1 << 4 | 1); // increment, memory -> SPI, enable
            write_volatile(SPI_CR2, CR2_8BIT | CR2_TXDMAEN);
            write_volatile(SPI_CR1, CR1_SLAVE | CR1_SPE);
        }
        self.have_data.set_high();
        let mut done = Self::wait_done(TX_CHANNEL, Duration::from_secs(1)).await;
        if done {
            // The last bytes are still in the SPI's FIFO: wait until it is
            // empty (FTLVL, bits 12:11) and the bus is idle (BSY, bit 7).
            let end = Instant::now() + Duration::from_millis(100);
            while unsafe { read_volatile(SPI_SR) } & (0b11 << 11 | 1 << 7) != 0 {
                if Instant::now() > end {
                    done = false;
                    break;
                }
                Timer::after_micros(200).await;
            }
        }
        self.have_data.set_low();
        self.idle();
        done
    }

    /// Append one frame with protobuf `body` to the transmit block at
    /// `offset`; returns the offset after it. Flags and the two u32 stay
    /// zero, as in the stock heartbeat.
    fn push_frame(&mut self, offset: usize, body: &[u8]) -> usize {
        let n = body.len();
        let f = &mut self.tx[offset..offset + 13 + n];
        f.fill(0);
        f[0] = 0x7E;
        f[2] = n as u8;
        f[12..12 + n].copy_from_slice(body);
        f[12 + n] = crc8(&f[1..12 + n]);
        offset + 13 + n
    }

    /// Start a message body: field 1 = our next sequence number.
    fn start_body(&mut self, body: &mut [u8; 24]) -> usize {
        self.sequence += 1;
        body[0] = 0x08;
        1 + put_varint(&mut body[1..], self.sequence)
    }

    /// Call often. Takes a block if the Apollo announces one, answers its
    /// heartbeat, and sends our own when it is due.
    pub async fn service(&mut self, log: &mut Log) {
        if self.request.is_high() {
            if self.receive_block().await {
                self.blocks_received += 1;
                self.digest_block(log);
            } else {
                let _ = writeln!(log.stamp(), "apollo: announced a block but did not clock it");
            }
        }
        if self.acks_pending > 0 {
            // One block with an acknowledgement frame per pending entry:
            // sub-message 3 { 1: sequence, 2: kind }.
            self.tx.fill(0);
            let mut offset = 0;
            for i in 0..self.acks_pending {
                let (sequence, kind) = self.acks[i];
                let mut body = [0u8; 24];
                let mut n = self.start_body(&mut body);
                let mut inner = [0u8; 12];
                inner[0] = 0x08;
                let mut m = 1 + put_varint(&mut inner[1..], sequence);
                inner[m] = 0x10;
                m += 1;
                m += put_varint(&mut inner[m..], kind);
                body[n] = (ACK << 3 | 2) as u8;
                body[n + 1] = m as u8;
                body[n + 2..n + 2 + m].copy_from_slice(&inner[..m]);
                n += 2 + m;
                offset = self.push_frame(offset, &body[..n]);
            }
            self.acks_pending = 0;
            if !self.send_block().await {
                let _ = writeln!(log.stamp(), "apollo: acknowledgement not taken within 1 s");
            }
        }
        if self.heartbeat_wanted || Instant::now() >= self.next_heartbeat {
            // field 20 (heartbeat sub-message), empty
            let mut body = [0u8; 24];
            let n = self.start_body(&mut body);
            body[n..n + 3].copy_from_slice(&[0xA2, 0x01, 0x00]);
            self.tx.fill(0);
            self.push_frame(0, &body[..n + 3]);
            self.heartbeat_wanted = false;
            if self.send_block().await {
                self.next_heartbeat = Instant::now() + HEARTBEAT_PERIOD;
                let _ = writeln!(log.stamp(), "apollo: heartbeat sent (sequence {})", self.sequence);
            } else {
                // Try again on a later call, but not in a tight loop.
                self.next_heartbeat = Instant::now() + Duration::from_secs(1);
                let _ = writeln!(log.stamp(), "apollo: heartbeat not taken within 1 s");
            }
        }
        if self.revision_asked {
            self.revision_asked = false;
            self.revision_answers += 1;
            if ANSWER_REVISION {
                if self.revision_answers <= 5 {
                    let _ = writeln!(log.stamp(), "apollo: asked for the board revision; answering {}", self.revision);
                }
                self.send(messages::PIN, messages::revision_answer(self.revision).as_bytes(), log).await;
            } else if self.revision_answers == 1 {
                let _ = writeln!(log.stamp(), "apollo: asked for the board revision (not answered yet)");
            }
        }
        // What the stock app sends when the Apollo has announced itself
        // (see `messages::time`): the time and our version once, then the
        // setup status. Without the setup status the Apollo's wear
        // detection stays off. Measured: sent right after the first
        // heartbeat, before the Apollo's announcement, the Apollo takes
        // the setup status but does not act on it; so it is repeated until
        // the Apollo acknowledges it.
        if SEND_TIME_AND_VERSION && self.setup_tries == 0 && self.setup_due.is_some_and(|due| Instant::now() >= due) {
            match rtc::now() {
                Some(t) => {
                    self.send(messages::TIME, messages::time(t.unix_seconds()).as_bytes(), log).await;
                }
                None => {
                    let _ = writeln!(log.stamp(), "apollo: no time to send, the clock is not set");
                }
            }
            self.send(messages::VERSION, messages::version(VERSION_TEXT).as_bytes(), log).await;
        }
        if !self.setup_acknowledged && self.setup_tries < SETUP_TRIES && self.setup_due.is_some_and(|due| Instant::now() >= due) {
            self.setup_tries += 1;
            self.setup_due = Some(Instant::now() + SETUP_RETRY);
            if self.send(messages::SETUP_STATUS, messages::setup_finished().as_bytes(), log).await {
                self.setup_sequence = self.sequence;
            }
            if self.setup_tries == SETUP_TRIES {
                let _ = writeln!(log.stamp(), "apollo: setup status never acknowledged; wear detection will stay off");
            }
        }
        // The settings follow the setup status, as in the stock app. Until
        // this firmware has settings of its own: everything off.
        if SEND_SETTINGS && self.setup_acknowledged && !self.settings_sent {
            self.settings_sent = true;
            self.send(messages::DO_NOT_DISTURB, messages::do_not_disturb(false, (0, 1439), None).as_bytes(), log).await;
            self.send(messages::SEDENTARY, messages::sedentary(None, None).as_bytes(), log).await;
        }
        if Instant::now() >= self.next_summary {
            self.next_summary = Instant::now() + Duration::from_secs(60);
            if self.counts.iter().any(|&c| c != 0) {
                let _ = write!(log.stamp(), "apollo: last minute, messages by kind:");
                for (kind, count) in self.counts.iter_mut().enumerate() {
                    if *count != 0 {
                        let _ = write!(log, " {}x{}", kind, count);
                        *count = 0;
                    }
                }
                let _ = writeln!(log);
            }
            self.log_readings(log);
        }
    }

    /// Send one message of `kind` with the given sub-message. False if the
    /// Apollo did not take the block within a second. The Apollo answers
    /// with an acknowledgement, which `digest_block` logs.
    pub async fn send(&mut self, kind: u32, payload: &[u8], log: &mut Log) -> bool {
        let mut body = [0u8; 80];
        if payload.len() > body.len() - 16 {
            return false;
        }
        self.sequence += 1;
        body[0] = 0x08;
        let mut n = 1 + put_varint(&mut body[1..], self.sequence);
        n += put_varint(&mut body[n..], kind << 3 | 2);
        n += put_varint(&mut body[n..], payload.len() as u32);
        body[n..n + payload.len()].copy_from_slice(payload);
        n += payload.len();
        self.tx.fill(0);
        self.push_frame(0, &body[..n]);
        let taken = self.send_block().await;
        let _ = writeln!(
            log.stamp(),
            "apollo: kind {} {} (sequence {}):",
            kind,
            if taken { "sent" } else { "NOT taken within 1 s" },
            self.sequence
        );
        log.hex(&body[..n]);
        taken
    }

    /// Vibrate: `count` pulses of `on_ms`, `off_ms` apart.
    pub async fn vibrate(&mut self, on_ms: u16, off_ms: u16, count: u8, log: &mut Log) -> bool {
        self.send(messages::MOTOR, messages::motor_pulses(on_ms, off_ms, count).as_bytes(), log).await
    }

    /// Tell the driver the board revision, for the Apollo's question.
    pub fn set_revision(&mut self, revision: u8) {
        self.revision = revision;
    }

    /// Start or stop a heart-rate measurement. Results arrive in
    /// `readings.heart_bpm`.
    pub async fn measure_heart_rate(&mut self, on: bool, log: &mut Log) -> bool {
        if !on {
            self.readings.heart_bpm = None;
        }
        self.heart_logged = 0;
        self.send(messages::HEART_RATE_SET, messages::heart_rate(on).as_bytes(), log).await
    }

    /// Stop a vibration that is still going.
    #[allow(dead_code)] // not needed yet: the pulses in use end by themselves
    pub async fn stop_vibration(&mut self, log: &mut Log) -> bool {
        self.send(messages::MOTOR, messages::motor_stop().as_bytes(), log).await
    }

    /// The lower button's most significant event since the last call.
    pub fn take_key(&mut self) -> Option<messages::KeyEvent> {
        self.key_pending.take()
    }

    fn log_readings(&self, log: &mut Log) {
        let r = &self.readings;
        let _ = write!(log.stamp(), "sensors:");
        if let Some((percent, mv, usb)) = r.battery {
            let _ = write!(log, " battery {}% {:.0} mV{}", percent, mv, if usb { " (USB power)" } else { "" });
        }
        if let Some([a, b, c]) = r.temperatures {
            let _ = write!(log, ", temperatures {:.1} {:.1} {:.1} C", a, b, c);
        }
        if let Some((pascal, metres)) = r.barometer {
            let _ = write!(log, ", pressure {:.1} hPa (altitude {:.0} m)", pascal / 100.0, metres);
        }
        if let Some(wear) = r.wear {
            let _ = write!(log, ", {}", if wear.on_wrist { "worn" } else { "not worn" });
        }
        let _ = writeln!(log);
    }

    /// Go through the frames of the block just received: count them, note a
    /// heartbeat request, and log the first frame of every kind in full.
    fn digest_block(&mut self, log: &mut Log) {
        let block: &[u8; BLOCK] = self.rx;
        let mut frames = 0;
        let mut i = 0;
        while i + 13 <= BLOCK {
            if block[i] != 0x7E || block[i + 1] != 0 {
                i += 1;
                continue;
            }
            let n = block[i + 2] as usize;
            let end = i + 12 + n;
            if end >= BLOCK || crc8(&block[i + 1..end]) != block[end] {
                i += 1;
                continue;
            }
            frames += 1;
            let body = &block[i + 12..end];
            // Body: field 1 = sequence (tag byte 0x08), then one sub-message.
            let mut sequence = None;
            let mut rest = body;
            if let [0x08, tail @ ..] = body {
                if let Some((value, used)) = varint(tail) {
                    sequence = Some(value);
                    rest = &tail[used..];
                }
            }
            // The one sub-message: its field number is the kind.
            let (kind, payload) = match Fields(rest).next() {
                Some((kind, Value::Bytes(payload))) => (Some(kind), payload),
                Some((kind, _)) => (Some(kind), &rest[..0]),
                None => (None, &rest[..0]),
            };
            if let Some(kind) = kind {
                self.readings.update(kind, payload);
                if kind == messages::KEY {
                    let (event, at) = messages::key_event(payload);
                    self.last_key = Some(event);
                    // A click or long press outranks the press and release around it.
                    if !matches!(self.key_pending, Some(messages::KeyEvent::Clicked | messages::KeyEvent::LongPressed)) {
                        self.key_pending = Some(event);
                    }
                    let _ = writeln!(log.stamp(), "button lower: {} (Apollo time {} ms)", event.name(), at);
                }
            }
            if kind == Some(messages::PIN) {
                if messages::asks_for_revision(payload) {
                    self.revision_asked = true;
                } else if self.pin_logged < 10 {
                    // Anything else of this kind is rare and worth a line.
                    self.pin_logged += 1;
                    let _ = write!(log.stamp(), "apollo: kind 7:");
                    for (tag, value) in Fields(payload) {
                        if let Some(v) = value.as_u32() {
                            let _ = write!(log, " {}={}", tag, v);
                        }
                    }
                    let _ = writeln!(log);
                }
            }
            if kind == Some(messages::SENSOR_NAMES) && self.setup_due.is_none() {
                // The Apollo has announced itself: the stock app's moment
                // for its start-up messages.
                self.setup_due = Some(Instant::now());
            }
            if kind == Some(messages::WEAR) {
                // Rare (only when the state changes), so every one is logged.
                if let Some(wear) = self.readings.wear {
                    let _ = writeln!(
                        log.stamp(),
                        "apollo: {}: {} (capacitive status {:#08x})",
                        if wear.on_wrist { "on the wrist" } else { "not on the wrist" },
                        wear.reason.name(),
                        wear.capacitive_status
                    );
                }
            }
            if kind == Some(messages::OFF_WRIST) {
                if let Some(long) = self.readings.off_wrist_long {
                    let _ = writeln!(log.stamp(), "apollo: off the wrist for longer than the timeout: {}", long);
                }
            }
            if kind == Some(messages::HEART) && self.heart_logged < 300 {
                // Every report in full while this is new, to learn the fields.
                self.heart_logged += 1;
                let _ = write!(log.stamp(), "apollo: heart report:");
                for (tag, value) in Fields(payload) {
                    if let Some(v) = value.as_u32() {
                        let _ = write!(log, " {}={}", tag, v);
                    }
                }
                let _ = writeln!(log);
            }
            if kind == Some(HEARTBEAT) {
                self.heartbeat_wanted = true;
            }
            if kind == Some(ACK) {
                // Acknowledgements of our heartbeats come every few seconds
                // and are not worth a line; the others are.
                let field = |n: u32| Fields(payload).find(|&(tag, _)| tag == n).and_then(|(_, v)| v.as_u32()).unwrap_or(0);
                if field(2) != HEARTBEAT {
                    let _ = writeln!(log.stamp(), "apollo: acknowledged our kind {} (sequence {})", field(2), field(1));
                }
                // Matched by sequence number: the kind the Apollo names is
                // not always the one it answers.
                if self.setup_tries > 0 && field(1) == self.setup_sequence {
                    self.setup_acknowledged = true;
                }
            }
            if block[i + 3] & FLAG_WANTS_ACK != 0 && self.acks_pending < self.acks.len() {
                if let (Some(sq), Some(k)) = (sequence, kind) {
                    self.acks[self.acks_pending] = (sq, k);
                    self.acks_pending += 1;
                }
            }
            let slot = kind.map(|k| k as usize).filter(|&k| k < KINDS);
            if let Some(k) = slot {
                self.counts[k] = self.counts[k].saturating_add(1);
            }
            let first = match slot {
                Some(k) => !core::mem::replace(&mut self.seen[k], true),
                None => true,
            };
            if first {
                let _ = writeln!(
                    log.stamp(),
                    "apollo: first message of kind {:?} (sequence {:?}, flags {:#04x}, {} bytes):",
                    kind,
                    sequence,
                    block[i + 3],
                    n
                );
                log.hex(body);
            }
            i = end + 1;
        }
        if frames == 0 {
            let _ = writeln!(log.stamp(), "apollo: block {} has no valid frame; first 48 bytes:", self.blocks_received);
            log.hex(&block[..48]);
        }
    }
}

/// Write a protobuf varint; returns the number of bytes used.
fn put_varint(out: &mut [u8], mut value: u32) -> usize {
    let mut n = 0;
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out[n] = byte;
            return n + 1;
        }
        out[n] = byte | 0x80;
        n += 1;
    }
}

/// Read a protobuf varint; returns (value, bytes used).
fn varint(data: &[u8]) -> Option<(u32, usize)> {
    let mut value = 0u32;
    for (i, &b) in data.iter().enumerate().take(5) {
        value |= ((b & 0x7F) as u32) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}
