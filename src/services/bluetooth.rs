//! Bluetooth as the watch: give the Cypress chip the watch's address (from
//! the factory data) and the pairing keys the stock firmware stored, then
//! make it connectable.

use core::fmt::Write;

use embassy_time::Duration;

use crate::drivers::cypress;
use crate::services::log::Log;
use crate::services::{factory, storage};

/// Stored pairings: `2:bluethooth_pair.cfg`, a raw copy of the stock app's
/// table of nine 175-byte slots (loaded at 0x0802AEEE). A slot is in use when
/// its first two bytes (the record id) are not zero; the stock app pushes
/// the id and the following 136 bytes to the chip (0x0802BB8A).
const PAIRING_VOLUME: usize = 2;
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
pub async fn start(r: cypress::Resources, log: &mut Log) -> Option<cypress::Link> {
    // The chip starts on a default address; without the watch's own there
    // is no point in starting it.
    let address = factory::bluetooth_address(log)?;
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
        let mut reversed = address;
        reversed.reverse();
        if !link.command(log, cypress::CMD_SET_LOCAL_BDA, &reversed).await {
            break 'setup false;
        }
        match link.request(log, cypress::CMD_READ_LOCAL_BDA, &[], cypress::EVT_LOCAL_BDA).await {
            Some((read_back, 6)) if read_back[..6] == address => {
                let _ = writeln!(log.stamp(), "cypress: address set and read back correctly");
            }
            Some((_, n)) => {
                // Not shown: the address is personal to this watch.
                let _ = writeln!(log.stamp(), "cypress: address reads back differently ({} bytes)", n);
                break 'setup false;
            }
            None => break 'setup false,
        }

        // 3. Pairing keys from the stock app's file on volume 2.
        let Some(table) = cortex_m::singleton!(: [u8; PAIR_FILE_LEN] = [0; PAIR_FILE_LEN]) else {
            break 'setup false;
        };
        let read = storage::inspect(PAIRING_VOLUME).map_err(|_| 1u8).and_then(|p| storage::read_root_file(p, PAIR_FILE, table));
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
