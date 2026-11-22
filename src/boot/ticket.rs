//! The bootloader's "OTA flag" block - our way back to the stock firmware.
//!
//! Reverse-engineered from stm32_boot.bin (B280_BootLoader_V1.01):
//!
//! * One 4 KB flash page at 0x0801_9000 is used as a small wear-levelled log
//!   of 0x200-byte blocks (8 slots). `flash_write_balance` zeroes the first
//!   double word of every used slot it passes, then writes the new block into
//!   the first fully erased slot; when none is left it erases the page and
//!   starts again at slot 0. `flash_read_balance` scans from the last slot
//!   down and takes the first block with magic + CRC ok.
//! * Block:  [0x000] = 0xA5, [0x1FF] = crc8 over bytes 0x000..0x0FF
//!   (the stock code truncates the length to 8 bits, so only 255 bytes).
//! * +0x20  SystemStart  record (32 bytes): boot/watchdog counter.
//! * +0x40  SystemUpdate record (16 bytes), all the bootloader acts on:
//!     +0x40 valid = 0xA5      +0x41 flag (1 = flash an image on next boot)
//!     +0x42 part  (source)    +0x43 err        +0x44 roll
//!     +0x45 flashed ("from_old" in the stock app)   +0x46 OTA step index (u16)
//!     +0x48 alt_src           +0x4D progress   +0x4E request bits
//!     +0x4F crc8 over +0x40..+0x4F
//!
//! With flag == 1 the bootloader copies an app image from eMMC into internal
//! flash and clears the flag. Setting it is all that is needed to get the
//! previous firmware back - no Bluetooth, eMMC or filesystem code required.

use crate::boot::flash;

const PAGE_ADDR: u32 = 0x0801_9000;
const PAGE_NUMBER: u8 = 0x19;
const BLOCK: usize = 0x200;
const SLOTS: usize = 0x1000 / BLOCK;
const MAGIC: u8 = 0xA5;

const SU: usize = 0x40; // SystemUpdate record
const SU_FLAG: usize = SU + 1;
const SU_PART: usize = SU + 2;
const SU_ERR: usize = SU + 3;
const SU_ALT_SRC: usize = SU + 8;
const SU_CRC: usize = SU + 0xF;
/// `su_part` value meaning "install the file named by `su_alt_src`".
const PART_USE_ALT_SRC: u8 = 3;

/// Image file the bootloader installs (argument of its `WriteFlashBin`).
#[derive(Clone, Copy)]
#[repr(u8)]
#[allow(dead_code)]
pub enum Source {
    /// `1:packetA/stm32/stm32_app.bin`; if writing it fails the bootloader
    /// retries it and then falls back to the System copy.
    PacketA = 0,
    /// `1:packetB/stm32/stm32_app.bin`
    PacketB = 1,
    /// `0:packet/stm32/stm32_app.bin` (factory copy; no fallback behind it).
    System = 2,
    /// `1:ota/stm32/stm32_app.bin` (where the stock OTA stages a new image).
    Ota = 3,
}

/// crc8, poly 0x31, init 0, MSB first (bootloader `0x08009A84`; the Apollo
/// link uses the same one).
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x31 } else { crc << 1 };
        }
    }
    crc
}

fn slot_addr(slot: usize) -> u32 {
    PAGE_ADDR + (slot * BLOCK) as u32
}

fn read_slot(slot: usize, out: &mut [u8; BLOCK]) {
    let p = slot_addr(slot) as *const u8;
    for (i, b) in out.iter_mut().enumerate() {
        // volatile: the flash contents change underneath us while we program
        *b = unsafe { core::ptr::read_volatile(p.add(i)) };
    }
}

fn block_valid(b: &[u8; BLOCK]) -> bool {
    b[0] == MAGIC && crc8(&b[..0xFF]) == b[BLOCK - 1]
}

/// Newest valid block, as the bootloader would find it.
fn read_block(out: &mut [u8; BLOCK]) -> bool {
    for slot in (0..SLOTS).rev() {
        read_slot(slot, out);
        if block_valid(out) {
            return true;
        }
    }
    false
}

/// Copy `out.len()` bytes from `offset` of the newest valid block. The
/// block holds more than the update record: the stock app keeps device
/// data in it too (see `services::factory`). False if there is no valid
/// block or the range does not fit.
pub fn read_record(offset: usize, out: &mut [u8]) -> bool {
    let mut block = [0u8; BLOCK];
    let Some(end) = offset.checked_add(out.len()).filter(|&end| end <= BLOCK) else {
        return false;
    };
    if !read_block(&mut block) {
        return false;
    }
    out.copy_from_slice(&block[offset..end]);
    true
}

/// Same algorithm as the bootloader's `flash_write_balance`.
fn write_block(block: &mut [u8; BLOCK]) -> Result<(), flash::Error> {
    block[0] = MAGIC;
    block[BLOCK - 1] = crc8(&block[..0xFF]);

    let mut tmp = [0u8; BLOCK];
    for slot in 0..SLOTS {
        read_slot(slot, &mut tmp);
        if tmp[0] == MAGIC {
            flash::program(slot_addr(slot), &[0u8; 8])?; // invalidate old block
        } else if tmp.iter().all(|&b| b == 0xFF) {
            return flash::program(slot_addr(slot), block);
        }
    }
    flash::erase_page_bank1(PAGE_NUMBER)?;
    flash::program(slot_addr(0), block)
}

/// Ask the bootloader to install `source` over the running app on the next
/// reset.
///
/// `report` is our only output channel for now: it travels in the record's
/// `err` byte, which the bootloader leaves alone after a successful install
/// and prints into `3:log/ota.log` (`...,err=<report>,roll=0`). The value 1
/// is reserved - the stock app treats err == 1 as a failed update and
/// reboots - so 1 is sent as 0.
///
/// Returns `Ok(true)` if an existing flag block was updated,
/// `Ok(false)` if none was found and a fresh one had to be created.
pub fn request_install(source: Source, report: u8) -> Result<bool, flash::Error> {
    let mut block = [0u8; BLOCK];
    let had_block = read_block(&mut block);
    if !had_block {
        block = [0u8; BLOCK];
    }
    // A clean record: one install request, nothing else in progress.
    //
    // * part = 3 + alt_src is the path the stock OTA itself uses. The other
    //   paths (part 0/1) make the bootloader set +0x45, which the stock app
    //   reads as "from_old" and answers with more OTA processing and a reboot.
    // * Everything else is zeroed because the stock app's boot check
    //   (`ota check index=...`) also resumes an OTA when index (+0x46),
    //   err (+0x43), +0x4C or bit 0 of +0x4E are set - and the OTA that was
    //   in progress is the one that installed *us*.
    block[SU..=SU_CRC].fill(0);
    block[SU] = MAGIC;
    block[SU_FLAG] = 1;
    block[SU_PART] = PART_USE_ALT_SRC;
    block[SU_ALT_SRC] = source as u8;
    block[SU_ERR] = if report == 1 { 0 } else { report };
    block[SU_CRC] = crc8(&block[SU..SU_CRC]);
    write_block(&mut block)?;

    // Read back: the bootloader must find exactly what we meant to write.
    let mut check = [0u8; BLOCK];
    if read_block(&mut check) && check[SU..=SU_CRC] == block[SU..=SU_CRC] {
        Ok(had_block)
    } else {
        Err(flash::Error(0))
    }
}

/// Image the bootloader restores when we leave.
pub const RETURN_TO: Source = Source::PacketA;

// Report byte (travels in the bootloader log as `err=`): 0x80 | stage << 4 | detail.
// The ticket is rewritten before every risky step, so after a hang (watchdog
// reset) the log still shows the last stage that was reached.
pub const STAGE_BOOTED: u8 = 0; // nothing else done yet
pub const STAGE_POWER: u8 = 1; // power pins set, about to talk to the card
pub const STAGE_INIT_FAILED: u8 = 2; // detail = sdmmc::Error value
pub const STAGE_LOG: u8 = 7; // detail 0 = log works, app running (or hung); 1..=7 = log failed at step; 15 = finished, log written

/// Book the return to the stock firmware, with a report of how far this
/// boot has come.
pub fn report(stage: u8, detail: u8) -> Result<bool, flash::Error> {
    let byte = 0x80 | (stage & 0x7) << 4 | (detail & 0xF);
    request_install(RETURN_TO, byte).or_else(|_| request_install(RETURN_TO, byte))
}
