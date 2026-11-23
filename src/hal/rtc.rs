//! The STM32's real-time clock, at register level.
//!
//! The clock runs from the 32.768 kHz crystal in the backup domain and
//! keeps going through resets and firmware changes. The stock app sets it
//! to local time (`0x0802DEC0`: the phone's GMT seconds plus the time zone
//! in minutes times 60), so that is what it holds here too.
//!
//! The HAL's `Rtc` is not used: creating one puts the clock into its
//! set-up mode, which stops it for a moment and restarts the running
//! second, on every boot. Here the clock is only read, and written only
//! when the time is set; the stock app's prescalers and format are left as
//! they are.

use core::ptr::{read_volatile, write_volatile};

use crate::hal::reg::{modify, RCC_APB1ENR1};

const RTC_TR: *mut u32 = 0x4000_2800 as *mut u32;
const RTC_DR: *mut u32 = 0x4000_2804 as *mut u32;
const RTC_CR: *mut u32 = 0x4000_2808 as *mut u32;
const RTC_ISR: *mut u32 = 0x4000_280C as *mut u32;
const RTC_PRER: *mut u32 = 0x4000_2810 as *mut u32;
const RTC_WPR: *mut u32 = 0x4000_2824 as *mut u32;
const RTC_SSR: *mut u32 = 0x4000_2828 as *mut u32;
const RTC_CALR: *const u32 = 0x4000_283C as *const u32;
/// Backup register 3: the stock app keeps the time zone here (see
/// `time_zone`).
const RTC_BKP3R: *mut u32 = 0x4000_285C as *mut u32;
const PWR_CR1: *mut u32 = 0x4000_7000 as *mut u32;

const APB1ENR1_RTCAPBEN: u32 = 1 << 10;
const PWR_CR1_DBP: u32 = 1 << 8;
const CR_FMT_12H: u32 = 1 << 6;
const ISR_INIT: u32 = 1 << 7;
const ISR_INITF: u32 = 1 << 6;
const ISR_INITS: u32 = 1 << 4;
const TR_PM: u32 = 1 << 22;

/// A date and time as the clock holds it (local time).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    /// 1 = Monday ... 7 = Sunday.
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Seconds since 1970-01-01 00:00:00, taking the fields as they are
    /// (no time zone applied). Valid for 2000 to 2099.
    pub fn unix_seconds(&self) -> u32 {
        const DAYS_BEFORE_MONTH: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let years = (self.year - 2000) as u32;
        // 2000-01-01 is day 10957; leap days before this year, then this year's.
        let mut days = 10957 + years * 365 + (years + 3) / 4 + DAYS_BEFORE_MONTH[(self.month - 1) as usize] + self.day as u32 - 1;
        if self.year % 4 == 0 && self.month > 2 {
            days += 1;
        }
        days * 86400 + self.hour as u32 * 3600 + self.minute as u32 * 60 + self.second as u32
    }
}

/// The clock's set-up registers, for the log.
pub struct State {
    pub cr: u32,
    pub isr: u32,
    pub prer: u32,
}

/// Switch on the clock's register interface. Call once before anything else.
pub fn enable() -> State {
    unsafe {
        modify(RCC_APB1ENR1, 0, APB1ENR1_RTCAPBEN);
        let _ = read_volatile(RCC_APB1ENR1); // let the clock enable take effect
        State { cr: read_volatile(RTC_CR), isr: read_volatile(RTC_ISR), prer: read_volatile(RTC_PRER) }
    }
}

fn bcd(value: u32) -> u8 {
    ((value >> 4) * 10 + (value & 0xF)) as u8
}

fn to_bcd(value: u8) -> u32 {
    ((value / 10) << 4 | value % 10) as u32
}

/// The clock's calibration register: how many of the crystal's pulses
/// the clock is told to skip (bits 8:0, out of every 2^20) or to add
/// (bit 15: 512 more), to correct a crystal that runs fast or slow. Kept
/// in the backup domain, so whatever the stock firmware wrote is still
/// in force.
pub fn calibration() -> u32 {
    unsafe { read_volatile(RTC_CALR) }
}

/// The current date and time, or `None` if the clock was never set.
pub fn now() -> Option<DateTime> {
    now_ms().map(|(time, _)| time)
}

/// The current date and time with the milliseconds into the second.
pub fn now_ms() -> Option<(DateTime, u16)> {
    unsafe {
        if read_volatile(RTC_ISR) & ISR_INITS == 0 {
            return None;
        }
        // Time and date are read one after the other; read again if the
        // clock ticked in between. Reading the sub-seconds first and the
        // date last is the order the chip's register latching expects.
        let (mut tr, mut dr, mut ss);
        let mut tries = 0;
        loop {
            let before = read_volatile(RTC_SSR);
            ss = before;
            tr = read_volatile(RTC_TR);
            dr = read_volatile(RTC_DR);
            if read_volatile(RTC_SSR) == before && read_volatile(RTC_TR) == tr {
                let _ = read_volatile(RTC_DR);
                break;
            }
            let _ = read_volatile(RTC_DR);
            tries += 1;
            if tries == 5 {
                return None;
            }
        }
        let mut hour = bcd((tr >> 16) & 0x3F);
        if read_volatile(RTC_CR) & CR_FMT_12H != 0 {
            hour = hour % 12 + if tr & TR_PM != 0 { 12 } else { 0 };
        }
        // The sub-second counter runs down from the prescaler's value.
        let steps = (read_volatile(RTC_PRER) & 0x7FFF) + 1;
        let ms = (steps.saturating_sub(1).saturating_sub(ss & 0xFFFF) * 1000 / steps).min(999) as u16;
        let time = DateTime {
            year: 2000 + bcd((dr >> 16) & 0xFF) as u16,
            month: bcd((dr >> 8) & 0x1F),
            day: bcd(dr & 0x3F),
            weekday: ((dr >> 13) & 7) as u8,
            hour,
            minute: bcd((tr >> 8) & 0x7F),
            second: bcd(tr & 0x7F),
        };
        Some((time, ms))
    }
}

/// The time zone: minutes that local time is ahead of GMT.
///
/// The stock app keeps it in backup register 3 as a signed 16-bit number
/// (read at 0x0802E146, written at 0x0802DD1A and 0x0802DEE2) and works
/// out GMT as local time minus this (0x0802E1F4). The register keeps its
/// value as long as the clock does, so both firmwares see the same zone.
pub fn time_zone() -> i16 {
    unsafe { read_volatile(RTC_BKP3R) as u16 as i16 }
}

/// Seconds since 1970 in GMT and the time zone in minutes; `None` if the
/// clock was never set.
pub fn gmt_and_zone() -> Option<(u32, i16)> {
    let zone = time_zone();
    let local = now()?.unix_seconds() as i64;
    Some(((local - zone as i64 * 60) as u32, zone))
}

/// Store the time zone (see `time_zone`).
pub fn set_time_zone(minutes: i16) {
    unsafe {
        modify(PWR_CR1, 0, PWR_CR1_DBP); // allow writes to the backup domain
        write_volatile(RTC_BKP3R, minutes as u16 as u32);
    }
}

/// Set the clock. The fields must be valid (the caller checks them); the
/// year must be 2000 to 2099. False if the clock did not enter its set-up
/// mode.
pub fn set(t: &DateTime) -> bool {
    unsafe {
        modify(PWR_CR1, 0, PWR_CR1_DBP); // allow writes to the backup domain
        write_volatile(RTC_WPR, 0xCA); // remove the write protection
        write_volatile(RTC_WPR, 0x53);
        modify(RTC_ISR, 0, ISR_INIT);
        let mut entered = false;
        // Takes two clock periods (about 60 us); allow far more.
        for _ in 0..200_000 {
            if read_volatile(RTC_ISR) & ISR_INITF != 0 {
                entered = true;
                break;
            }
        }
        if entered {
            let twelve_hour = read_volatile(RTC_CR) & CR_FMT_12H != 0;
            let (hour, pm) = match (twelve_hour, t.hour) {
                (false, h) => (h, 0),
                (true, 0) => (12, 0),
                (true, h @ 1..=11) => (h, 0),
                (true, 12) => (12, TR_PM),
                (true, h) => (h - 12, TR_PM),
            };
            write_volatile(RTC_TR, pm | to_bcd(hour) << 16 | to_bcd(t.minute) << 8 | to_bcd(t.second));
            write_volatile(
                RTC_DR,
                to_bcd((t.year - 2000) as u8) << 16 | (t.weekday as u32) << 13 | to_bcd(t.month) << 8 | to_bcd(t.day),
            );
        }
        modify(RTC_ISR, ISR_INIT, 0); // the clock runs on from the new time
        write_volatile(RTC_WPR, 0xFF);
        entered
    }
}

/// Where in the second a deliberate reset is made, in milliseconds.
///
/// The stock bootloader sets the clock up afresh at every start
/// (0x0800C034 calls its `HAL_RTC_Init` at 0x0800AB28, which enters the
/// clock's initialisation mode and rewrites the prescalers without
/// looking whether the clock is already running). That drops the part of
/// the current second that has passed. Measured: the clock does not
/// drift while the watch runs (5 ms in an hour), and one restart cost
/// 0.73 s. The bootloader cannot be changed, but the moment of our own
/// resets can: made so that the bootloader's set-up falls just after the
/// start of a second, there is next to nothing to drop.
///
/// Measured with resets made at the start of a second: each of four
/// restarts cost 0.261 to 0.271 s, which is the time the bootloader takes
/// to reach its clock set-up. A reset 20 ms later than 1000 minus that
/// puts the set-up about 20 ms into the next second: late enough never
/// to fall before the second's start, which would cost a whole second.
const RESET_AT_MS: u16 = 1000 - 264 + 20;

/// Put back the one setting of the clock that the HAL changes for its
/// Stop-mode support: it reads the clock's registers directly
/// (RTC_CR.BYPSHAD) instead of through their shadow copies, which lag
/// after a stop. The setting lives in the backup domain and would stay
/// for the stock firmware, which did not choose it. Called before the
/// firmware hands over on purpose; after a fault it stays.
pub fn hand_back() {
    const CR_BYPSHAD: u32 = 1 << 5;
    unsafe {
        modify(PWR_CR1, 0, PWR_CR1_DBP);
        write_volatile(RTC_WPR, 0xCA);
        write_volatile(RTC_WPR, 0x53);
        modify(RTC_CR, CR_BYPSHAD, 0);
        write_volatile(RTC_WPR, 0xFF);
    }
}

/// Reset the processor at the moment in the second that costs the clock
/// least (`RESET_AT_MS`); at once if the clock is not running. Waits up
/// to a second, with everything else standing still.
pub fn reset_on_the_second() -> ! {
    // Several seconds' worth of looking at the clock, should it stand.
    // Generous on purpose: giving up before the moment has come would
    // reset at an arbitrary one.
    for _ in 0..20_000_000u32 {
        match now_ms() {
            Some((_, ms)) if (ms + 1000 - RESET_AT_MS) % 1000 < 8 => break,
            Some(_) => {}
            None => break,
        }
    }
    hand_back();
    cortex_m::peripheral::SCB::sys_reset()
}
