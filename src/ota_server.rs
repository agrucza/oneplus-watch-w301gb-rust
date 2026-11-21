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
//! flag record says so (`ota_flag::Source::Ota`).
//!
//! Files can also be fetched from the watch, as with the stock firmware:
//!
//!   "061<volume>:<path>\r"           -> frames of 0x40, length (u32 big-endian),
//!                                       data; the PC answers each with "062\r"
//!   after the last frame             -> "AT^FILE_EXPORT_OVER"
//!
//! Staying in Rust: every boot first books the return to the stock firmware
//! (see `main`). Once a PC has completed the handshake - proof that this
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

use cortex_m::peripheral::SCB;
use embassy_time::{Duration, Instant, Timer};

use crate::apollo::Apollo;
use crate::buttons::Buttons;
use crate::cypress::{self, Link};
use crate::log::Log;
use crate::ota_flag;
use crate::package::{crc32_update, Package, PACKAGE_HEADER};
use crate::storage::{self, FileReader, FileWriter, Partition};

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
enum State {
    Commands,
    Receiving { total: usize, received: usize },
}

/// Serve the update protocol until an install or a return request resets
/// the watch. `image_partition` is volume 1, `log_partition` volume 3.
pub async fn run(
    link: &mut Link,
    log: &mut Log,
    mut apollo: Option<Apollo>,
    mut buttons: Buttons,
    image_partition: Partition,
    log_partition: Partition,
) -> ! {
    let Some(chunk) = cortex_m::singleton!(: [u8; CHUNK_DATA + 4] = [0; CHUNK_DATA + 4]) else {
        SCB::sys_reset(); // cannot happen: `run` is entered once
    };
    let mut chunk_len = 0usize;
    let mut state = State::Commands;
    let mut package: Option<Package> = None;
    let mut ready_image: Option<usize> = None; // length of a complete, verified image
    let mut rx_errors_logged = 0u32;
    // Does the image file still hold the image we are running, and is the
    // bootloader booked to reinstall it after a reset?
    let mut image_file_is_ours = true;
    let mut staying = false;
    let mut handshaken = false;
    let first_contact_deadline = Instant::now() + FIRST_CONTACT_LIMIT;

    let mut last_save = Instant::now();
    loop {
        // The Apollo's handshake cannot wait long, so look after it between
        // Bluetooth packets - but not while a package is streaming in.
        if let (Some(apollo), State::Commands) = (apollo.as_mut(), &state) {
            apollo.service(log).await;
        }
        buttons.poll(log);
        let Some(packet) = link.receive(Duration::from_millis(20)).await else {
            if link.rx_errors != rx_errors_logged {
                rx_errors_logged = link.rx_errors;
                let _ = writeln!(log.stamp(), "cypress: {} UART receive errors so far", rx_errors_logged);
            }
            // An image nobody has reached half an hour after boot is of no
            // use and cannot be replaced: give up. No handshake means the
            // booking is still the stock firmware, so this reset goes there.
            if !handshaken && Instant::now() > first_contact_deadline {
                let _ = writeln!(log.stamp(), "ota: no PC within 30 minutes of boot, returning to stock");
                let _ = log.flush(log_partition);
                SCB::sys_reset();
            }
            // Nothing is arriving: a good moment to save what was logged, so
            // that it survives if the app dies. Not during a transfer, where
            // the sender is waiting for us.
            if log.pending() && matches!(state, State::Commands) && last_save.elapsed() > Duration::from_secs(2) {
                let _ = log.flush(log_partition);
                last_save = Instant::now();
            }
            continue;
        };
        let opcode = packet.opcode;
        if opcode != EVT_SPP_RX_DATA || packet.payload.len() < 2 {
            match opcode {
                EVT_SPP_CONNECTED => {
                    let _ = writeln!(log.stamp(), "bluetooth: peer connected");
                }
                EVT_SPP_DISCONNECTED => {
                    let _ = writeln!(log.stamp(), "bluetooth: peer disconnected");
                    if let State::Receiving { total, received } = state {
                        let _ = writeln!(log.stamp(), "ota: transfer aborted at {} of {} bytes", received, total);
                        if let Some(p) = package.take() {
                            let _ = p.finish(total);
                        }
                        state = State::Commands;
                        chunk_len = 0;
                    }
                    let _ = log.flush(log_partition);
                }
                EVT_SPP_TX_COMPLETE => {} // one per answer we send; not worth a log line
                _ => cypress::log_packet(log, &packet),
            }
            continue;
        }
        let handle = [packet.payload[0], packet.payload[1]];
        let data = &packet.payload[2..];

        // What to answer, decided while `packet` (borrowed from the link) is alive.
        let mut answer: &[u8] = b"";
        let mut install = false;
        let mut to_stock = false;
        let mut export: Option<([u8; 128], usize)> = None; // "<volume>:<path>" to send to the PC

        match state {
            State::Commands => {
                let text = core::str::from_utf8(data).unwrap_or("");
                let _ = writeln!(log.stamp(), "bluetooth rx: {:?}", text);
                if text.contains("AT^BLUETOOTH") {
                    answer = b"W301RUST OK\r\n";
                    handshaken = true;
                    if !staying && image_file_is_ours {
                        // Our own first 512 bytes, as flashed.
                        let own = unsafe { core::slice::from_raw_parts(0x0801_E000 as *const u8, 512) };
                        if storage::file_starts_with(image_partition, "ota/stm32/stm32_app.bin", own)
                            && ota_flag::request_install(ota_flag::Source::Ota, 0x8B).is_ok()
                        {
                            staying = true;
                            let _ = writeln!(log.stamp(), "ota: a reset now comes back to this image");
                        } else {
                            image_file_is_ours = false;
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
                    let acceptable = path == Some(PACKAGE_PATH) && size > PACKAGE_HEADER + 4;
                    let mut rebooked = true;
                    if acceptable {
                        ready_image = None;
                        // The image file is about to be overwritten: until the
                        // new one is verified, a reset must go to the stock app.
                        image_file_is_ours = false;
                        if staying {
                            staying = false;
                            rebooked = ota_flag::request_install(ota_flag::Source::PacketA, 0x8C).is_ok();
                            if !rebooked {
                                // Leave the file alone while the bootloader would still install it.
                                let _ = writeln!(log.stamp(), "ota: could not rebook the stock firmware");
                            }
                        }
                    }
                    if acceptable && rebooked {
                        match FileWriter::rewrite(image_partition, &["OTA", "STM32"], "stm32_app.bin") {
                            Ok(writer) => {
                                package = Some(Package::new(writer));
                                state = State::Receiving { total: size, received: 0 };
                                chunk_len = 0;
                                answer = b"330";
                                let _ = writeln!(log.stamp(), "ota: receiving package, {} bytes", size);
                            }
                            Err(step) => {
                                let _ = writeln!(log.stamp(), "ota: cannot open the image file (step {})", step);
                            }
                        }
                    } else if !acceptable {
                        let _ = writeln!(log.stamp(), "ota: only {} can be pushed", PACKAGE_PATH);
                    }
                } else if let Some(what) = text.strip_prefix("061").and_then(|t| t.strip_suffix('\r')) {
                    let mut name = [0u8; 128];
                    let n = what.len().min(name.len());
                    name[..n].copy_from_slice(&what.as_bytes()[..n]);
                    export = Some((name, n));
                } else if text.contains("FILE_EXPORT_OVER") {
                    // the PC's closing word after a file export; no answer
                } else if text.contains("FI^SETUNCOMPFLAG") {
                    answer = b"OK\r\n";
                } else if text.contains("FI^OTAUPDATE") {
                    install = ready_image.is_some();
                    answer = if install { b"OK\r\n" } else { b"ERROR\r\n" };
                } else if text.contains("AT^W301STOCK") {
                    to_stock = true;
                    answer = b"OK\r\n";
                } else {
                    answer = b"ERROR\r\n";
                }
            }
            State::Receiving { total, received } => {
                // Chunks are sent one at a time; the sender waits for 310/311.
                let expect = (total - received).min(CHUNK_DATA) + 4;
                let n = data.len().min(expect - chunk_len);
                chunk[chunk_len..chunk_len + n].copy_from_slice(&data[..n]);
                chunk_len += n;
                if chunk_len == expect {
                    let (body, crc) = chunk[..expect].split_at(expect - 4);
                    chunk_len = 0;
                    if !crc32_update(0xFFFF_FFFF, body) != u32::from_le_bytes([crc[0], crc[1], crc[2], crc[3]]) {
                        answer = b"311";
                    } else {
                        if let Some(p) = package.as_mut() {
                            p.feed(body, total);
                        }
                        let received = received + body.len();
                        answer = b"310";
                        if received == total {
                            state = State::Commands;
                            match package.take().map(|p| p.finish(total)) {
                                Some(Ok(len)) => {
                                    ready_image = Some(len);
                                    let _ = writeln!(log.stamp(), "ota: package complete, image of {} bytes verified", len);
                                }
                                Some(Err(reason)) => {
                                    let _ = writeln!(log.stamp(), "ota: package rejected: {}", reason);
                                }
                                None => {}
                            }
                        } else {
                            state = State::Receiving { total, received };
                        }
                    }
                }
            }
        }

        if !answer.is_empty() {
            let mut out = [0u8; 16];
            out[..2].copy_from_slice(&handle);
            out[2..2 + answer.len()].copy_from_slice(answer);
            if !link.send(CMD_SPP_DATA, &out[..2 + answer.len()]).await {
                let _ = writeln!(log.stamp(), "bluetooth: answer could not be sent");
            }
        }

        if let Some((name, n)) = export {
            let what = core::str::from_utf8(&name[..n]).unwrap_or("");
            // Write out what is still in RAM first, so a pulled log is complete.
            let _ = log.flush(log_partition);
            let sent = export_file(link, handle, what, chunk).await;
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
                let _ = log.flush(log_partition);
                // The bootloader installs 1:ota/stm32/stm32_app.bin on the next reset.
                let booked = ota_flag::request_install(ota_flag::Source::Ota, 0x8A);
                if booked.is_err() {
                    // The earlier ticket (back to the stock app) is still in place.
                    let _ = writeln!(log.stamp(), "ota: could not book the install");
                    let _ = log.flush(log_partition);
                }
            } else {
                let _ = writeln!(log.stamp(), "bluetooth: return to the stock firmware requested");
                let _ = log.flush(log_partition);
                if ota_flag::request_install(ota_flag::Source::PacketA, 0x8C).is_err() {
                    let _ = writeln!(log.stamp(), "bluetooth: could not book the stock firmware; staying");
                    let _ = log.flush(log_partition);
                    continue;
                }
            }
            SCB::sys_reset();
        }
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
        while left > 0 && result.is_ok() {
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
