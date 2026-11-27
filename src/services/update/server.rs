//! Firmware update over Bluetooth, speaking the stock firmware's protocol.
//!
//! The PC-side tool (`opw_bt.py`, and `tools/ota.sh` on top of it) works
//! unchanged against this app and against the stock app:
//!
//!   "00AT^BLUETOOTH"                 -> "W301RUST OK\r\n"  (handshake after
//!                                       connecting; the stock app answers
//!                                       "OK\r\n", so the PC can tell which
//!                                       firmware it is talking to)
//!   "300\r"                          -> "300"     (start of a file push)
//!   "33<ext>,<path>,<size>\r"        -> "330"     (open the receive channel)
//!   chunks of up to 47996 bytes, each followed by its CRC32 (little-endian)
//!                                    -> "310" accepted / "311" send again
//!   "01FI^SETUNCOMPFLAG"             -> "OK\r\n"
//!   "01FI^OTAUPDATE"                 -> "OK\r\n", then install and reboot
//!
//! Only one file is accepted: the update package `1:B280STM32_ota.bin`
//! (see `tools/mkota.py`): a 0xB4-byte header, the app image, and the
//! image's CRC32. The image is written straight to
//! `1:ota/stm32/stm32_app.bin`, the file the bootloader installs when the
//! flag record says so (`ticket::Source::Ota`).
//!
//! Files can also be fetched from the watch, as with the stock firmware:
//!
//!   "061<volume>:<path>\r"           -> frames of 0x40, length (u32 big-endian),
//!                                       data; the PC answers each with "062\r"
//!   after the last frame             -> "AT^FILE_EXPORT_OVER"
//!
//! Staying in Rust: every boot first books the return to the stock firmware
//! (see `boot::ticket`). Once a PC has completed the handshake - proof that this
//! image can be reached and replaced - the booking is changed to "install
//! the image file again", so a reset of any kind comes back to this app.
//! The file is `1:ota/stm32/stm32_app.bin`, which is what the bootloader
//! flashed us from. As soon as a new package starts to overwrite that file,
//! the booking goes back to the stock firmware until the new image is
//! complete and verified.
//!
//! One extra command: "00AT^W301STOCK" -> "OK\r\n", then reset. With the
//! return ticket booked at boot, that brings the stock firmware back.

use core::fmt::Write;

use embassy_time::{Duration, Instant, Timer};

use crate::drivers::cypress::{self, Link};
use crate::services::clock;
use crate::services::log::Log;
use crate::services::progress::{self, Transfer};
use crate::boot::ticket;
use crate::services::update::package::{crc32_update, Package, PACKAGE_HEADER};
use crate::services::storage::{self, FileReader, FileWriter, Partition};

// Serial-port (SPP) packets of the Cypress chip, group 0x04.
/// Event: a peer connected. Payload: address (6, reversed), handle (u16), 1 byte.
const EVT_SPP_CONNECTED: u16 = 0x0401;
/// Event: the connection is gone. Payload: handle (u16).
const EVT_SPP_DISCONNECTED: u16 = 0x0404;
/// Event: received data. Payload: handle (u16), data.
const EVT_SPP_RX_DATA: u16 = 0x0406;
/// Event: the chip has sent our data on. Payload: handle (u16), status.
const EVT_SPP_TX_COMPLETE: u16 = 0x0405;
/// Command: send data. Payload: handle (u16), data (stock: 0x080A7720).
const CMD_SPP_DATA: u16 = 0x0403;

/// How long a freshly booted image waits for a PC before giving up.
const FIRST_CONTACT_LIMIT: Duration = Duration::from_secs(30 * 60);
const CHUNK_DATA: usize = 47996;
/// Data bytes per frame when sending a file (the stock firmware uses the same).
const EXPORT_FRAME: usize = 0xA000;
/// Largest data part of one serial-port packet (stock limit at 0x080A7748).
const SPP_MAX_DATA: usize = 960;
const PACKAGE_PATH: &str = "1:B280STM32_ota.bin";
#[derive(Clone, Copy)]
enum State {
    Commands,
    Receiving { total: usize, received: usize },
    /// A package was ended on the watch: what the sender still sends is
    /// dropped and answered with "311" (once every `answer_at` at most),
    /// at which it gives up after three tries; over when it disconnects
    /// or has been silent for `REFUSE_FOR`.
    Refusing { since: Instant, answer_at: Instant },
    /// A PC has offered a package ("AT^W301OFFER") and waits for the
    /// answer, which the wearer gives on the display, until `until`.
    Offered { handle: [u8; 2], until: Instant, what: Transfer },
}

/// How long an offer waits for the wearer, and how long after a yes the
/// package may begin.
const OFFER_FOR: Duration = Duration::from_secs(30);
const ACCEPTED_FOR: Duration = Duration::from_secs(60);
/// How long an accepted transfer is shown while the PC has not begun it.
const AWAITED_FOR: Duration = Duration::from_secs(20);

/// How long after the last data of a refused package the sender counts
/// as gone.
const REFUSE_FOR: Duration = Duration::from_secs(15);

/// What one `Server::step` did.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Nothing arrived within the waiting time.
    Idle,
    /// A packet from the Bluetooth chip was dealt with.
    Handled,
}

/// The update protocol's state between packets.
pub struct Server {
    chunk: &'static mut [u8; CHUNK_DATA + 4],
    chunk_len: usize,
    state: State,
    /// When the package under way began.
    receiving_since: Instant,
    /// What the wearer accepted, and until when it may begin.
    accepted: Option<(Transfer, Instant)>,
    /// An accepted transfer is on the display and has not begun: until
    /// when it stays there.
    awaited: Option<Instant>,
    package: Option<Package>,
    /// Length of a complete, verified image.
    ready_image: Option<usize>,
    rx_errors_logged: u32,
    // Does the image file still hold the image we are running, and is the
    // bootloader booked to reinstall it after a reset?
    image_file_is_ours: bool,
    staying: bool,
    handshaken: bool,
    first_contact_deadline: Instant,
    /// Volume 1: the package and the image file.
    image_partition: Partition,
    /// A peer is connected to the serial port.
    pub connected: bool,
    /// Pairing a new device: it gets the chip's packets that are its own.
    pub pairing: crate::services::pairing::Pairing,
}

impl Server {
    /// `None` on the second call.
    pub fn new(image_partition: Partition) -> Option<Self> {
        let chunk = cortex_m::singleton!(: [u8; CHUNK_DATA + 4] = [0; CHUNK_DATA + 4])?;
        Some(Self {
            chunk,
            chunk_len: 0,
            state: State::Commands,
            receiving_since: Instant::now(),
            accepted: None,
            awaited: None,
            package: None,
            ready_image: None,
            rx_errors_logged: 0,
            image_file_is_ours: true,
            staying: false,
            handshaken: false,
            first_contact_deadline: Instant::now() + FIRST_CONTACT_LIMIT,
            image_partition,
            connected: false,
            pairing: crate::services::pairing::Pairing::new(),
        })
    }

    /// True while a package is streaming in: the sender waits for every
    /// answer, so nothing slow should run in between.
    pub fn transferring(&self) -> bool {
        matches!(self.state, State::Receiving { .. })
    }

    /// Whether the wearer accepted an offer of `what` shortly before; a
    /// yes is good for one transfer.
    fn take_accepted(&mut self, what: Transfer) -> bool {
        match self.accepted {
            Some((accepted, until)) if accepted == what => {
                self.accepted = None;
                self.awaited = None;
                Instant::now() < until
            }
            _ => false,
        }
    }

    /// Whether a reset comes back to this firmware: the bootloader is
    /// booked to install it again, which happens once a PC has been in
    /// touch since the start. Otherwise a reset goes to the stock
    /// firmware.
    pub fn staying(&self) -> bool {
        self.staying
    }

    /// Wait up to 20 ms for a packet from the Bluetooth chip and deal with
    /// it. An install or a return request ends in a reset from in here.
    pub async fn step(&mut self, link: &mut Link, wait: Duration, log: &mut Log) -> Activity {
        // The package was ended on the watch (`progress::cancel`).
        if let State::Receiving { total, received } = self.state {
            if progress::cancelled() {
                let _ = writeln!(log.stamp(), "ota: transfer ended on the watch at {} of {} bytes", received, total);
                if let Some(p) = self.package.take() {
                    let _ = p.finish(total);
                }
                self.state = State::Refusing { since: Instant::now(), answer_at: Instant::now() };
                self.chunk_len = 0;
                progress::end();
                let _ = log.flush();
            }
        }
        // An offer is answered once the wearer has decided, or with a no
        // when the time is up.
        if let State::Offered { handle, until, what } = self.state {
            let decision = progress::decision().or((Instant::now() > until).then_some(false));
            if let Some(accept) = decision {
                self.state = State::Commands;
                progress::end();
                self.accepted = accept.then(|| (what, Instant::now() + ACCEPTED_FOR));
                // After a yes the display says that the PC is waited
                // for, until it begins: a file is fetched over a new
                // connection, some seconds later.
                self.awaited = None;
                if accept && what != Transfer::OfferedPut {
                    progress::begin(Transfer::Waiting);
                    self.awaited = Some(Instant::now() + AWAITED_FOR);
                }
                let _ = writeln!(log.stamp(), "bluetooth: the offered transfer was {}", if accept { "accepted on the watch" } else if progress::decision().is_some() { "denied on the watch" } else { "not answered in time: denied" });
                let answer: &[u8] = if accept { b"OK\r\n" } else { b"ERROR DENIED\r\n" };
                let mut out = [0u8; 20];
                out[..2].copy_from_slice(&handle);
                out[2..2 + answer.len()].copy_from_slice(answer);
                if !link.send(CMD_SPP_DATA, &out[..2 + answer.len()]).await {
                    let _ = writeln!(log.stamp(), "bluetooth: answer could not be sent");
                }
                let _ = log.flush();
            }
        }
        // An accepted transfer the PC does not begin, or that is ended
        // on the watch before it began, is taken off the display again;
        // ended, it also loses its yes.
        if let Some(until) = self.awaited {
            if progress::cancelled() {
                let _ = writeln!(log.stamp(), "bluetooth: the accepted transfer was ended on the watch before it began");
                self.accepted = None;
            }
            if progress::cancelled() || Instant::now() > until {
                self.awaited = None;
                progress::end();
            }
        }
        if let State::Refusing { since, .. } = self.state {
            if since.elapsed() > REFUSE_FOR {
                self.state = State::Commands;
            }
        }
        let Some(packet) = link.receive_idle(wait).await else {
            if link.rx_errors != self.rx_errors_logged {
                self.rx_errors_logged = link.rx_errors;
                let _ = writeln!(log.stamp(), "cypress: {} UART receive errors so far", self.rx_errors_logged);
            }
            // An image nobody has reached half an hour after boot is of no
            // use and cannot be replaced: give up. No handshake means the
            // booking is still the stock firmware, so this reset goes there.
            if !self.handshaken && Instant::now() > self.first_contact_deadline {
                let _ = writeln!(log.stamp(), "ota: no PC within 30 minutes of boot, returning to stock");
                let _ = log.flush();
                crate::hal::rtc::reset_on_the_second();
            }
            return Activity::Idle;
        };
        let opcode = packet.opcode;
        if opcode != EVT_SPP_RX_DATA || packet.payload.len() < 2 {
            match opcode {
                EVT_SPP_CONNECTED => {
                    self.connected = true;
                    // Who it is, for the page of paired devices: the
                    // peer's address comes first, last byte first.
                    if packet.payload.len() >= 6 {
                        crate::services::pairing::serial_peer(&packet.payload[..6]);
                    }
                    let _ = writeln!(log.stamp(), "bluetooth: peer connected");
                }
                EVT_SPP_DISCONNECTED => {
                    self.connected = false;
                    let _ = writeln!(log.stamp(), "bluetooth: peer disconnected");
                    if let State::Receiving { total, received } = self.state {
                        let _ = writeln!(log.stamp(), "ota: transfer aborted at {} of {} bytes", received, total);
                        if let Some(p) = self.package.take() {
                            let _ = p.finish(total);
                        }
                        self.state = State::Commands;
                        self.chunk_len = 0;
                        progress::end();
                    }
                    if matches!(self.state, State::Offered { .. }) {
                        progress::end();
                    }
                    if matches!(self.state, State::Refusing { .. } | State::Offered { .. }) {
                        self.state = State::Commands;
                    }
                    let _ = log.flush();
                }
                EVT_SPP_TX_COMPLETE => {} // one per answer we send; not worth a log line
                _ => {
                    if !self.pairing.event(opcode, packet.payload, log) {
                        cypress::log_packet(log, &packet);
                    }
                }
            }
            return Activity::Handled;
        }
        let handle = [packet.payload[0], packet.payload[1]];
        let data = &packet.payload[2..];
    
        // What to answer, decided while `packet` (borrowed from the link) is alive.
        let mut answer: &[u8] = b"";
        // An answer that is put together here, where `answer` is fixed text.
        let mut report = [0u8; 96];
        let mut report_len = 0;
        let mut install = false;
        let mut to_stock = false;
        let mut export: Option<([u8; 128], usize)> = None; // "<volume>:<path>" to send to the PC
    
        match self.state {
            State::Commands => {
                let text = core::str::from_utf8(data).unwrap_or("");
                let _ = writeln!(log.stamp(), "bluetooth rx: {:?}", text);
                if text.contains("AT^W301OFFER") {
                    // Answered when the wearer has decided (see the top).
                    let what = if text.contains("OFFER=GET") {
                        Transfer::OfferedGet
                    } else if text.contains("OFFER=PUT") {
                        Transfer::OfferedPut
                    } else {
                        Transfer::Offered
                    };
                    self.state = State::Offered { handle, until: Instant::now() + OFFER_FOR, what };
                    progress::begin(what);
                    let _ = writeln!(log.stamp(), "bluetooth: a transfer is offered ({:?}); waiting for the wearer", what);
                } else if text.contains("AT^BLUETOOTH") {
                    answer = b"W301RUST OK\r\n";
                    self.handshaken = true;
                    if !self.staying && self.image_file_is_ours {
                        // Our own first 512 bytes, as flashed.
                        let own = unsafe { core::slice::from_raw_parts(0x0801_E000 as *const u8, 512) };
                        if storage::file_starts_with(self.image_partition, "ota/stm32/stm32_app.bin", own)
                            && ticket::request_install(ticket::Source::Ota, 0x8B).is_ok()
                        {
                            self.staying = true;
                            let _ = writeln!(log.stamp(), "ota: a reset now comes back to this image");
                        } else {
                            self.image_file_is_ours = false;
                            let _ = writeln!(log.stamp(), "ota: image file is not this image; a reset returns to stock");
                        }
                    }
                } else if text == "300\r" {
                    answer = b"300";
                } else if let Some(args) = text.strip_prefix("33").and_then(|t| t.strip_suffix('\r')) {
                    // "<ext>,<path>,<size>"
                    let mut parts = args.split(',');
                    let (_ext, path, size) = (parts.next(), parts.next(), parts.next());
                    let size = size.and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                    answer = b"ERROR\r\n";
                    // Only a package the wearer said yes to, shortly before.
                    let accepted = self.take_accepted(Transfer::Offered);
                    if !accepted {
                        let _ = writeln!(log.stamp(), "ota: a package that was not offered and accepted on the watch is refused");
                    }
                    let acceptable = accepted && path == Some(PACKAGE_PATH) && size > PACKAGE_HEADER + 4;
                    let mut rebooked = true;
                    if acceptable {
                        self.ready_image = None;
                        // The image file is about to be overwritten: until the
                        // new one is verified, a reset must go to the stock app.
                        self.image_file_is_ours = false;
                        if self.staying {
                            self.staying = false;
                            rebooked = ticket::request_install(ticket::Source::PacketA, 0x8C).is_ok();
                            if !rebooked {
                                // Leave the file alone while the bootloader would still install it.
                                let _ = writeln!(log.stamp(), "ota: could not rebook the stock firmware");
                            }
                        }
                    }
                    if acceptable && rebooked {
                        match FileWriter::rewrite(self.image_partition, &["OTA", "STM32"], "stm32_app.bin") {
                            Ok(writer) => {
                                self.package = Some(Package::new(writer));
                                self.state = State::Receiving { total: size, received: 0 };
                                self.chunk_len = 0;
                                self.receiving_since = Instant::now();
                                progress::begin(Transfer::Receiving);
                                answer = b"330";
                                let _ = writeln!(log.stamp(), "ota: receiving package, {} bytes", size);
                            }
                            Err(step) => {
                                let _ = writeln!(log.stamp(), "ota: cannot open the image file (step {})", step);
                            }
                        }
                    } else if accepted && !acceptable {
                        let _ = writeln!(log.stamp(), "ota: only {} can be pushed", PACKAGE_PATH);
                    }
                    // Accepted and shown, but it did not begin.
                    if accepted && !matches!(self.state, State::Receiving { .. }) {
                        progress::end();
                    }
                } else if let Some(what) = text.strip_prefix("061").and_then(|t| t.strip_suffix('\r')) {
                    if self.take_accepted(Transfer::OfferedGet) {
                        let mut name = [0u8; 128];
                        let n = what.len().min(name.len());
                        name[..n].copy_from_slice(&what.as_bytes()[..n]);
                        export = Some((name, n));
                    } else {
                        let _ = writeln!(log.stamp(), "export: {} was not offered and accepted on the watch: refused", what);
                        answer = b"ERROR\r\n";
                    }
                } else if text.contains("FILE_EXPORT_OVER") {
                    // the PC's closing word after a file export; no answer
                } else if text.contains("FI^SETUNCOMPFLAG") {
                    answer = b"OK\r\n";
                } else if text.contains("FI^OTAUPDATE") {
                    install = self.ready_image.is_some();
                    answer = if install { b"OK\r\n" } else { b"ERROR\r\n" };
                } else if text.contains("AT^W301TIME?") {
                    report_len = clock::report(&mut report);
                } else if let Some((_, time)) = text.split_once("AT^W301TIME=") {
                    answer = if clock::set_from_text(time.trim_end(), log) { b"OK\r\n" } else { b"ERROR\r\n" };
                } else if let Some((_, put)) = text.split_once("AT^W301PUT=") {
                    let accepted = self.take_accepted(Transfer::OfferedPut);
                    if !accepted {
                        let _ = writeln!(log.stamp(), "put: not offered and accepted on the watch: refused");
                    }
                    answer = if accepted && put_small_file(put.trim_end(), log) { b"OK\r\n" } else { b"ERROR\r\n" };
                } else if text.contains("AT^W301STOCK") {
                    to_stock = true;
                    answer = b"OK\r\n";
                } else {
                    answer = b"ERROR\r\n";
                }
            }
            // Nothing is taken while the wearer decides.
            State::Offered { .. } => {}
            State::Refusing { answer_at, .. } => {
                let now = Instant::now();
                if now >= answer_at {
                    answer = b"311";
                    self.state = State::Refusing { since: now, answer_at: now + Duration::from_millis(500) };
                } else {
                    self.state = State::Refusing { since: now, answer_at };
                }
            }
            State::Receiving { total, received } => {
                // Chunks are sent one at a time; the sender waits for 310/311.
                let expect = (total - received).min(CHUNK_DATA) + 4;
                let n = data.len().min(expect - self.chunk_len);
                self.chunk[self.chunk_len..self.chunk_len + n].copy_from_slice(&data[..n]);
                self.chunk_len += n;
                if self.chunk_len == expect {
                    let (body, crc) = self.chunk[..expect].split_at(expect - 4);
                    self.chunk_len = 0;
                    if !crc32_update(0xFFFF_FFFF, body) != u32::from_le_bytes([crc[0], crc[1], crc[2], crc[3]]) {
                        answer = b"311";
                    } else {
                        if let Some(p) = self.package.as_mut() {
                            p.feed(body, total);
                        }
                        let received = received + body.len();
                        answer = b"310";
                        progress::at(received, total);
                        if received == total {
                            self.state = State::Commands;
                            progress::end();
                            match self.package.take().map(|p| p.finish(total)) {
                                Some(Ok(len)) => {
                                    self.ready_image = Some(len);
                                    let _ = writeln!(log.stamp(), "ota: package complete after {} s, image of {} bytes verified", self.receiving_since.elapsed().as_secs(), len);
                                }
                                Some(Err(reason)) => {
                                    let _ = writeln!(log.stamp(), "ota: package rejected: {}", reason);
                                }
                                None => {}
                            }
                        } else {
                            self.state = State::Receiving { total, received };
                        }
                    }
                }
            }
        }
    
        if report_len > 0 {
            answer = &report[..report_len];
        }
        if !answer.is_empty() {
            let mut out = [0u8; 100];
            out[..2].copy_from_slice(&handle);
            out[2..2 + answer.len()].copy_from_slice(answer);
            if !link.send(CMD_SPP_DATA, &out[..2 + answer.len()]).await {
                let _ = writeln!(log.stamp(), "bluetooth: answer could not be sent");
            }
        }
    
        if let Some((name, n)) = export {
            let what = core::str::from_utf8(&name[..n]).unwrap_or("");
            // Write out what is still in RAM first, so a pulled log is complete.
            let _ = log.flush();
            let sent = export_file(link, handle, what, self.chunk).await;
            match sent {
                Ok(bytes) => {
                    let _ = writeln!(log.stamp(), "export: sent {} ({} bytes)", what, bytes);
                }
                Err(reason) => {
                    let _ = writeln!(log.stamp(), "export of {} failed: {}", what, reason);
                }
            }
        }
    
        if install || to_stock {
            Timer::after_millis(300).await; // let the answer reach the PC
            if install {
                let _ = writeln!(log.stamp(), "ota: installing the received image, rebooting");
                let _ = log.flush();
                // The bootloader installs 1:ota/stm32/stm32_app.bin on the next reset.
                let booked = ticket::request_install(ticket::Source::Ota, 0x8A);
                if booked.is_err() {
                    // The earlier ticket (back to the stock app) is still in place.
                    let _ = writeln!(log.stamp(), "ota: could not book the install");
                    let _ = log.flush();
                }
            } else {
                let _ = writeln!(log.stamp(), "bluetooth: return to the stock firmware requested");
                let _ = log.flush();
                if ticket::request_install(ticket::Source::PacketA, 0x8C).is_err() {
                    let _ = writeln!(log.stamp(), "bluetooth: could not book the stock firmware; staying");
                    let _ = log.flush();
                    return Activity::Handled;
                }
            }
            crate::hal::rtc::reset_on_the_second();
        }
        Activity::Handled
    }
}

/// Send one serial-port packet and wait until the chip has passed it on.
/// Returns true if the PC's frame confirmation ("062") arrived meanwhile:
/// the PC can answer before the chip reports the last packet as sent, and
/// that answer must not get lost.
async fn spp_send(link: &mut Link, handle: [u8; 2], data: &[u8]) -> Result<bool, &'static str> {
    let mut out = [0u8; 2 + SPP_MAX_DATA];
    out[..2].copy_from_slice(&handle);
    out[2..2 + data.len()].copy_from_slice(data);
    if !link.send(CMD_SPP_DATA, &out[..2 + data.len()]).await {
        return Err("the chip does not take data");
    }
    let mut confirmed = false;
    for _ in 0..6 {
        match link.receive(Duration::from_millis(500)).await {
            Some(p) if p.opcode == EVT_SPP_TX_COMPLETE => return Ok(confirmed),
            Some(p) if p.opcode == EVT_SPP_DISCONNECTED => return Err("the PC disconnected"),
            Some(p) if is_frame_confirmation(&p) => confirmed = true,
            _ => {}
        }
    }
    Err("no confirmation from the chip")
}

fn is_frame_confirmation(p: &cypress::Packet<'_>) -> bool {
    p.opcode == EVT_SPP_RX_DATA && p.payload.len() > 2 && p.payload[2..].starts_with(b"062")
}

/// Send the file named by `what` ("<volume>:<path>") to the PC in the stock
/// export format. `buf` is scratch space. Returns the number of bytes sent.
/// A missing file is answered like an empty one, with just the closing word.
async fn export_file(
    link: &mut Link,
    handle: [u8; 2],
    what: &str,
    buf: &mut [u8; CHUNK_DATA + 4],
) -> Result<usize, &'static str> {
    let mut reader = None;
    let mut problem = None;
    match what.split_once(':') {
        Some((volume, path)) => match volume.parse::<usize>() {
            Ok(v) if v < 4 => match storage::inspect(v) {
                Ok(partition) => match FileReader::open(partition, path) {
                    Ok(r) => reader = Some(r),
                    Err(_) => problem = Some("file not found"),
                },
                Err(_) => problem = Some("volume not readable"),
            },
            _ => problem = Some("no such volume"),
        },
        None => problem = Some("name must be <volume>:<path>"),
    }

    let mut total = 0usize;
    let mut result = Ok(());
    if let Some(reader) = reader.as_mut() {
        let mut left = reader.len() as usize;
        let whole = left;
        progress::begin(Transfer::Sending);
        while left > 0 && result.is_ok() {
            if progress::cancelled() {
                result = Err("ended on the watch");
                break;
            }
            let n = left.min(EXPORT_FRAME);
            // Frame: 0x40, length (u32 big-endian), data.
            buf[0] = 0x40;
            buf[1..5].copy_from_slice(&(n as u32).to_be_bytes());
            match reader.read(&mut buf[5..5 + n]) {
                Ok(got) if got == n => {}
                _ => {
                    result = Err("reading the file failed");
                    break;
                }
            }
            let mut confirmed = false;
            for piece in buf[..5 + n].chunks(SPP_MAX_DATA) {
                match spp_send(link, handle, piece).await {
                    Ok(early) => confirmed |= early,
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            if result.is_err() {
                break;
            }
            total += n;
            left -= n;
            progress::at(total, whole);
            // The PC confirms every frame with "062\r".
            for _ in 0..40 {
                if confirmed {
                    break;
                }
                match link.receive(Duration::from_millis(500)).await {
                    Some(p) if is_frame_confirmation(&p) => confirmed = true,
                    Some(p) if p.opcode == EVT_SPP_DISCONNECTED => break,
                    _ => {}
                }
            }
            if !confirmed {
                result = Err("the PC did not confirm a frame");
            }
        }
    }
    progress::end();
    if let Some(reader) = reader {
        reader.close();
    }
    result?;
    let _ = spp_send(link, handle, b"AT^FILE_EXPORT_OVER").await?;
    match problem {
        Some(reason) => Err(reason),
        None => Ok(total),
    }
}

/// "AT^W301PUT=<NAME>,<hex>": write a small file into the log folder of
/// the log volume, in place of one of that name. For the few files this
/// firmware reads but does not carry (the amplifier's patch and
/// settings, see `drivers::audio`). The name is a short one of capitals
/// and digits with an extension; the content comes as hex in the same
/// packet, so 440 bytes are the most.
fn put_small_file(put: &str, log: &mut Log) -> bool {
    let Some((name, hex)) = put.split_once(',') else {
        return false;
    };
    let named_well = name.len() <= 12
        && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.')
        && name.split_once('.').is_some_and(|(stem, ext)| (1..=8).contains(&stem.len()) && (1..=3).contains(&ext.len()) && !ext.contains('.'));
    // Not over the log, the settings or what the firmware keeps there.
    let reserved = ["RUST.LOG", "RUST.OLD", "RUST.CFG"].contains(&name);
    let mut content = [0u8; 440];
    let digits = hex.as_bytes();
    if !named_well || reserved || digits.is_empty() || digits.len() % 2 != 0 || digits.len() / 2 > content.len() {
        let _ = writeln!(log.stamp(), "put: refused (name {:?}, {} hex digits)", name, digits.len());
        return false;
    }
    let length = digits.len() / 2;
    for (byte, pair) in content.iter_mut().zip(digits.chunks_exact(2)) {
        let digit = |d: u8| (d as char).to_digit(16);
        let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
            let _ = writeln!(log.stamp(), "put: refused, {:?} is not hex", name);
            return false;
        };
        *byte = (high << 4 | low) as u8;
    }
    let Some(partition) = log.partition() else {
        return false;
    };
    let written = (|| {
        // The file has to exist before it can be rewritten.
        storage::append(partition, "LOG", name, &[]).map_err(|_| 1u8)?;
        let mut writer = FileWriter::rewrite(partition, &["LOG"], name).map_err(|_| 2u8)?;
        writer.write(&content[..length]).map_err(|()| 3u8)?;
        writer.finish().map_err(|()| 4u8)
    })();
    match written {
        Ok(()) => {
            let _ = writeln!(log.stamp(), "put: 3:log/{} written, {} bytes", name, length);
        }
        Err(step) => {
            let _ = writeln!(log.stamp(), "put: 3:log/{} could NOT be written (step {})", name, step);
        }
    }
    written.is_ok()
}
