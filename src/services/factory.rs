//! Factory data: what the manufacturer wrote into this one watch.
//!
//! The stock app keeps it in `0:flash_device.txt`, 1024 bytes, loaded at
//! `0x08032A10`. The file is valid if its first byte is 0xA5 and its last
//! byte is the checksum (`boot::ticket::crc8`) of its first 255 bytes.
//! Inside are records, each a 0xA5 mark, the data as text padded with
//! zeros, and a checksum over the record before it.
//!
//! Only the Bluetooth address is read here: the record at 0x28, twelve hex
//! digits in the order the address is written (stock accessors at
//! `0x08031E9C` to write and `0x08031ED0` to read; the Bluetooth module
//! calls the latter). The other records hold serial numbers and keys.

use core::fmt::Write;

use crate::boot::ticket::crc8;
use crate::services::log::Log;
use crate::services::storage;

const VOLUME: usize = 0;
const FILE: &str = "flash_device.txt";
const FILE_LEN: usize = 1024;
const MARK: u8 = 0xA5;
/// The part of the file its checksum covers.
const CHECKED_LEN: usize = 255;

/// The address record: mark, 12 hex digits, 2 zeros, checksum.
const ADDRESS_RECORD: usize = 0x28;
const ADDRESS_RECORD_LEN: usize = 16;

/// The watch's Bluetooth address, first byte first. `None` (with the reason
/// in the log) if the file or the record is missing or damaged. The address
/// itself is never logged.
pub fn bluetooth_address(log: &mut Log) -> Option<[u8; 6]> {
    let file = cortex_m::singleton!(: [u8; FILE_LEN] = [0; FILE_LEN])?;
    let read = storage::inspect(VOLUME).map_err(|_| 1u8).and_then(|p| storage::read_root_file(p, FILE, file));
    match read {
        Ok(FILE_LEN) => {}
        Ok(len) => {
            let _ = writeln!(log.stamp(), "factory data: file has {} bytes, expected {}", len, FILE_LEN);
            return None;
        }
        Err(step) => {
            let _ = writeln!(log.stamp(), "factory data: file could not be read (step {})", step);
            return None;
        }
    }
    if file[0] != MARK || crc8(&file[..CHECKED_LEN]) != file[FILE_LEN - 1] {
        let _ = writeln!(log.stamp(), "factory data: file is damaged (mark or checksum)");
        return None;
    }
    let record = &file[ADDRESS_RECORD..ADDRESS_RECORD + ADDRESS_RECORD_LEN];
    let (body, checksum) = record.split_at(ADDRESS_RECORD_LEN - 1);
    if body[0] != MARK || crc8(body) != checksum[0] {
        let _ = writeln!(log.stamp(), "factory data: Bluetooth address record is damaged");
        return None;
    }
    let mut address = [0u8; 6];
    for (byte, digits) in address.iter_mut().zip(body[1..13].chunks_exact(2)) {
        let text = core::str::from_utf8(digits).ok()?;
        let Ok(value) = u8::from_str_radix(text, 16) else {
            let _ = writeln!(log.stamp(), "factory data: Bluetooth address is not hex");
            return None;
        };
        *byte = value;
    }
    let _ = writeln!(log.stamp(), "factory data: Bluetooth address read");
    Some(address)
}
