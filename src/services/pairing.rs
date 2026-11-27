//! Pairing a new device over Bluetooth.
//!
//! The stock firmware lets everything pair that finds the watch: it
//! answers the chip's question "is this number right?" with a yes by
//! itself (0x0802BBFC in B.70). Here the watch can be found only while
//! the pairing page is shown (`want`), and the question goes to the
//! wearer on the confirmation screen, with the number.
//!
//! A pairing the wearer accepted is kept: the chip's record goes into
//! the first free slot of ours in the table (`pair_table`) and the
//! table is written, first to a second copy in the log folder, then to
//! the stock firmware's file; each is read back. Nothing already in the
//! table is ever replaced. A device is removed only by the wearer, on
//! the page that lists them.
//!
//! The commands are the stock app's: pairing allowed (0x0009 with 1,
//! which stock sends at every start and never takes back), the name
//! (0x0012), and to be found or not (0x0008). They are sent without
//! waiting for the chip's status, as the stock app does.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_time::{Duration, Instant};

use crate::drivers::cypress::{self, Link};
use crate::services::pair_table::{self, Refused, Table, FILE_LEN, RECORD_LEN, SLOTS};
use crate::services::storage::{self, FileReader, FileWriter};
use crate::services::factory;
use crate::services::log::Log;
use crate::services::progress::{self, Transfer};

/// How long the wearer has for the answer. The other side gives up
/// after 30 s (Bluetooth's own limit for this question); a no sent
/// before that ends it cleanly.
const ANSWER_WITHIN: Duration = Duration::from_secs(25);

/// How far a pairing is, for the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Starting,
    Ready,
    Accepted,
    Denied,
    Paired,
    Failed,
    /// No slot is free: a device has to be removed first.
    Full,
    /// Paired, but the table could not be written: gone after a restart.
    NotKept,
}

/// The stock firmware's file on volume 2, and the second copy in the
/// log folder of the log volume, used at a start if the first is not
/// readable or not of its length.
const VOLUME: usize = 2;
const FILE: &str = "bluethooth_pair.cfg";
const COPY_VOLUME: usize = 3;
const COPY: &str = "PAIRS.BAK";
const COPY_PATH: &str = "LOG/PAIRS.BAK";

/// The table, and a second buffer to read a written file back into.
/// Only the main task touches them (`bluetooth::set_up`, `Pairing`).
fn table() -> &'static mut Table {
    static mut TABLE: Table = [0; FILE_LEN];
    // Safety: only the main task runs this, one use at a time.
    unsafe { &mut *(&raw mut TABLE) }
}

fn check() -> &'static mut Table {
    static mut CHECK: Table = [0; FILE_LEN];
    // Safety: as `table`.
    unsafe { &mut *(&raw mut CHECK) }
}

/// What the page of devices shows of each slot: in use, type, address.
static SHOWN: [[AtomicU8; 8]; SLOTS] = [const { [const { AtomicU8::new(0) }; 8] }; SLOTS];
/// The device that last opened the serial port: the PC that flashes.
static SERIAL: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
/// A slot the wearer asked to have removed, plus 1; 0 for none.
static REMOVE: AtomicU8 = AtomicU8::new(0);
/// The table in memory is not what the file holds: a write failed.
static UNSAVED: AtomicBool = AtomicBool::new(false);

fn publish() {
    let table = table();
    for (n, shown) in SHOWN.iter().enumerate() {
        let entry = pair_table::entry(table, n);
        shown[0].store(entry.is_some() as u8, Ordering::Relaxed);
        shown[1].store(entry.map_or(0, |e| e.kind), Ordering::Relaxed);
        for i in 0..6 {
            shown[2 + i].store(entry.map_or(0, |e| e.address[i]), Ordering::Relaxed);
        }
    }
}

/// The device in slot `slot` for the page: its type and its address.
pub fn shown(slot: usize) -> Option<(u8, [u8; 6])> {
    let shown = SHOWN.get(slot)?;
    (shown[0].load(Ordering::Relaxed) != 0).then(|| (shown[1].load(Ordering::Relaxed), [2, 3, 4, 5, 6, 7].map(|i| shown[i].load(Ordering::Relaxed))))
}

/// A device opened the serial port; `peer` as the chip gives it.
pub fn serial_peer(peer: &[u8]) {
    for (to, from) in SERIAL.iter().zip(peer.iter()) {
        to.store(*from, Ordering::Relaxed);
    }
}

/// Whether `address` is the device that last opened the serial port.
pub fn serial(address: [u8; 6]) -> bool {
    // Either byte order: which one that event uses has not been seen.
    let serial = [0, 1, 2, 3, 4, 5].map(|i| SERIAL[i].load(Ordering::Relaxed));
    let mut reversed = serial;
    reversed.reverse();
    address != [0; 6] && (serial == address || reversed == address)
}

/// Ask for the device in `slot` to be removed (the display task).
pub fn request_remove(slot: usize) {
    REMOVE.store(slot as u8 + 1, Ordering::Relaxed);
    crate::hal::wake::nudge();
}

pub fn unsaved() -> bool {
    UNSAVED.load(Ordering::Relaxed)
}

/// Read a file of the table's length into the checking buffer.
fn read_whole(partition: storage::Partition, path: &str) -> bool {
    let Ok(mut file) = FileReader::open(partition, path) else {
        return false;
    };
    let whole = file.len() as usize == FILE_LEN && file.read(check()) == Ok(FILE_LEN);
    file.close();
    whole
}

/// Read the table at a start of the chip: the stock firmware's file, or
/// the second copy if that one is not there in its length. The table
/// is returned for handing its records to the chip.
pub fn load(log: &mut Log) -> Option<&'static Table> {
    let table = table();
    let read = storage::inspect(VOLUME).map_err(|_| 1u8).and_then(|p| storage::read_root_file(p, FILE, table));
    match read {
        Ok(FILE_LEN) => {}
        other => {
            match other {
                Ok(len) => {
                    let _ = writeln!(log.stamp(), "pairing file has {} bytes, expected {}", len, FILE_LEN);
                }
                Err(step) => {
                    let _ = writeln!(log.stamp(), "pairing file could not be read (step {})", step);
                }
            }
            let copied = storage::inspect(COPY_VOLUME).is_ok_and(|p| read_whole(p, COPY_PATH));
            let _ = writeln!(log.stamp(), "pairing: the second copy is {}", if copied { "used instead" } else { "not there either" });
            if !copied {
                table.fill(0);
                publish();
                return None;
            }
            table.copy_from_slice(check());
            // The file itself is written again with the next change.
            UNSAVED.store(true, Ordering::Relaxed);
        }
    }
    publish();
    Some(table)
}

/// Write the table: the second copy first, then the stock firmware's
/// file, each read back and compared. The file is cut to nothing before
/// it is written, so a write that is interrupted leaves it short; the
/// stock firmware and `load` then take it as not there, which is what
/// the second copy is for.
fn save(log: &mut Log) -> bool {
    let table = table();
    let write = |partition: storage::Partition, dirs: &[&str], name: &str, path: &str| -> Result<(), u8> {
        let mut writer = FileWriter::rewrite(partition, dirs, name).map_err(|_| 2u8)?;
        writer.write(&table[..]).map_err(|()| 3u8)?;
        writer.finish().map_err(|()| 4u8)?;
        (read_whole(partition, path) && check()[..] == table[..]).then_some(()).ok_or(5u8)
    };
    let copy = storage::inspect(COPY_VOLUME).map_err(|_| 1u8).and_then(|p| {
        // The file has to exist before it can be rewritten.
        storage::append(p, "LOG", COPY, &[]).map_err(|_| 1u8)?;
        write(p, &["LOG"], COPY, COPY_PATH)
    });
    if let Err(step) = copy {
        let _ = writeln!(log.stamp(), "pairing: the second copy could NOT be written (step {}); the pairing file is left as it is", step);
        UNSAVED.store(true, Ordering::Relaxed);
        return false;
    }
    let file = storage::inspect(VOLUME).map_err(|_| 1u8).and_then(|p| write(p, &[], FILE, FILE));
    let _ = match file {
        Ok(()) => writeln!(log.stamp(), "pairing: the table is written and read back, file and second copy"),
        Err(step) => writeln!(log.stamp(), "pairing: the pairing file could NOT be written (step {}); the second copy holds the table", step),
    };
    UNSAVED.store(file.is_err(), Ordering::Relaxed);
    let _ = log.flush();
    file.is_ok()
}

static WANTED: AtomicBool = AtomicBool::new(false);
static STAGE: AtomicU8 = AtomicU8::new(0);
/// The chip was set up afresh: it is not to be found and knows no name.
static CHIP_FRESH: AtomicBool = AtomicBool::new(true);
/// The last four characters of the watch's name, once it was built.
static SUFFIX: [AtomicU8; 4] = [AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0)];

/// Say whether the watch is to be found by new devices (the display
/// task, from whether the pairing page is shown).
pub fn want(on: bool) {
    if WANTED.swap(on, Ordering::Relaxed) != on {
        crate::hal::wake::nudge();
    }
}

pub fn stage() -> Stage {
    match STAGE.load(Ordering::Relaxed) {
        1 => Stage::Ready,
        2 => Stage::Accepted,
        3 => Stage::Denied,
        4 => Stage::Paired,
        5 => Stage::Failed,
        6 => Stage::Full,
        7 => Stage::NotKept,
        _ => Stage::Starting,
    }
}

/// The pairing page was left: the next opening starts afresh.
pub fn left() {
    set_stage(Stage::Starting);
}

fn set_stage(stage: Stage) {
    STAGE.store(stage as u8, Ordering::Relaxed);
}

/// The last four characters of the name; zeros while not known.
pub fn suffix() -> [u8; 4] {
    [0, 1, 2, 3].map(|i| SUFFIX[i].load(Ordering::Relaxed))
}

/// The chip was set up from its start (`bluetooth::set_up`).
pub fn chip_set_up() {
    CHIP_FRESH.store(true, Ordering::Relaxed);
}

/// The name as the stock firmware's is seen from outside: "OnePlus
/// Watch " and the last four digits of the address in capitals (seen on
/// a PC for a watch of this kind; the code that builds it was not
/// found in the stock app).
pub fn name(address: [u8; 6]) -> [u8; 18] {
    let mut name = *b"OnePlus Watch 0000";
    for (i, byte) in address[4..].iter().enumerate() {
        name[14 + 2 * i] = b"0123456789ABCDEF"[(byte >> 4) as usize];
        name[15 + 2 * i] = b"0123456789ABCDEF"[(byte & 15) as usize];
    }
    name
}

/// What is owed to the chip and how far the question to the wearer is.
pub struct Pairing {
    /// What the chip was last told: to be found or not.
    findable: bool,
    /// Pairing was allowed and the name given since the chip's start.
    prepared: bool,
    /// The device that asked and until when the wearer may answer.
    asked: Option<([u8; 6], Instant)>,
    /// An answer still to be sent: the device and yes or no.
    answer: Option<([u8; 6], bool)>,
    /// The device the wearer said yes to, as the question named it:
    /// only its record is kept.
    accepted: Option<[u8; 6]>,
    /// Its record, handed over by the chip and not yet stored.
    staged: Option<[u8; RECORD_LEN]>,
}

impl Pairing {
    pub const fn new() -> Self {
        Self { findable: false, prepared: false, asked: None, answer: None, accepted: None, staged: None }
    }

    /// Take a packet from the chip if it belongs to pairing. Nothing is
    /// sent from here; `poll` does that.
    pub fn event(&mut self, opcode: u16, payload: &[u8], log: &mut Log) -> bool {
        match opcode {
            cypress::EVT_USER_CONFIRMATION if payload.len() >= 10 => {
                let mut peer = [0u8; 6];
                peer.copy_from_slice(&payload[..6]);
                let number = u32::from_le_bytes([payload[6], payload[7], payload[8], payload[9]]);
                let line = log.stamp();
                let _ = write!(line, "pairing: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} asks: ", peer[0], peer[1], peer[2], peer[3], peer[4], peer[5]);
                if pair_table::holds(table(), peer) {
                    // A new pairing would replace the key the device
                    // holds, and nothing in the table is replaced: the
                    // one in the file would no longer fit, which would
                    // lock the device out, the PC that flashes included.
                    // It has to be removed here first.
                    let _ = writeln!(line, "denied, the pairing file already holds this device");
                    self.answer = Some((peer, false));
                    set_stage(Stage::Denied);
                } else if !self.findable {
                    let _ = writeln!(line, "denied, the pairing page is not shown");
                    self.answer = Some((peer, false));
                } else if progress::now().is_some() || self.asked.is_some() {
                    let _ = writeln!(line, "denied, something else is being asked or transferred");
                    self.answer = Some((peer, false));
                } else {
                    let _ = writeln!(line, "the wearer is asked");
                    progress::begin_pairing(number);
                    self.asked = Some((peer, Instant::now() + ANSWER_WITHIN));
                }
                true
            }
            cypress::EVT_NVRAM_DATA => {
                // Who it is for and what it holds, never the keys. The
                // layout is the pairing file's slot (id, address at 2,
                // classic key at 9, LE keys from 0x22).
                let any = |from: usize, to: usize| if payload.get(from..to.min(payload.len())).is_some_and(|keys| keys.iter().any(|&b| b != 0)) { "yes" } else { "no" };
                // Kept only if it is the record of the device the wearer
                // just said yes to. Measured: the question and the
                // record carry the address in the same byte order, first
                // byte first (the answer to the question the other way).
                let ours = payload.len() == RECORD_LEN && self.accepted.is_some_and(|peer| peer.iter().rev().eq(payload[2..8].iter()) || peer[..] == payload[2..8]);
                if payload.len() >= 8 {
                    let _ = writeln!(
                        log.stamp(),
                        "pairing: the chip hands over a record: id {:#06x}, {} bytes, for {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, classic key {}, LE keys {}; {}",
                        u16::from_le_bytes([payload[0], payload[1]]),
                        payload.len(),
                        payload[2], payload[3], payload[4], payload[5], payload[6], payload[7],
                        any(9, 0x19),
                        any(0x22, 0x8A),
                        if ours { "to be kept" } else { "not kept: no pairing accepted here for it" },
                    );
                } else {
                    let _ = writeln!(log.stamp(), "pairing: the chip hands over a record of {} bytes", payload.len());
                }
                if ours {
                    let mut record = [0u8; RECORD_LEN];
                    record.copy_from_slice(payload);
                    self.staged = Some(record);
                    self.accepted = None;
                }
                true
            }
            cypress::EVT_PAIRING_COMPLETE => {
                let _ = writeln!(log.stamp(), "pairing: the chip says a pairing ended, {} bytes, first {:?}", payload.len(), payload.first());
                // The stock app's one rule: 5 is a failure.
                if payload.first() == Some(&5) && stage() == Stage::Accepted {
                    set_stage(Stage::Failed);
                }
                true
            }
            _ => false,
        }
    }

    /// Send what is owed to the chip. Called by the main loop every pass.
    pub async fn poll(&mut self, link: &mut Link, log: &mut Log) {
        if CHIP_FRESH.swap(false, Ordering::Relaxed) {
            self.findable = false;
            self.prepared = false;
        }
        if !link.is_open() {
            return;
        }
        // The wearer's answer, or a no when the time is up or the
        // question was taken off the display by something else.
        if let Some((peer, until)) = self.asked {
            let still_asked = progress::now().is_some_and(|(what, _)| what == Transfer::OfferedPairing);
            let decision = if !still_asked { Some(false) } else { progress::decision().or((Instant::now() > until).then_some(false)) };
            if let Some(accept) = decision {
                let _ = writeln!(log.stamp(), "pairing: {}", if accept { "accepted on the watch" } else if still_asked && progress::decision().is_some() { "denied on the watch" } else { "not answered in time: denied" });
                if still_asked {
                    progress::end();
                }
                self.asked = None;
                self.answer = Some((peer, accept));
                self.accepted = accept.then_some(peer);
                set_stage(if accept { Stage::Accepted } else { Stage::Denied });
            }
        }
        if let Some((peer, accept)) = self.answer.take() {
            let mut payload = [0u8; 7];
            // The address goes back in the other byte order than the
            // question brought it: the stock app's builder turns the six
            // bytes round (0x080689F0). Sent as they came, the chip
            // takes the answer (status 0) and the pairing still runs
            // into the other side's time limit (measured 2026-11-27).
            payload[..6].copy_from_slice(&peer);
            payload[..6].reverse();
            payload[6] = accept as u8;
            if !link.send(cypress::CMD_USER_CONFIRMATION, &payload).await {
                let _ = writeln!(log.stamp(), "pairing: the answer could not be sent");
            }
            let _ = log.flush();
        }
        // The record of an accepted pairing: into the table and the file.
        if let Some(record) = self.staged.take() {
            match pair_table::store(table(), &record) {
                Ok(slot) => {
                    let kept = save(log);
                    publish();
                    let _ = writeln!(log.stamp(), "pairing: the device is in slot {}{}", slot, if kept { "" } else { ", in memory only" });
                    set_stage(if kept { Stage::Paired } else { Stage::NotKept });
                }
                Err(why) => {
                    let _ = writeln!(log.stamp(), "pairing: the record is not stored: {}", match why {
                        Refused::NotARecord => "not a record",
                        Refused::Held => "the device is in the table already",
                        Refused::Full => "no slot is free",
                    });
                    set_stage(if why == Refused::Full { Stage::Full } else { Stage::NotKept });
                }
            }
            let _ = log.flush();
        }
        // A device the wearer removes: forgotten by the chip, unless
        // another slot carries the same record, out of the table, and
        // the table written. The last device stays: without any, the
        // chip is not made connectable at a start.
        let remove = REMOVE.swap(0, Ordering::Relaxed) as usize;
        if remove != 0 && pair_table::entries(table()).count() > 1 {
            if let Some((id, still_used)) = pair_table::remove(table(), remove - 1) {
                let told = still_used || link.send(cypress::CMD_DELETE_NVRAM_DATA, &id.to_le_bytes()).await;
                let kept = save(log);
                publish();
                let _ = writeln!(log.stamp(), "pairing: the device in slot {} (record {:#06x}) is removed; the chip {}; the table {}", remove - 1, id, if still_used { "keeps the record, another slot carries it" } else if told { "was told" } else { "could NOT be told" }, if kept { "is written" } else { "is NOT written" });
                let _ = log.flush();
            }
        }
        let mut wanted = WANTED.load(Ordering::Relaxed);
        // With no slot free the watch is not to be found at all.
        if wanted && !self.findable && pair_table::free_slot(table()).is_none() {
            if stage() != Stage::Full {
                let _ = writeln!(log.stamp(), "pairing: no slot is free, the watch is not made to be found");
                set_stage(Stage::Full);
            }
            wanted = false;
        }
        if wanted != self.findable {
            let mut sent = true;
            if wanted && !self.prepared {
                match factory::bluetooth_address(log) {
                    Some(address) => {
                        let name = name(address);
                        for (i, byte) in name[14..].iter().enumerate() {
                            SUFFIX[i].store(*byte, Ordering::Relaxed);
                        }
                        sent = link.send(cypress::CMD_SET_PAIRING_MODE, &[1]).await && link.send(cypress::CMD_SET_LOCAL_NAME, &name).await;
                        self.prepared = sent;
                        let _ = writeln!(log.stamp(), "pairing: pairing allowed and the name given: {}", if sent { "sent" } else { "NOT sent" });
                    }
                    None => sent = false,
                }
            }
            // To be found and to be connected to, or only the second,
            // which is what the chip is set to at its start.
            if sent && link.send(cypress::CMD_SET_VISIBILITY, &[wanted as u8, 1]).await {
                self.findable = wanted;
                let _ = writeln!(log.stamp(), "pairing: the watch can {} be found by new devices", if wanted { "now" } else { "no longer" });
                // How the last pairing went stays on the page through
                // a dark display; leaving the page clears it (`left`).
                if wanted && stage() == Stage::Starting {
                    set_stage(Stage::Ready);
                }
            } else if wanted {
                // Not tried again every pass: the page has to be opened anew.
                let _ = writeln!(log.stamp(), "pairing: the chip could not be told; open the page again");
                WANTED.store(false, Ordering::Relaxed);
            }
            let _ = log.flush();
        }
    }
}
