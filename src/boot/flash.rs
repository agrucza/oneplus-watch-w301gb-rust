//! Minimal internal-flash programming for the STM32L4R9, straight on the
//! registers (RM0432 ch. 3). Deliberately independent of the HAL: this is the
//! code the "return ticket" relies on, so it must not depend on clock setup,
//! interrupts or HAL start-up checks.

use core::ptr::{read_volatile, write_volatile};

const KEYR: *mut u32 = 0x4002_2008 as *mut u32;
const SR: *mut u32 = 0x4002_2010 as *mut u32;
const CR: *mut u32 = 0x4002_2014 as *mut u32;

const SR_BSY: u32 = 1 << 16;
/// OPERR, PROGERR, WRPERR, PGAERR, SIZERR, PGSERR, MISERR, FASTERR, RDERR, OPTVERR
const SR_ERRORS: u32 = 0xC3FA;
const CR_PG: u32 = 1 << 0;
const CR_PER: u32 = 1 << 1;
const CR_PNB_SHIFT: u32 = 3;
const CR_PNB_MASK: u32 = 0xFF << CR_PNB_SHIFT;
const CR_BKER: u32 = 1 << 11;
const CR_STRT: u32 = 1 << 16;
const CR_LOCK: u32 = 1 << 31;

/// Flash error: the raw error bits of FLASH_SR.
#[derive(Clone, Copy, defmt::Format)]
pub struct Error(pub u32);

unsafe fn wait_done() -> Result<(), Error> {
    while read_volatile(SR) & SR_BSY != 0 {}
    let err = read_volatile(SR) & SR_ERRORS;
    if err != 0 {
        write_volatile(SR, err); // write-1-to-clear
        return Err(Error(err));
    }
    Ok(())
}

unsafe fn unlock() {
    if read_volatile(CR) & CR_LOCK != 0 {
        write_volatile(KEYR, 0x4567_0123);
        write_volatile(KEYR, 0xCDEF_89AB);
    }
    write_volatile(SR, SR_ERRORS); // clear stale errors
}

unsafe fn lock() {
    write_volatile(CR, read_volatile(CR) | CR_LOCK);
}

/// Program `data` (multiple of 8 bytes) at `addr` (8-byte aligned).
/// The target must be erased, except that all-zero double words may be
/// written over anything (used to invalidate old records).
pub fn program(addr: u32, data: &[u8]) -> Result<(), Error> {
    assert!(addr % 8 == 0 && data.len() % 8 == 0);
    cortex_m::interrupt::free(|_| unsafe {
        unlock();
        let mut res = wait_done();
        if res.is_ok() {
            write_volatile(CR, read_volatile(CR) | CR_PG);
            let mut a = addr as *mut u32;
            for dw in data.chunks_exact(8) {
                write_volatile(a, u32::from_le_bytes([dw[0], dw[1], dw[2], dw[3]]));
                write_volatile(a.add(1), u32::from_le_bytes([dw[4], dw[5], dw[6], dw[7]]));
                res = wait_done();
                if res.is_err() {
                    break;
                }
                a = a.add(2);
            }
            write_volatile(CR, read_volatile(CR) & !CR_PG);
        }
        lock();
        res
    })
}

/// Erase one 4 KB page of bank 1 (dual-bank layout): page n = 0x0800_0000 + n * 0x1000.
pub fn erase_page_bank1(page: u8) -> Result<(), Error> {
    cortex_m::interrupt::free(|_| unsafe {
        unlock();
        let mut res = wait_done();
        if res.is_ok() {
            let cr = read_volatile(CR) & !(CR_PNB_MASK | CR_BKER | CR_PG);
            write_volatile(CR, cr | CR_PER | ((page as u32) << CR_PNB_SHIFT));
            write_volatile(CR, read_volatile(CR) | CR_STRT);
            res = wait_done();
            write_volatile(CR, read_volatile(CR) & !(CR_PER | CR_PNB_MASK));
        }
        lock();
        res
    })
}
