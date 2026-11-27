//! Bluetooth as the watch: give the Cypress chip the watch's address (from
//! the factory data) and the pairing keys the stock firmware stored, then
//! make it connectable.

use core::fmt::Write;

use embassy_time::Duration;

use crate::drivers::cypress;
use crate::services::log::Log;
use crate::services::{factory, pair_table};

/// Stored pairings: `2:bluethooth_pair.cfg`, a raw copy of the stock app's
/// table of nine 175-byte slots (`pair_table`, read by `pairing::load`).
/// A slot is in use when its first two bytes (the record id) are not
/// zero; the stock app pushes the id and the following 136 bytes to the
/// chip (0x0802BB8A).
const PAIR_SLOT_LEN: usize = pair_table::SLOT_LEN;

/// Start the chip, give it the watch's address and the stored pairing keys,
/// and make it connectable. Returns the link only if all of that worked.
///
/// The order matters: answering on the watch's address without the keys
/// would make the PC's connection fail authentication, which could cost the
/// PC its pairing with the stock firmware. So every step is checked and the
/// chip only becomes connectable if all of them worked.
pub async fn start(r: cypress::Resources, log: &mut Log) -> Option<cypress::Link> {
    let Some(mut link) = cypress::Link::power_up(r).await else {
        let _ = writeln!(log.stamp(), "cypress: could not open the link");
        return None;
    };
    let _ = writeln!(log.stamp(), "cypress: powered up");
    set_up(&mut link, log).await.then_some(link)
}

/// What `start` does with a chip that has just been powered up; also run
/// after the chip was switched off and on again while the firmware runs
/// (`cypress::Link::power_cycle`). True if the chip is connectable.
pub async fn set_up(link: &mut cypress::Link, log: &mut Log) -> bool {
    // The chip starts on a default address; without the watch's own there
    // is no point in starting it.
    let Some(address) = factory::bluetooth_address(log) else {
        return false;
    };

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
        return false;
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

        // 3. Pairing keys from the stock app's file on volume 2, or
        // from the second copy if that file is not there in its length.
        let Some(table) = crate::services::pairing::load(log) else {
            break 'setup false;
        };
        let mut pushed = 0;
        for entry in pair_table::entries(table) {
            let slot = &table[entry.slot * PAIR_SLOT_LEN..(entry.slot + 1) * PAIR_SLOT_LEN];
            // Log who the record is for, never the key itself: of the
            // keys only whether there are any, and the stock app's type
            // (0xAD: 0 phone by serial port, 1 phone by LE, 2 headset,
            // 3 paired in its special mode).
            let _ = writeln!(
                log.stamp(),
                "pairing slot {}: id {:#06x}, peer {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, type {}, classic key {}, LE keys {}",
                entry.slot,
                entry.id,
                slot[2], slot[3], slot[4], slot[5], slot[6], slot[7],
                entry.kind,
                if slot[0x09..0x19].iter().any(|&b| b != 0) { "yes" } else { "no" },
                if slot[0x22..0x8A].iter().any(|&b| b != 0) { "yes" } else { "no" },
            );
            if !link.command(log, cypress::CMD_PUSH_NVRAM_DATA, pair_table::record(table, entry.slot)).await {
                break 'setup false;
            }
            pushed += 1;
        }
        let _ = writeln!(log.stamp(), "cypress: {} pairing records accepted", pushed);
        if pushed == 0 {
            break 'setup false;
        }

        // 4. The watch's name, which paired devices read at every
        // connection; without it the chip answers with its own.
        // A chip that does not take it is still made connectable: the
        // name is not worth losing the connection to the PC over.
        if !link.command(log, cypress::CMD_SET_LOCAL_NAME, &crate::services::pairing::name(address)).await {
            let _ = writeln!(log.stamp(), "cypress: the name was NOT taken");
        }

        // 5. Connectable for already paired devices, not discoverable.
        link.command(log, cypress::CMD_SET_VISIBILITY, &[0, 1]).await
    };
    let _ = writeln!(
        log.stamp(),
        "cypress: {}",
        if connectable { "connectable as the watch" } else { "NOT made connectable" }
    );

    // Not to be found again, whatever it was before.
    crate::services::pairing::chip_set_up();
    connectable
}
