//! The date and time of day.
//!
//! The clock holds local time, as under the stock firmware (see
//! `hal::rtc`). It is set from the PC with `tools/ota.sh time`, which sends
//! `AT^W301TIME=YYYY-MM-DD HH:MM:SS +HHMM`; the last part is the time zone
//! and may be left out, the zone then stays as it is.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::hal::rtc;
pub use crate::hal::rtc::DateTime;
use crate::services::log::Log;

/// Set when the clock or the time zone was changed; see `take_changed`.
static CHANGED: AtomicBool = AtomicBool::new(false);

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

impl DateTime {
    /// "Mon" ... "Sun".
    pub fn weekday_name(&self) -> &'static str {
        WEEKDAYS.get(self.weekday.wrapping_sub(1) as usize).copied().unwrap_or("???")
    }
}

/// "2026-11-22 12:45:10"
impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

/// Switch the clock's interface on and log what it says.
pub fn start(log: &mut Log) {
    let state = rtc::enable();
    match rtc::now() {
        Some(t) => {
            let _ = write!(log.stamp(), "clock: {} {}", t.weekday_name(), t);
        }
        None => {
            let _ = write!(log.stamp(), "clock: not set");
        }
    }
    let _ = write!(log, ", zone {} min", rtc::time_zone());
    let _ = writeln!(log, " (RTC_CR={:#010x} ISR={:#010x} PRER={:#010x})", state.cr, state.isr, state.prer);
}

/// True once after the clock or the time zone was set: whoever keeps a
/// copy of the time (the Apollo) has to be told.
pub fn take_changed() -> bool {
    CHANGED.swap(false, Ordering::Relaxed)
}

/// "+HHMM" or "-HHMM" as minutes; at most 14 hours either way.
fn parse_zone(text: &str) -> Option<i16> {
    let b = text.as_bytes();
    if b.len() != 5 || !b[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let sign = match b[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (hours, minutes) = (text[1..3].parse::<i16>().ok()?, text[3..5].parse::<i16>().ok()?);
    (hours <= 14 && minutes < 60).then_some(sign * (hours * 60 + minutes))
}

/// The current local date and time; `None` if the clock was never set.
pub fn now() -> Option<DateTime> {
    rtc::now()
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 => 29, // enough for 2000 to 2099
        2 => 28,
        _ => 31,
    }
}

/// Day of the week for a date from 2000 to 2099: 1 = Monday ... 7 = Sunday.
fn weekday(year: u16, month: u8, day: u8) -> u8 {
    let mut days = (year - 2000) as u32 * 365 + (year - 2000 + 3) as u32 / 4;
    for m in 1..month {
        days += days_in_month(year, m) as u32;
    }
    days += day as u32 - 1;
    // 2000-01-01 was a Saturday (6).
    ((days + 5) % 7) as u8 + 1
}

/// Parse "YYYY-MM-DD HH:MM:SS"; `None` if it is not a valid time between
/// 2000 and 2099.
pub fn parse(text: &str) -> Option<DateTime> {
    let b = text.as_bytes();
    if b.len() != 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let number = |range: core::ops::Range<usize>| text.get(range)?.parse::<u16>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)? as u8, number(8..10)? as u8);
    let (hour, minute, second) = (number(11..13)? as u8, number(14..16)? as u8, number(17..19)? as u8);
    let valid = (2000..=2099).contains(&year)
        && (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
        && hour < 24
        && minute < 60
        && second < 60;
    valid.then(|| DateTime { year, month, day, weekday: weekday(year, month, day), hour, minute, second })
}

/// Set the clock from "YYYY-MM-DD HH:MM:SS" (local time). False if the text
/// is not a valid time or the clock did not take it.
pub fn set_from_text(text: &str, log: &mut Log) -> bool {
    // An optional time zone follows the time: " +0100", " -0530".
    let (text, zone) = match text.get(19..) {
        Some(rest) if !rest.is_empty() => match parse_zone(rest.trim()) {
            Some(zone) => (&text[..19], Some(zone)),
            None => {
                let _ = writeln!(log.stamp(), "clock: {:?} is not a time zone", rest);
                return false;
            }
        },
        _ => (text, None),
    };
    if let Some(zone) = zone {
        let before = rtc::time_zone();
        rtc::set_time_zone(zone);
        let _ = writeln!(log.stamp(), "clock: time zone set to {} min ahead of GMT (was {})", rtc::time_zone(), before);
    }
    let Some(t) = parse(text) else {
        let _ = writeln!(log.stamp(), "clock: {:?} is not a time", text);
        return false;
    };
    let before = rtc::now();
    let taken = rtc::set(&t);
    CHANGED.store(true, Ordering::Relaxed);
    let _ = write!(log.stamp(), "clock: {} {} {}", if taken { "set to" } else { "could NOT be set to" }, t.weekday_name(), t);
    match before {
        Some(b) => {
            let _ = writeln!(log, " (was {})", b);
        }
        None => {
            let _ = writeln!(log, " (was not set)");
        }
    }
    taken
}
