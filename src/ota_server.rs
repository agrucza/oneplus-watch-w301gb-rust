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
//! One extra command: "00AT^W301STOCK" -> "OK\r\n", then reset. With the
//! return ticket booked at boot, that brings the stock firmware back.

use core::fmt::Write;

use cortex_m::peripheral::SCB;
use embassy_time::{Duration, Timer};

use crate::cypress::{self, Link};
use crate::log::Log;
use crate::ota_flag;
use crate::package::{crc32_update, Package, PACKAGE_HEADER};
use crate::storage::{FileWriter, Partition};

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

const CHUNK_DATA: usize = 47996;
const PACKAGE_PATH: &str = "1:B280STM32_ota.bin";
enum State {
    Commands,
    Receiving { total: usize, received: usize },
}

/// Serve the update protocol until an install or a return request resets
/// the watch. `image_partition` is volume 1, `log_partition` volume 3.
pub async fn run(link: &mut Link, log: &mut Log, image_partition: Partition, log_partition: Partition) -> ! {
    let Some(chunk) = cortex_m::singleton!(: [u8; CHUNK_DATA + 4] = [0; CHUNK_DATA + 4]) else {
        SCB::sys_reset(); // cannot happen: `run` is entered once
    };
    let mut chunk_len = 0usize;
    let mut state = State::Commands;
    let mut package: Option<Package> = None;
    let mut ready_image: Option<usize> = None; // length of a complete, verified image

    loop {
        let Some(packet) = link.receive(Duration::from_millis(500)).await else {
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

        match state {
            State::Commands => {
                let text = core::str::from_utf8(data).unwrap_or("");
                let _ = writeln!(log.stamp(), "bluetooth rx: {:?}", text);
                if text.contains("AT^BLUETOOTH") {
                    answer = b"W301RUST OK\r\n";
                } else if text == "300\r" {
                    answer = b"300";
                } else if let Some(args) = text.strip_prefix("33").and_then(|t| t.strip_suffix('\r')) {
                    // "<ext>,<path>,<size>"
                    let mut parts = args.split(',');
                    let (_ext, path, size) = (parts.next(), parts.next(), parts.next());
                    let size = size.and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                    answer = b"ERROR\r\n";
                    if path == Some(PACKAGE_PATH) && size > PACKAGE_HEADER + 4 {
                        ready_image = None;
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
                    } else {
                        let _ = writeln!(log.stamp(), "ota: only {} can be pushed", PACKAGE_PATH);
                    }
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
            }
            SCB::sys_reset();
        }
    }
}
