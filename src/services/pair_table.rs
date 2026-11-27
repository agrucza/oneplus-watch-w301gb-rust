//! The table of paired devices as the stock firmware keeps it in
//! `2:bluethooth_pair.cfg`: nine slots of 175 bytes, written whole
//! (read in B.70: loaded at 0x0802AEBC, saved at 0x0802BB3C).
//!
//! A slot: 0 the record's id (chosen by the chip; 0 = free), 2 the
//! device's address, first byte first, then its keys; id and the 136
//! bytes after it are what the chip hands over and takes back. 0xAD
//! the stock app's type: 0 a phone by serial port (slot 0), 1 a phone
//! by LE (slot 1), 2 a headset (slots 2 to 6), 3 a device paired in a
//! special mode (slots 7 and 8).
//!
//! What is paired with this firmware goes into the first free slot of
//! 2 to 6 with type 2, which is where the stock firmware put the PC.
//! Nothing is ever pushed out: with no slot free, nothing is stored.

pub const SLOTS: usize = 9;
pub const SLOT_LEN: usize = 175;
pub const FILE_LEN: usize = SLOTS * SLOT_LEN;
/// The id and what the chip hands over and takes back with it.
pub const RECORD_LEN: usize = 2 + 136;
const TYPE_AT: usize = 0xAD;
/// The slots new pairings go to, and the type they get.
const OURS: core::ops::RangeInclusive<usize> = 2..=6;
const OUR_TYPE: u8 = 2;

pub type Table = [u8; FILE_LEN];

/// A paired device, for showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    pub slot: usize,
    pub id: u16,
    /// First byte first, as an address is written.
    pub address: [u8; 6],
    pub kind: u8,
}

fn slot(table: &Table, n: usize) -> &[u8] {
    &table[n * SLOT_LEN..(n + 1) * SLOT_LEN]
}

/// The device in slot `n`, if the slot is in use.
pub fn entry(table: &Table, n: usize) -> Option<Entry> {
    if n >= SLOTS {
        return None;
    }
    let s = slot(table, n);
    let id = u16::from_le_bytes([s[0], s[1]]);
    let mut address = [0u8; 6];
    address.copy_from_slice(&s[2..8]);
    (id != 0).then_some(Entry { slot: n, id, address, kind: s[TYPE_AT] })
}

/// The devices in use, in the order of their slots.
pub fn entries(table: &Table) -> impl Iterator<Item = Entry> + '_ {
    (0..SLOTS).filter_map(|n| entry(table, n))
}

/// What the chip is given for slot `n`: the id and the 136 bytes.
pub fn record(table: &Table, n: usize) -> &[u8] {
    &slot(table, n)[..RECORD_LEN]
}

/// The slot a new pairing would go to.
pub fn free_slot(table: &Table) -> Option<usize> {
    OURS.into_iter().find(|&n| entry(table, n).is_none())
}

/// Whether a device with this address is in the table; `address` in
/// either byte order (the chip's question brings it last byte first).
pub fn holds(table: &Table, address: [u8; 6]) -> bool {
    let mut reversed = address;
    reversed.reverse();
    entries(table).any(|e| e.address == address || e.address == reversed)
}

/// Why a record was not stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
    /// Not the length the chip hands over, or an id of 0.
    NotARecord,
    /// Its device is in the table already.
    Held,
    /// No slot of ours is free.
    Full,
}

/// Put a record the chip handed over into the first free slot of ours.
/// The slot is cleared first, so nothing of a device removed earlier
/// stays behind it. Returns the slot.
pub fn store(table: &mut Table, record: &[u8]) -> Result<usize, Refused> {
    if record.len() != RECORD_LEN || (record[0] == 0 && record[1] == 0) {
        return Err(Refused::NotARecord);
    }
    let mut address = [0u8; 6];
    address.copy_from_slice(&record[2..8]);
    if entries(table).any(|e| e.address == address) {
        return Err(Refused::Held);
    }
    let n = free_slot(table).ok_or(Refused::Full)?;
    let s = &mut table[n * SLOT_LEN..(n + 1) * SLOT_LEN];
    s.fill(0);
    s[..RECORD_LEN].copy_from_slice(record);
    s[TYPE_AT] = OUR_TYPE;
    Ok(n)
}

/// Take the device in slot `n` out and clear the slot. Returns its id
/// and whether another slot still carries the same id (then the chip
/// must keep that record: the stock firmware filed one device twice).
pub fn remove(table: &mut Table, n: usize) -> Option<(u16, bool)> {
    let removed = entry(table, n)?;
    table[n * SLOT_LEN..(n + 1) * SLOT_LEN].fill(0);
    Some((removed.id, entries(table).any(|e| e.id == removed.id)))
}
