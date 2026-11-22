//! The watch face: three rings of sixty segments that fill up, all drawn
//! the same way - the outer one for the seconds, then the minutes, then
//! the hours (five segments to the hour) - and in
//! the middle the time in segment digits with the date, two small dials
//! (battery and heart rate) and a strip of bars.
//!
//! The face is a list of elements. Each has a rectangle (`rect`), a number
//! that stands for what it shows right now (`key`) and a routine that
//! draws it (`draw`). Whoever shows the face compares the keys with those
//! of the picture on the screen and repaints only the elements whose key
//! changed (see `services::damage`). Some elements move; for those the
//! rectangle depends on the state, and both the old and the new place
//! have to be repainted.
//!
//! Nothing in here depends on the watch: it draws into any
//! `embedded-graphics` target, which is how the preview pictures on the PC
//! are made from the same code.

use core::fmt::Write;

use embedded_graphics::mono_font::{MonoFont, MonoTextStyleBuilder};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use profont::{PROFONT_14_POINT, PROFONT_18_POINT};

/// The picture is `SIZE` x `SIZE`, round, centred on (`C`, `C`).
pub const SIZE: i32 = 454;
const C: i32 = SIZE / 2;

const BLACK: Rgb565 = Rgb565::BLACK;
const TEAL: Rgb565 = Rgb565::new(0, 56, 27);
const TEAL_MID: Rgb565 = Rgb565::new(0, 34, 17);
const TEAL_DIM: Rgb565 = Rgb565::new(0, 13, 7);
const TEAL_OFF: Rgb565 = Rgb565::new(0, 6, 3);
const ORANGE: Rgb565 = Rgb565::new(31, 36, 0);
const ORANGE_OFF: Rgb565 = Rgb565::new(5, 5, 0);
const RED: Rgb565 = Rgb565::new(31, 10, 4);

/// What the face shows.
#[derive(Clone, Copy)]
pub struct State {
    /// Local time; `None` if the clock is not set.
    pub time: Option<Time>,
    /// Milliseconds into the current second, 0 to 999.
    pub millis: u16,
    /// Milliseconds since some fixed moment; drives what merely spins.
    pub ticks: u32,
    /// Battery charge in percent and whether power is connected.
    pub battery: Option<(u8, bool)>,
    /// Heart rate in beats a minute, if a measurement has one.
    pub heart: Option<u16>,
    /// A heart-rate measurement is running.
    pub measuring: bool,
    pub bluetooth: bool,
    pub worn: Option<bool>,
    /// Temperature in tenths of a degree.
    pub temperature: Option<i16>,
}

#[derive(Clone, Copy)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    /// 1 = Monday ... 7 = Sunday.
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

// --- Elements -----------------------------------------------------------------

/// Sixty segments, one per second: those that have passed are lit, and
/// the current one fills up as the second goes by.
const FIRST_SECOND: usize = 0;
/// Sixty segments, one per minute.
const FIRST_MINUTE: usize = FIRST_SECOND + 60;
/// Sixty segments for the twelve hours, five to the hour, so the ring
/// also shows how far the hour has got; every fifth one, where an hour
/// begins, is deeper.
const FIRST_HOUR: usize = FIRST_MINUTE + 60;
const HOUR_TENS: usize = FIRST_HOUR + 60;
const HOUR_ONES: usize = HOUR_TENS + 1;
const MINUTE_TENS: usize = HOUR_ONES + 1;
const MINUTE_ONES: usize = MINUTE_TENS + 1;
const COLON: usize = MINUTE_ONES + 1;
const SECOND_TENS: usize = COLON + 1;
const SECOND_ONES: usize = SECOND_TENS + 1;
const DATE: usize = SECOND_ONES + 1;
const STATUS: usize = DATE + 1;
/// The dials' bodies stand still; what circles round them are elements
/// of their own, so that a turn repaints only a small mark.
const BATTERY_DIAL: usize = STATUS + 1;
const HEART_DIAL: usize = BATTERY_DIAL + 1;
const BATTERY_MARK: usize = HEART_DIAL + 1;
const FIRST_HEART_MARK: usize = BATTERY_MARK + 1;
const BARS: usize = FIRST_HEART_MARK + 3;
/// Number of elements.
pub const ELEMENTS: usize = BARS + 1;

/// The three rings: inner and outer radius. The hours are 20 wide, the
/// minutes half that and the seconds half of that again, packed against
/// the edge of the glass.
const SECOND_RING: (i32, i32) = (221, 226);
const MINUTE_RING: (i32, i32) = (206, 216);
const HOUR_RING: (i32, i32) = (181, 201);
/// Inner radius of the hour ring's segments that begin an hour.
const HOUR_MARK: i32 = 173;
/// A segment of the second or minute ring: from and to within its six
/// degrees, in tenths of a degree.
const SEGMENT: (i32, i32) = (8, 52);

// Large digits: hours and minutes.
const BIG: Digit = Digit { w: 46, h: 84, t: 9 };
const BIG_Y: i32 = 185;
const BIG_X: [i32; 4] = [103, 161, 247, 305];
// Small digits: seconds.
const SMALL: Digit = Digit { w: 22, h: 40, t: 5 };
const SMALL_Y: i32 = 282;
const SMALL_X: [i32; 2] = [202, 230];
// The two dials.
const DIAL_Y: i32 = 302;
const DIAL_X: [i32; 2] = [140, 314];
/// The marks circling the dials: inner and outer radius, and their length
/// in tenths of a degree. Short and far enough out that the rectangle
/// around a mark never reaches the number in the middle of the dial,
/// wherever on the circle the mark is: text is costly to draw again.
const MARK: (i32, i32) = (29, 32);
const MARK_LENGTH: i32 = 300;

/// Which kind of element `n` is, for timing the drawing: 0 = a ring
/// segment, 1 = a digit or the colon, 2 = a line of text, 3 = a dial,
/// 4 = a mark circling a dial, 5 = the bars.
pub fn kind(n: usize) -> usize {
    match n {
        FIRST_SECOND..HOUR_TENS => 0,
        HOUR_TENS..DATE => 1,
        DATE | STATUS => 2,
        BATTERY_DIAL | HEART_DIAL => 3,
        BATTERY_MARK..BARS => 4,
        _ => 5,
    }
}

/// True if two rectangles share at least one pixel. Asked thousands of
/// times a picture, so it is spelt out instead of built from the
/// library's `intersection`.
pub fn overlap(a: &Rectangle, b: &Rectangle) -> bool {
    a.top_left.x < b.top_left.x + b.size.width as i32
        && b.top_left.x < a.top_left.x + a.size.width as i32
        && a.top_left.y < b.top_left.y + b.size.height as i32
        && b.top_left.y < a.top_left.y + a.size.height as i32
}

/// The smallest rectangle that holds both `a` and `b`.
pub fn around(a: &Rectangle, b: &Rectangle) -> Rectangle {
    let left = a.top_left.x.min(b.top_left.x);
    let top = a.top_left.y.min(b.top_left.y);
    let right = (a.top_left.x + a.size.width as i32).max(b.top_left.x + b.size.width as i32);
    let bottom = (a.top_left.y + a.size.height as i32).max(b.top_left.y + b.size.height as i32);
    Rectangle::new(Point::new(left, top), Size::new((right - left) as u32, (bottom - top) as u32))
}

/// The whole picture.
pub fn whole() -> Rectangle {
    Rectangle::new(Point::zero(), Size::new(SIZE as u32, SIZE as u32))
}

/// How much of the current second's segment is filled, in tenths of a
/// degree (in steps of four).
fn second_fill(s: &State) -> i32 {
    (SEGMENT.1 - SEGMENT.0) * s.millis.min(999) as i32 / 1000 / 4 * 4 + 4
}

/// Where the mark circling the battery dial begins, in tenths of a degree.
fn battery_mark(s: &State) -> i32 {
    spin(s, 3000) as i32 * 20
}

/// Where heart mark `n` (0 to 2) begins; they circle the other way, and
/// faster while a measurement runs.
fn heart_mark(n: usize, s: &State) -> i32 {
    3600 - spin(s, if s.measuring { 1000 } else { 4000 }) as i32 * 20 + n as i32 * 1200
}

/// The rectangle element `n` covers while it shows `s`.
pub fn rect(n: usize, s: &State) -> Rectangle {
    match n {
        FIRST_SECOND..FIRST_MINUTE => {
            let k = (n - FIRST_SECOND) as i32;
            segment_box(C, C, SECOND_RING.0, SECOND_RING.1, k * 60 + SEGMENT.0, k * 60 + SEGMENT.1)
        }
        FIRST_MINUTE..FIRST_HOUR => {
            let k = (n - FIRST_MINUTE) as i32;
            segment_box(C, C, MINUTE_RING.0, MINUTE_RING.1, k * 60 + SEGMENT.0, k * 60 + SEGMENT.1)
        }
        FIRST_HOUR..HOUR_TENS => {
            let k = (n - FIRST_HOUR) as i32;
            segment_box(C, C, HOUR_MARK, HOUR_RING.1, k * 60 + SEGMENT.0, k * 60 + SEGMENT.1)
        }
        HOUR_TENS..=MINUTE_ONES => BIG.rect(BIG_X[n - HOUR_TENS], BIG_Y),
        COLON => Rectangle::new(Point::new(219, BIG_Y), Size::new(16, BIG.h as u32)),
        SECOND_TENS | SECOND_ONES => SMALL.rect(SMALL_X[n - SECOND_TENS], SMALL_Y),
        DATE => text_rect(152, &PROFONT_18_POINT, 14),
        STATUS => text_rect(128, &PROFONT_14_POINT, 18),
        BATTERY_DIAL | HEART_DIAL => Rectangle::with_center(Point::new(DIAL_X[n - BATTERY_DIAL], DIAL_Y), Size::new(53, 53)),
        BATTERY_MARK => segment_box(DIAL_X[0], DIAL_Y, MARK.0, MARK.1, battery_mark(s), battery_mark(s) + MARK_LENGTH),
        FIRST_HEART_MARK..BARS => {
            let from = heart_mark(n - FIRST_HEART_MARK, s);
            segment_box(DIAL_X[1], DIAL_Y, MARK.0, MARK.1, from, from + MARK_LENGTH)
        }
        _ => Rectangle::new(Point::new(170, 330), Size::new(114, 26)),
    }
}

/// A number that changes whenever what element `n` shows changes.
pub fn key(n: usize, s: &State) -> u32 {
    let t = s.time;
    let digit = |value: Option<u8>, tens: bool| value.map_or(10, |v| if tens { v / 10 } else { v % 10 }) as u32;
    // A ring's segment: 0 = still to come, 1 = passed, 2 = the current one.
    let ring = |index: usize, now: Option<u8>| match now {
        Some(now) if index as u8 == now => 2,
        Some(now) if (index as u8) < now => 1,
        _ => 0,
    };
    match n {
        FIRST_SECOND..FIRST_MINUTE => match ring(n - FIRST_SECOND, t.map(|t| t.second)) {
            // The current one changes as it fills.
            2 => 2 + second_fill(s) as u32,
            other => other,
        },
        FIRST_MINUTE..FIRST_HOUR => ring(n - FIRST_MINUTE, t.map(|t| t.minute)),
        // Five segments to the hour: twelve minutes each.
        FIRST_HOUR..HOUR_TENS => ring(n - FIRST_HOUR, t.map(|t| t.hour % 12 * 5 + t.minute / 12)),
        HOUR_TENS => digit(t.map(|t| t.hour), true),
        HOUR_ONES => digit(t.map(|t| t.hour), false),
        MINUTE_TENS => digit(t.map(|t| t.minute), true),
        MINUTE_ONES => digit(t.map(|t| t.minute), false),
        COLON => t.map_or(0, |_| (s.millis < 500) as u32),
        SECOND_TENS => digit(t.map(|t| t.second), true),
        SECOND_ONES => digit(t.map(|t| t.second), false),
        DATE => t.map_or(0, |t| (t.year as u32) << 16 | (t.month as u32) << 12 | (t.day as u32) << 4 | t.weekday as u32),
        STATUS => {
            (s.temperature.unwrap_or(i16::MIN) as u16 as u32) << 8
                | (s.bluetooth as u32) << 2
                | match s.worn {
                    Some(true) => 2,
                    Some(false) => 1,
                    None => 0,
                }
        }
        BATTERY_DIAL => s.battery.map_or(0x1FF, |(percent, power)| (percent as u32) << 1 | power as u32),
        HEART_DIAL => (s.heart.unwrap_or(0) as u32) << 1 | s.measuring as u32,
        BATTERY_MARK => battery_mark(s) as u32,
        FIRST_HEART_MARK..BARS => (heart_mark(n - FIRST_HEART_MARK, s) as u32) << 1 | s.measuring as u32,
        // The bars jump five times a second.
        _ => s.ticks / 200,
    }
}

/// Where something that turns once in `period` milliseconds stands, in
/// steps of two degrees (0 to 179).
fn spin(s: &State, period: u32) -> u32 {
    s.ticks % period * 180 / period
}

/// Draw element `n` as it shows `s`. Only pixels inside `clip` are worked
/// out; the background there must already be black.
pub fn draw<D: DrawTarget<Color = Rgb565>>(target: &mut D, clip: &Rectangle, n: usize, s: &State) {
    let t = s.time;
    let digit = |value: Option<u8>, tens: bool| value.map(|v| if tens { v / 10 } else { v % 10 });
    match n {
        FIRST_SECOND..FIRST_MINUTE => {
            let k = (n - FIRST_SECOND) as i32;
            let (from, to) = (k * 60 + SEGMENT.0, k * 60 + SEGMENT.1);
            let rest = if k % 5 == 0 { TEAL_DIM } else { TEAL_OFF };
            match key(n, s) {
                0 => ring_segment(target, clip, C, C, SECOND_RING.0, SECOND_RING.1, from, to, rest),
                1 => ring_segment(target, clip, C, C, SECOND_RING.0, SECOND_RING.1, from, to, TEAL),
                _ => {
                    // The current second: orange as far as it has got.
                    let filled = from + second_fill(s);
                    ring_segment(target, clip, C, C, SECOND_RING.0, SECOND_RING.1, from, filled.min(to), ORANGE);
                    ring_segment(target, clip, C, C, SECOND_RING.0, SECOND_RING.1, filled.min(to), to, rest);
                }
            }
        }
        FIRST_MINUTE..FIRST_HOUR => {
            let k = (n - FIRST_MINUTE) as i32;
            let color = match key(n, s) {
                2 => ORANGE,
                1 => TEAL,
                _ if k % 5 == 0 => TEAL_DIM,
                _ => TEAL_OFF,
            };
            ring_segment(target, clip, C, C, MINUTE_RING.0, MINUTE_RING.1, k * 60 + SEGMENT.0, k * 60 + SEGMENT.1, color);
        }
        FIRST_HOUR..HOUR_TENS => {
            let k = (n - FIRST_HOUR) as i32;
            let color = match key(n, s) {
                2 => ORANGE,
                1 => TEAL,
                _ if k % 5 == 0 => TEAL_DIM,
                _ => TEAL_OFF,
            };
            let inner = if k % 5 == 0 { HOUR_MARK } else { HOUR_RING.0 };
            ring_segment(target, clip, C, C, inner, HOUR_RING.1, k * 60 + SEGMENT.0, k * 60 + SEGMENT.1, color);
        }
        HOUR_TENS => BIG.draw(target, BIG_X[0], BIG_Y, digit(t.map(|t| t.hour), true), TEAL, TEAL_OFF),
        HOUR_ONES => BIG.draw(target, BIG_X[1], BIG_Y, digit(t.map(|t| t.hour), false), TEAL, TEAL_OFF),
        MINUTE_TENS => BIG.draw(target, BIG_X[2], BIG_Y, digit(t.map(|t| t.minute), true), TEAL, TEAL_OFF),
        MINUTE_ONES => BIG.draw(target, BIG_X[3], BIG_Y, digit(t.map(|t| t.minute), false), TEAL, TEAL_OFF),
        COLON => {
            let color = if key(n, s) == 1 { ORANGE } else { ORANGE_OFF };
            for y in [BIG_Y + 24, BIG_Y + 52] {
                let _ = target.fill_solid(&Rectangle::new(Point::new(222, y), Size::new(10, 10)), color);
            }
        }
        SECOND_TENS => SMALL.draw(target, SMALL_X[0], SMALL_Y, digit(t.map(|t| t.second), true), ORANGE, ORANGE_OFF),
        SECOND_ONES => SMALL.draw(target, SMALL_X[1], SMALL_Y, digit(t.map(|t| t.second), false), ORANGE, ORANGE_OFF),
        DATE => {
            let mut text = Label::new();
            match t {
                Some(t) => {
                    let day = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"].get(t.weekday.wrapping_sub(1) as usize).copied().unwrap_or("???");
                    let _ = write!(text, "{} {:02}.{:02}.{:04}", day, t.day, t.month, t.year);
                }
                None => {
                    let _ = write!(text, "NO TIME SIGNAL");
                }
            }
            centred(target, text.as_str(), 152, &PROFONT_18_POINT, TEAL);
        }
        STATUS => {
            let mut text = Label::new();
            let _ = write!(
                text,
                "{} : {}",
                if s.bluetooth { "LINK" } else { "SOLO" },
                match s.worn {
                    Some(true) => "ON",
                    Some(false) => "OFF",
                    None => "--",
                }
            );
            if let Some(temperature) = s.temperature {
                let _ = write!(text, " : {}.{}C", temperature / 10, (temperature % 10).abs());
            }
            centred(target, text.as_str(), 128, &PROFONT_14_POINT, ORANGE);
        }
        BATTERY_DIAL => {
            let (x, y) = (DIAL_X[0], DIAL_Y);
            let (percent, _) = s.battery.unwrap_or((0, false));
            let low = s.battery.is_some() && percent < 20;
            // A track, and the charge as an arc on it.
            ring_segment(target, clip, x, y, 19, 25, 0, 3600, TEAL_OFF);
            ring_segment(target, clip, x, y, 19, 25, 0, percent.min(100) as i32 * 36, if low { RED } else { TEAL });
            let mut text = Label::new();
            match s.battery {
                Some(_) => {
                    let _ = write!(text, "{}", percent.min(99));
                }
                None => {
                    let _ = write!(text, "--");
                }
            }
            // Text is costly to draw, so only where it is really asked for:
            // the marks circling outside touch the dial's rectangle at
            // its corners, never the number.
            if touches(clip, x, y) {
                centred_at(target, text.as_str(), x, y - 8, &PROFONT_14_POINT, if low { RED } else { TEAL });
            }
        }
        HEART_DIAL => {
            let (x, y) = (DIAL_X[1], DIAL_Y);
            ring_segment(target, clip, x, y, 19, 25, 0, 3600, if s.measuring { ORANGE } else { ORANGE_OFF });
            let mut text = Label::new();
            match s.heart {
                Some(bpm) if bpm != 0 => {
                    let _ = write!(text, "{}", bpm.min(999));
                }
                _ => {
                    let _ = write!(text, "--");
                }
            }
            if touches(clip, x, y) {
                centred_at(target, text.as_str(), x, y - 8, &PROFONT_14_POINT, ORANGE);
            }
        }
        BATTERY_MARK => {
            let from = battery_mark(s);
            ring_segment(target, clip, DIAL_X[0], DIAL_Y, MARK.0, MARK.1, from, from + MARK_LENGTH, ORANGE);
        }
        FIRST_HEART_MARK..BARS => {
            let from = heart_mark(n - FIRST_HEART_MARK, s);
            ring_segment(target, clip, DIAL_X[1], DIAL_Y, MARK.0, MARK.1, from, from + MARK_LENGTH, if s.measuring { ORANGE } else { TEAL_MID });
        }
        _ => {
            // Nineteen bars above and below a middle line, scrambled from
            // the time: teal up, orange down.
            let seed = key(n, s);
            for bar in 0..19u32 {
                let scrambled = (seed.wrapping_mul(2654435761).wrapping_add(bar.wrapping_mul(0x9E3779B1))).wrapping_mul(0x85EBCA6B) >> 24;
                let up = 2 + scrambled as i32 % 11;
                let down = 1 + (scrambled as i32 >> 4) % 9;
                let x = 170 + bar as i32 * 6;
                let _ = target.fill_solid(&Rectangle::new(Point::new(x, 343 - up), Size::new(4, up as u32)), TEAL);
                let _ = target.fill_solid(&Rectangle::new(Point::new(x, 345), Size::new(4, down as u32)), ORANGE);
            }
        }
    }
}

// --- Ring segments --------------------------------------------------------------

/// Sine of 0 to 90 degrees, times 16384.
const SINE: [i32; 91] = [0, 286, 572, 857, 1143, 1428, 1713, 1997, 2280, 2563, 2845, 3126, 3406, 3686, 3964, 4240, 4516, 4790, 5063, 5334, 5604, 5872, 6138, 6402, 6664, 6924, 7182, 7438, 7692, 7943, 8192, 8438, 8682, 8923, 9162, 9397, 9630, 9860, 10087, 10311, 10531, 10749, 10963, 11174, 11381, 11585, 11786, 11982, 12176, 12365, 12551, 12733, 12911, 13085, 13255, 13421, 13583, 13741, 13894, 14044, 14189, 14330, 14466, 14598, 14726, 14849, 14968, 15082, 15191, 15296, 15396, 15491, 15582, 15668, 15749, 15826, 15897, 15964, 16026, 16083, 16135, 16182, 16225, 16262, 16294, 16322, 16344, 16362, 16374, 16382, 16384];

/// Sine of an angle in tenths of a degree, times 16384.
fn sine(tenths: i32) -> i32 {
    let a = tenths.rem_euclid(3600);
    let (a, sign) = if a >= 1800 { (a - 1800, -1) } else { (a, 1) };
    let a = if a > 900 { 1800 - a } else { a };
    let (whole, part) = ((a / 10) as usize, a % 10);
    let low = SINE[whole];
    let high = SINE[(whole + 1).min(90)];
    sign * (low + (high - low) * part / 10)
}

/// The direction of an angle (tenths of a degree, clockwise from twelve
/// o'clock) as a vector times 16384.
fn direction(tenths: i32) -> (i32, i32) {
    (sine(tenths), -sine(tenths + 900))
}

/// The rectangle a ring segment covers: centre, inner and outer radius,
/// from and to in tenths of a degree clockwise from twelve o'clock.
fn segment_box(cx: i32, cy: i32, inner: i32, outer: i32, from: i32, to: i32) -> Rectangle {
    if to - from >= 3600 {
        return Rectangle::with_center(Point::new(cx, cy), Size::new(2 * outer as u32 + 3, 2 * outer as u32 + 3));
    }
    let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut reach = |angle: i32, radius: i32| {
        let (dx, dy) = direction(angle);
        let (x, y) = (cx + dx * radius / 16384, cy + dy * radius / 16384);
        left = left.min(x);
        right = right.max(x);
        top = top.min(y);
        bottom = bottom.max(y);
    };
    for angle in [from, to] {
        reach(angle, inner);
        reach(angle, outer);
    }
    // Where the arc passes straight up, right, down or left it reaches
    // furthest.
    let mut quarter = from.div_euclid(900) * 900 + 900;
    while quarter < to {
        reach(quarter, outer);
        quarter += 900;
    }
    Rectangle::new(Point::new(left - 2, top - 2), Size::new((right - left + 5) as u32, (bottom - top + 5) as u32))
}

/// Fill a segment of a ring: centre, inner and outer radius, from and to
/// in tenths of a degree clockwise from twelve o'clock. Only the pixels
/// inside `clip` are looked at, and of those only the ones each row has
/// inside the ring. It goes out as runs of pixels along each row, which
/// is the cheapest thing to hand to the frame.
///
/// The edges are softened: a pixel the edge passes through gets the
/// colour in proportion to how much of it lies inside, in sixteen steps,
/// on the black background. Without that, curved and slanted edges show
/// as stairs.
#[allow(clippy::too_many_arguments)]
fn ring_segment<D: DrawTarget<Color = Rgb565>>(target: &mut D, clip: &Rectangle, cx: i32, cy: i32, inner: i32, outer: i32, from: i32, to: i32, color: Rgb565) {
    if to <= from {
        return;
    }
    let area = segment_box(cx, cy, inner, outer, from, to).intersection(clip);
    if area.is_zero_sized() {
        return;
    }
    let full = to - from >= 3600;
    // A segment of up to half a turn lies between two edges; a longer one
    // is everything that is not in the remaining, shorter one.
    let wide = !full && to - from > 1800;
    let (first, second) = if wide { (direction(to), direction(from)) } else { (direction(from), direction(to)) };
    let (inner2, outer2) = (inner * inner, outer * outer);
    // The softened edge reaches one pixel beyond the ring on either side.
    let (far, near) = (outer + 1, (inner - 1).max(0));
    let (far2, near2) = (far * far, near * near);
    // Inside these the ring's own edges are too far away to matter.
    let (solid_from, solid_to) = ((inner + 1) * (inner + 1), (outer - 1).max(0) * (outer - 1).max(0));
    let shades = shades(color);
    let (x0, x1) = (area.top_left.x, area.top_left.x + area.size.width as i32 - 1);
    // How far the ring reaches along a row, and how wide its hole is there.
    // Both change by little from one row to the next, so after the first
    // row they are stepped along instead of worked out afresh.
    let (mut reach, mut hole) = (-1, -1);
    for y in area.rows() {
        let dy = y - cy;
        if dy * dy > far2 {
            continue;
        }
        // Where this row crosses the ring: from -reach to +reach, less
        // the hole from -hole to +hole in the middle.
        let limit = far2 - dy * dy;
        if reach < 0 {
            reach = root(limit);
        }
        while reach * reach > limit {
            reach -= 1;
        }
        while (reach + 1) * (reach + 1) <= limit {
            reach += 1;
        }
        let limit = near2 - dy * dy;
        if limit <= 0 {
            hole = 0;
        } else {
            if hole < 0 {
                hole = root_up(limit);
            }
            while hole > 0 && (hole - 1) * (hole - 1) >= limit {
                hole -= 1;
            }
            while hole * hole < limit {
                hole += 1;
            }
        }
        let pieces = if hole == 0 { [(-reach, reach), (1, 0)] } else { [(-reach, -hole), (hole, reach)] };
        for (from_dx, to_dx) in pieces {
            let (first_x, last_x) = ((cx + from_dx).max(x0), (cx + to_dx).min(x1));
            // A run of pixels that get the same shade: where it began and
            // which shade (0 = nothing, 16 = the full colour).
            let mut run: (i32, i32) = (first_x, 0);
            for x in first_x..=last_x + 1 {
                let dx = x - cx;
                let shade = if x > last_x {
                    0
                } else {
                    // How much of the pixel lies inside, by each edge, in
                    // sixteenths; the scarcest one counts.
                    let d2 = dx * dx + dy * dy;
                    let mut inside = 16;
                    if d2 > solid_to {
                        inside = inside.min(8 - (d2 - outer2) * 8 / outer.max(1));
                    }
                    if inner > 0 && d2 < solid_from {
                        inside = inside.min(8 + (d2 - inner2) * 8 / inner);
                    }
                    if !full {
                        let by_first = 8 + ((first.0 * dy - first.1 * dx) >> 10);
                        let by_second = 8 + ((dx * second.1 - dy * second.0) >> 10);
                        let between = by_first.min(by_second).clamp(0, 16);
                        inside = inside.min(if wide { 16 - between } else { between });
                    }
                    inside.clamp(0, 16)
                };
                if shade != run.1 {
                    if run.1 != 0 && x > run.0 {
                        let _ = target.fill_solid(&Rectangle::new(Point::new(run.0, y), Size::new((x - run.0) as u32, 1)), shades[run.1 as usize]);
                    }
                    run = (x, shade);
                }
            }
        }
    }
}

/// `color` in seventeen strengths, from nothing (black) to all of it.
fn shades(color: Rgb565) -> [Rgb565; 17] {
    let mut out = [Rgb565::BLACK; 17];
    for (step, shade) in out.iter_mut().enumerate() {
        let part = |channel: u8| (channel as u32 * step as u32 + 8) / 16;
        *shade = Rgb565::new(part(color.r()) as u8, part(color.g()) as u8, part(color.b()) as u8);
    }
    out
}

/// The square root of `v`, rounded down.
fn root(v: i32) -> i32 {
    let v = v.max(0) as u32;
    let (mut result, mut bit) = (0u32, 1u32 << 30);
    let mut rest = v;
    while bit > rest {
        bit >>= 2;
    }
    while bit != 0 {
        if rest >= result + bit {
            rest -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result as i32
}

/// The square root of `v`, rounded up.
fn root_up(v: i32) -> i32 {
    let r = root(v);
    if r * r < v {
        r + 1
    } else {
        r
    }
}

// --- Segment digits -------------------------------------------------------------

/// A seven-segment digit of width `w`, height `h` and stroke `t`. All of
/// it is rectangles, which is the cheapest thing to draw.
struct Digit {
    w: i32,
    h: i32,
    t: i32,
}

/// Which segments are lit for 0 to 9: bit 0 = top, then clockwise (top
/// right, bottom right, bottom, bottom left, top left), bit 6 = middle.
const SEGMENTS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];

impl Digit {
    fn rect(&self, x: i32, y: i32) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(self.w as u32, self.h as u32))
    }

    /// The rectangle of segment `n` for a digit at (x, y).
    fn segment(&self, n: usize, x: i32, y: i32) -> Rectangle {
        let (w, h, t) = (self.w, self.h, self.t);
        let gap = 2;
        let mid = y + (h - t) / 2;
        let across = |top: i32| Rectangle::new(Point::new(x + t + gap, top), Size::new((w - 2 * t - 2 * gap) as u32, t as u32));
        let down = |left: i32, top: i32, bottom: i32| Rectangle::new(Point::new(left, top + gap), Size::new(t as u32, (bottom - top - 2 * gap) as u32));
        match n {
            0 => across(y),
            1 => down(x + w - t, y + t / 2, mid + t / 2),
            2 => down(x + w - t, mid + t / 2, y + h - t / 2),
            3 => across(y + h - t),
            4 => down(x, mid + t / 2, y + h - t / 2),
            5 => down(x, y + t / 2, mid + t / 2),
            _ => across(mid),
        }
    }

    /// Draw `value` (or, for `None`, only the middle bar) at (x, y); the
    /// segments that are not lit are drawn in `off`.
    fn draw<D: DrawTarget<Color = Rgb565>>(&self, target: &mut D, x: i32, y: i32, value: Option<u8>, on: Rgb565, off: Rgb565) {
        let lit = value.map_or(0x40, |v| SEGMENTS[(v % 10) as usize]);
        for n in 0..7 {
            let _ = target.fill_solid(&self.segment(n, x, y), if lit & 1 << n != 0 { on } else { off });
        }
    }
}

// --- Text -----------------------------------------------------------------------

/// True if `clip` reaches into the place of a dial's number, a field of
/// 34 x 20 pixels around (x, y).
fn touches(clip: &Rectangle, x: i32, y: i32) -> bool {
    overlap(&Rectangle::with_center(Point::new(x, y), Size::new(34, 20)), clip)
}

/// The rectangle a centred line of `chars` characters at height `y` covers.
fn text_rect(y: i32, font: &MonoFont<'_>, chars: u32) -> Rectangle {
    let w = (font.character_size.width + font.character_spacing) * chars;
    Rectangle::new(Point::new(C - w as i32 / 2 - 1, y), Size::new(w + 2, font.character_size.height))
}

fn centred<D: DrawTarget<Color = Rgb565>>(target: &mut D, text: &str, y: i32, font: &MonoFont<'_>, color: Rgb565) {
    centred_at(target, text, C, y, font, color);
}

/// Draw `text` centred on `x` with its top at `y`.
fn centred_at<D: DrawTarget<Color = Rgb565>>(target: &mut D, text: &str, x: i32, y: i32, font: &MonoFont<'_>, color: Rgb565) {
    let position = TextStyleBuilder::new().alignment(Alignment::Center).baseline(Baseline::Top).build();
    let style = MonoTextStyleBuilder::new().font(font).text_color(color).background_color(BLACK).build();
    let _ = Text::with_text_style(text, Point::new(x, y), style, position).draw(target);
}

/// A short piece of text put together with `write!`.
struct Label {
    text: [u8; 24],
    len: usize,
}

impl Label {
    fn new() -> Self {
        Self { text: [0; 24], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Label {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.text.len() && b.is_ascii() {
                self.text[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}
