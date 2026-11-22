//! Minimal eMMC driver for SDMMC1 on the STM32L4R9, straight on the registers.
//!
//! The Embassy HAL (0.6) has no SDMMC driver for the L4+ family - its chip
//! data lists SDMMC1 without a register block - so this follows the reference
//! manual (RM0432, SDMMC chapter) and the sequence of ST's `HAL_MMC_Init`
//! that the stock bootloader uses.
//!
//! Polling. `init` leaves the card at 12 MHz on one data line;
//! `high_speed` switches card and controller to 48 MHz and
//! `four_data_lines` to the 4-bit bus, as the stock app does (0x0804D93A
//! and 0x0804D9D2 in B.70). Reads can cover several sectors per command
//! (`read_sectors`), and so can writes (`write_sectors`).
//!
//! Wiring (bootloader `HAL_MMC_MspInit`, 0x08007F66): CLK PC12, CMD PD2,
//! D0..D3 PC8..PC11, all alternate function 12, pull-up, very high speed.
//! Kernel clock: CLK48 = MSI 48 MHz (set up in `clocks`).

use core::ptr::{read_volatile, write_volatile};

use embassy_time::{Duration, Instant};

use crate::hal::reg::{RCC_AHB2ENR, RCC_AHB2RSTR, modify};

const GPIOC: usize = 0x4800_0800;
const GPIOD: usize = 0x4800_0C00;
const SDMMC1: usize = 0x5006_2400;

const POWER: *mut u32 = SDMMC1 as *mut u32;
const CLKCR: *mut u32 = (SDMMC1 + 0x04) as *mut u32;
const ARGR: *mut u32 = (SDMMC1 + 0x08) as *mut u32;
const CMDR: *mut u32 = (SDMMC1 + 0x0C) as *mut u32;
const RESP1R: *const u32 = (SDMMC1 + 0x14) as *const u32;
const DTIMER: *mut u32 = (SDMMC1 + 0x24) as *mut u32;
const DLENR: *mut u32 = (SDMMC1 + 0x28) as *mut u32;
const DCTRL: *mut u32 = (SDMMC1 + 0x2C) as *mut u32;
const STAR: *const u32 = (SDMMC1 + 0x34) as *const u32;
const ICR: *mut u32 = (SDMMC1 + 0x38) as *mut u32;
const IDMACTRLR: *mut u32 = (SDMMC1 + 0x50) as *mut u32;
const IDMABASE0R: *mut u32 = (SDMMC1 + 0x58) as *mut u32;

// SDMMC_STAR bits
const CCRCFAIL: u32 = 1 << 0;
const DCRCFAIL: u32 = 1 << 1;
const CTIMEOUT: u32 = 1 << 2;
const DTIMEOUT: u32 = 1 << 3;
const TXUNDERR: u32 = 1 << 4;
const RXOVERR: u32 = 1 << 5;
const CMDREND: u32 = 1 << 6;
const CMDSENT: u32 = 1 << 7;
const DATAEND: u32 = 1 << 8;
const BUSYD0: u32 = 1 << 20;
const IDMATE: u32 = 1 << 27;
const ALL_FLAGS: u32 = 0x1FE0_0FFF;

// SDMMC_CMDR bits
const CMDTRANS: u32 = 1 << 6;
const CPSMEN: u32 = 1 << 12;

const KERNEL_HZ: u32 = crate::hal::clocks::SDMMC_KERNEL_HZ;

/// The card clock in Hz, for the data timeouts.
static CARD_HZ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(12_000_000);

/// The card's capacity in sectors, from its extended settings.
static CARD_SECTORS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Set by `init`: cards up to 2 GB take byte addresses, larger ones sectors.
static BYTE_ADDRESSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// True if the card answered CMD1 with byte addressing (capacity <= 2 GB).
pub fn byte_addressed() -> bool {
    BYTE_ADDRESSED.load(core::sync::atomic::Ordering::Relaxed)
}
const RCA: u32 = 2; // relative card address we assign

/// Where bring-up or a transfer failed. The numeric values go into the
/// report byte, so keep them stable (1..=15).
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
pub enum Error {
    /// CMD1 got no answer at all: no card, no power, or wrong pins.
    NoAnswer = 1,
    /// CMD1 answered but the card never left its power-up busy state.
    NeverReady = 2,
    /// CMD2 (read card ID) failed.
    Identify = 3,
    /// CMD3 (assign address) failed.
    Address = 4,
    /// CMD7 (select card) failed.
    Select = 5,
    /// CMD16 (block length) failed.
    BlockLen = 6,
    /// A read/write command was not accepted.
    Command = 7,
    /// Data CRC error.
    DataCrc = 8,
    /// Card did not deliver the data in time.
    DataTimeout = 9,
    /// Receive overrun or DMA transfer error.
    Overrun = 10,
    /// The controller never finished (our own deadline).
    Stuck = 11,
    /// The card stayed busy, or reported an error, after a write.
    Program = 12,
    /// The card did not take a change of its settings (CMD6).
    Switch = 13,
}

#[derive(Clone, Copy, PartialEq)]
enum Resp {
    None = 0,
    Short = 1,
    /// Short response without CRC check (R3: the OCR carries no CRC).
    ShortNoCrc = 2,
    Long = 3,
}

enum CmdError {
    Timeout,
    Crc,
    Stuck,
}

fn spin_for(d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {}
}

/// Send one command and wait for it to complete. Returns the short response.
fn command(index: u32, arg: u32, resp: Resp, with_data: bool) -> Result<u32, CmdError> {
    unsafe {
        write_volatile(ICR, ALL_FLAGS);
        write_volatile(ARGR, arg);
        let trans = if with_data { CMDTRANS } else { 0 };
        write_volatile(CMDR, index | (resp as u32) << 8 | trans | CPSMEN);

        let done = if resp == Resp::None { CMDSENT } else { CMDREND | CCRCFAIL | CTIMEOUT };
        let end = Instant::now() + Duration::from_millis(500);
        loop {
            let st = read_volatile(STAR);
            if st & done != 0 {
                if st & CTIMEOUT != 0 {
                    return Err(CmdError::Timeout);
                }
                if st & CCRCFAIL != 0 {
                    return Err(CmdError::Crc);
                }
                return Ok(read_volatile(RESP1R));
            }
            if Instant::now() > end {
                return Err(CmdError::Stuck);
            }
        }
    }
}

/// Wait until the card releases D0 (busy signalling after R1b commands).
fn wait_not_busy(limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while unsafe { read_volatile(STAR) } & BUSYD0 != 0 {
        if Instant::now() > end {
            return false;
        }
    }
    true
}

fn set_clock(hz: u32) {
    // SDMMC_CK = kernel clock / (2 * CLKDIV); 1-bit bus, no power saving,
    // no hardware flow control.
    let div = KERNEL_HZ.div_ceil(2 * hz).clamp(1, 0x3FF);
    unsafe { write_volatile(CLKCR, div) };
}

/// Pins, clock and reset of the controller (what `HAL_MMC_MspInit` does).
fn hardware_init() {
    unsafe {
        // Clocks: GPIOC (bit 2), GPIOD (bit 3), SDMMC1 (bit 22).
        modify(RCC_AHB2ENR, 0, 1 << 2 | 1 << 3 | 1 << 22);
        let _ = read_volatile(RCC_AHB2ENR);
        // Start from a clean controller.
        modify(RCC_AHB2RSTR, 0, 1 << 22);
        modify(RCC_AHB2RSTR, 1 << 22, 0);

        // GPIO registers: MODER +0x00, OSPEEDR +0x08, PUPDR +0x0C, AFRL +0x20, AFRH +0x24.
        let pin = |port: usize, n: u32| {
            modify(port as *mut u32, 0b11 << (2 * n), 0b10 << (2 * n)); // MODER: alternate function
            modify((port + 0x08) as *mut u32, 0, 0b11 << (2 * n)); // very high speed
            modify((port + 0x0C) as *mut u32, 0b11 << (2 * n), 0b01 << (2 * n)); // pull-up
            let afr = (port + 0x20 + 4 * (n as usize / 8)) as *mut u32;
            modify(afr, 0xF << (4 * (n % 8)), 12 << (4 * (n % 8))); // AF12 = SDMMC1
        };
        for n in 8..=12 {
            pin(GPIOC, n); // D0..D3, CLK
        }
        pin(GPIOD, 2); // CMD
    }
}

/// Bring the card from power-up to transfer state.
/// Returns the card's operating-conditions register (OCR).
pub fn init() -> Result<u32, Error> {
    hardware_init();

    set_clock(150_000); // identification speed (the stock firmware uses divider 160)
    unsafe { write_volatile(POWER, 0b11) }; // clock the card
    spin_for(Duration::from_millis(5)); // well over the required 74 clock cycles

    // CMD0: go idle.
    let _ = command(0, 0, Resp::None, false);
    spin_for(Duration::from_millis(2));

    // CMD1: negotiate voltage, ask for sector addressing (same argument as
    // the stock firmware). Repeat until the
    // card reports power-up complete (bit 31).
    let end = Instant::now() + Duration::from_secs(2);
    let ocr = loop {
        match command(1, 0xC0FF_8000, Resp::ShortNoCrc, false) {
            Ok(ocr) if ocr & 1 << 31 != 0 => break ocr,
            Ok(_) => {}
            Err(_) => return Err(Error::NoAnswer),
        }
        if Instant::now() > end {
            return Err(Error::NeverReady);
        }
        spin_for(Duration::from_millis(10));
    };

    // OCR bits 30:29: 2 = sector addressing, 0 = byte addressing.
    BYTE_ADDRESSED.store((ocr >> 29) & 0b11 != 2, core::sync::atomic::Ordering::Relaxed);

    // CMD2: card ID (not used, but required by the state machine).
    command(2, 0, Resp::Long, false).map_err(|_| Error::Identify)?;
    // CMD3: assign the relative address.
    command(3, RCA << 16, Resp::Short, false).map_err(|_| Error::Address)?;
    // CMD7: select the card (answers with busy).
    command(7, RCA << 16, Resp::Short, false).map_err(|_| Error::Select)?;
    if !wait_not_busy(Duration::from_millis(500)) {
        return Err(Error::Select);
    }

    set_clock(12_000_000);
    CARD_HZ.store(12_000_000, core::sync::atomic::Ordering::Relaxed);

    // CMD16: 512-byte blocks.
    command(16, 512, Resp::Short, false).map_err(|_| Error::BlockLen)?;
    Ok(ocr)
}

fn card_hz() -> u32 {
    CARD_HZ.load(core::sync::atomic::Ordering::Relaxed)
}

/// Write one byte of the card's extended settings (CMD6, "write byte"),
/// wait until the card is done and check that it took the value.
fn switch(index: u32, value: u32) -> Result<(), Error> {
    command(6, 3 << 24 | index << 16 | value << 8, Resp::Short, false).map_err(|_| Error::Switch)?;
    if !wait_not_busy(Duration::from_millis(500)) {
        return Err(Error::Switch);
    }
    // Status (CMD13): bit 7 = the switch failed; bits 12:9 = state, 4 =
    // ready for transfers.
    let end = Instant::now() + Duration::from_millis(500);
    loop {
        match command(13, RCA << 16, Resp::Short, false) {
            Ok(status) if status & 1 << 7 != 0 => return Err(Error::Switch),
            Ok(status) if (status >> 9) & 0xF == 4 => return Ok(()),
            Ok(_) => {}
            Err(_) => return Err(Error::Switch),
        }
        if Instant::now() > end {
            return Err(Error::Switch);
        }
    }
}

/// Switch the card to its high-speed timing and the clock to 48 MHz.
///
/// As the stock app (0x0804D93A in B.70): CMD6 with the argument
/// 0x03B90100 (extended setting 185, "HS_TIMING", = 1), then the
/// controller's clock divider 0 (the 48 MHz kernel clock undivided) with
/// its high-speed bit (CLKCR bit 19) set.
///
/// On an error the card may be in either timing: call `init` again.
pub fn high_speed() -> Result<(), Error> {
    switch(185, 1)?;
    unsafe { modify(CLKCR, 0x3FF, 1 << 19) };
    CARD_HZ.store(KERNEL_HZ, core::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// Switch the card and the controller to four data lines.
///
/// As the stock app (0x0804D9D2 in B.70): CMD6 with the argument
/// 0x03B70100 (extended setting 183, "BUS_WIDTH", = 1), then the
/// controller's bus width field (CLKCR bits 15:14) = 1.
///
/// On an error the card may be in either width: call `init` again.
pub fn four_data_lines() -> Result<(), Error> {
    switch(183, 1)?;
    unsafe { modify(CLKCR, 0b11 << 14, 0b01 << 14) };
    Ok(())
}

/// Read one 512-byte sector. `buf` is word-aligned because the controller's
/// internal DMA writes it directly.
pub fn read_sector(sector: u32, buf: &mut [u32; 128]) -> Result<(), Error> {
    read_sectors(sector, buf)
}

/// Read consecutive sectors into `buf`, whose length in words must be a
/// multiple of 128 (one sector) and at most 65535 sectors.
///
/// One sector goes as CMD17. More go as the stock app sends them
/// (0x080864D6 in B.70): CMD23 with the number of sectors, then CMD18; the
/// card stops by itself after that many, so no stop command is needed.
pub fn read_sectors(sector: u32, buf: &mut [u32]) -> Result<(), Error> {
    let count = (buf.len() / 128) as u32;
    if count == 0 || buf.len() % 128 != 0 || count > 0xFFFF {
        return Err(Error::Command);
    }
    let address = if byte_addressed() { sector.wrapping_mul(512) } else { sector };
    read_data(if count > 1 { 18 } else { 17 }, address, buf)
}

/// The card's extended settings ("EXT_CSD", CMD8): 512 bytes.
pub fn read_extended_settings(buf: &mut [u32; 128]) -> Result<(), Error> {
    read_data(8, 0, buf)
}

/// What the card says about itself in its extended settings.
#[derive(Clone, Copy)]
pub struct CardInfo {
    /// Capacity in 512-byte sectors (bytes 212 to 215).
    pub sectors: u32,
    /// Version of the settings table (byte 192).
    pub revision: u8,
    /// Speeds the card supports (byte 196): bit 0 = 26 MHz, bit 1 = 52 MHz.
    pub device_type: u8,
    /// Timing in use (byte 185): 0 = slow, 1 = high speed.
    pub timing: u8,
    /// Bus width in use (byte 183): 0 = one, 1 = four, 2 = eight lines.
    pub bus_width: u8,
    /// Power-off notice state (byte 34).
    pub power_notice: u8,
}

/// Read the card's extended settings and pick out `CardInfo`. The capacity
/// is remembered for `sectors`.
pub fn card_info() -> Result<CardInfo, Error> {
    let mut words = [0u32; 128];
    read_extended_settings(&mut words)?;
    let byte = |n: usize| (words[n / 4] >> (8 * (n % 4))) as u8;
    let info = CardInfo {
        sectors: words[212 / 4],
        revision: byte(192),
        device_type: byte(196),
        timing: byte(185),
        bus_width: byte(183),
        power_notice: byte(34),
    };
    CARD_SECTORS.store(info.sectors, core::sync::atomic::Ordering::Relaxed);
    Ok(info)
}

/// The card's capacity in sectors; 0 until `card_info` has run.
pub fn sectors() -> u32 {
    CARD_SECTORS.load(core::sync::atomic::Ordering::Relaxed)
}

/// One data transfer from the card into `buf` with the given command.
/// More than one sector is announced with CMD23 first.
fn read_data(index: u32, address: u32, buf: &mut [u32]) -> Result<(), Error> {
    let count = (buf.len() / 128) as u32;
    unsafe {
        write_volatile(ICR, ALL_FLAGS);
        if count > 1 {
            command(23, count, Resp::Short, false).map_err(|_| Error::Command)?;
        }
        write_volatile(DTIMER, card_hz()); // 1 s of card clock
        write_volatile(DLENR, count * 512);
        write_volatile(IDMABASE0R, buf.as_mut_ptr() as u32);
        write_volatile(IDMACTRLR, 1); // internal DMA, single buffer
        write_volatile(DCTRL, 9 << 4 | 1 << 1); // 512-byte blocks, card -> host

        let result = match command(index, address, Resp::Short, true) {
            Err(_) => Err(Error::Command),
            Ok(_) => {
                let end = Instant::now() + Duration::from_secs(2);
                loop {
                    let st = read_volatile(STAR);
                    if st & DCRCFAIL != 0 {
                        break Err(Error::DataCrc);
                    } else if st & DTIMEOUT != 0 {
                        break Err(Error::DataTimeout);
                    } else if st & (RXOVERR | IDMATE) != 0 {
                        break Err(Error::Overrun);
                    } else if st & DATAEND != 0 {
                        break Ok(());
                    } else if Instant::now() > end {
                        break Err(Error::Stuck);
                    }
                }
            }
        };

        write_volatile(IDMACTRLR, 0);
        write_volatile(DCTRL, 0);
        write_volatile(ICR, ALL_FLAGS);
        result
    }
}

/// Write consecutive sectors from `buf`, whose length in words must be a
/// multiple of 128 (one sector) and at most 65535 sectors, and wait until
/// the card has stored them.
///
/// One sector goes as CMD24. More go as the stock app sends them
/// (0x080865C6 in B.70): CMD23 with the number of sectors, then CMD25.
pub fn write_sectors(sector: u32, buf: &[u32]) -> Result<(), Error> {
    let count = (buf.len() / 128) as u32;
    if count == 0 || buf.len() % 128 != 0 || count > 0xFFFF {
        return Err(Error::Command);
    }
    unsafe {
        write_volatile(ICR, ALL_FLAGS);
        if count > 1 {
            command(23, count, Resp::Short, false).map_err(|_| Error::Command)?;
        }
        write_volatile(DTIMER, 4 * card_hz()); // 4 s of card clock
        write_volatile(DLENR, count * 512);
        write_volatile(IDMABASE0R, buf.as_ptr() as u32);
        write_volatile(IDMACTRLR, 1); // internal DMA, single buffer
        write_volatile(DCTRL, 9 << 4); // 512-byte blocks, host -> card

        let address = if byte_addressed() { sector.wrapping_mul(512) } else { sector };
        let result = match command(if count > 1 { 25 } else { 24 }, address, Resp::Short, true) {
            Err(_) => Err(Error::Command),
            Ok(_) => {
                let end = Instant::now() + Duration::from_secs(5);
                loop {
                    let st = read_volatile(STAR);
                    if st & DCRCFAIL != 0 {
                        break Err(Error::DataCrc);
                    } else if st & DTIMEOUT != 0 {
                        break Err(Error::DataTimeout);
                    } else if st & (TXUNDERR | IDMATE) != 0 {
                        break Err(Error::Overrun);
                    } else if st & DATAEND != 0 {
                        break Ok(());
                    } else if Instant::now() > end {
                        break Err(Error::Stuck);
                    }
                }
            }
        };

        write_volatile(IDMACTRLR, 0);
        write_volatile(DCTRL, 0);
        write_volatile(ICR, ALL_FLAGS);
        result?;
    }

    // The card programs the blocks after the transfer. Ask for its status
    // (CMD13) until it is back in the transfer state and ready for data.
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match command(13, RCA << 16, Resp::Short, false) {
            // bits 12:9 = state (4 = transfer), bit 8 = ready for data,
            // bits 31:16 and 15,7 carry error flags.
            Ok(status) if status & 0xFDF9_8080 != 0 => return Err(Error::Program),
            Ok(status) if (status >> 9) & 0xF == 4 && status & 1 << 8 != 0 => return Ok(()),
            Ok(_) => {}
            Err(_) => return Err(Error::Program),
        }
        if Instant::now() > end {
            return Err(Error::Program);
        }
    }
}

/// Tell the card to end a transfer (CMD12) and wait until it is ready
/// again; for getting it out of a multi-sector transfer that failed.
pub fn stop_transfer() {
    let _ = command(12, 0, Resp::Short, false);
    let _ = wait_not_busy(Duration::from_millis(500));
    unsafe { write_volatile(ICR, ALL_FLAGS) };
}
