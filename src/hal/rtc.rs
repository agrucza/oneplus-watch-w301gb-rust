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

/// The current date and time, or `None` if the clock was never set.
pub fn now() -> Option<DateTime> {
    unsafe {
        if read_volatile(RTC_ISR) & ISR_INITS == 0 {
            return None;
        }
        // Time and date are read one after the other; read again if the
        // clock ticked in between. Reading the sub-seconds first and the
        // date last is the order the chip's register latching expects.
        let (mut tr, mut dr);
        let mut tries = 0;
        loop {
            let before = read_volatile(RTC_SSR);
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
        Some(DateTime {
            year: 2000 + bcd((dr >> 16) & 0xFF) as u16,
            month: bcd((dr >> 8) & 0x1F),
            day: bcd(dr & 0x3F),
            weekday: ((dr >> 13) & 7) as u8,
            hour,
            minute: bcd((tr >> 8) & 0x7F),
            second: bcd(tr & 0x7F),
        })
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
