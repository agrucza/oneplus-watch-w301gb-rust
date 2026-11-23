//! The watch face, after the "Nightwatch OS" design (round kit, screen 01):
//! sixty ticks around the rim that light up with the seconds and a dot
//! that goes round with them, the hours in red over the minutes in white,
//! a date tag above them, an arc for the battery on the left and one for
//! the steps on the right, two pills below the time, and a line of text
//! bent along the top and the bottom.
//!
//! The design's small text is 9 to 12 pixels, which is under a millimetre
//! on this panel; labels and values are about 18 pixels here. Its scan
//! lines and vignette over the whole picture are left out. What the
//! firmware has no data for yet (steps, unread messages) is shown as
//! dashes, dimmed.
//!
//! The face is a list of elements. Each has a rectangle (`rect`), a number
//! that stands for what it shows right now (`key`) and a routine that
//! draws it (`draw`). Whoever shows the face compares the keys with those
//! of the picture on the screen and repaints only the elements whose key
//! changed (see `services::damage`). The second dot moves; for it the
//! rectangle depends on the state, and both the old and the new place
//! have to be repainted. Elements are drawn in the order of their
//! numbers, so the glow of a digit lies under every digit.
//!
//! Nothing in here depends on the watch: it draws into any
//! `embedded-graphics` target, which is how the preview pictures on the PC
//! are made from the same code.

use core::fmt::Write;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, Triangle};

use crate::services::font::Paint;
use crate::services::font_data::{
    NW_HEART, NW_HERO, NW_HERO_GLOW, NW_HERO_GLOW_PAD, NW_LABEL, NW_MESSAGE, NW_MONO, NW_RIM_BOTTOM, NW_RIM_BOTTOM_AT, NW_RIM_TOP, NW_RIM_TOP_AT,
};

pub use super::theme::{whole, SIZE};
use super::theme::{self, rgb, BONE, C, CHROME, CYAN, INK, SIGNAL, SIGNAL_HOT, STEEL, STEEL_2};

/// The minutes' soft white glow: the design's 25 % of the text colour.
const BONE_GLOW: Rgb565 = rgb(0x3A3B3C);

/// What the face shows. Not every face uses all of it.
#[derive(Clone, Copy)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Sixty ticks, one per second: those up to the current second are lit.
const FIRST_TICK: usize = 0;
/// The dot that goes round the rim with the seconds.
const SECOND_DOT: usize = FIRST_TICK + 60;
/// The red triangle at twelve o'clock.
const MARKER: usize = SECOND_DOT + 1;
const RIM_TOP: usize = MARKER + 1;
const RIM_BOTTOM: usize = RIM_TOP + 1;
const BATTERY_ARC: usize = RIM_BOTTOM + 1;
const STEP_ARC: usize = BATTERY_ARC + 1;
const BATTERY_TEXT: usize = STEP_ARC + 1;
const STEP_TEXT: usize = BATTERY_TEXT + 1;
/// The glows of the four big digits, before everything they shine under.
const FIRST_GLOW: usize = STEP_TEXT + 1;
const DATE: usize = FIRST_GLOW + 4;
/// The big digits: hours' tens and ones, minutes' tens and ones.
const FIRST_DIGIT: usize = DATE + 1;
const HEART_PILL: usize = FIRST_DIGIT + 4;
const MESSAGE_PILL: usize = HEART_PILL + 1;
/// How many elements the face has.
pub const ELEMENTS: usize = MESSAGE_PILL + 1;

// Where things are. Angles are tenths of a degree, clockwise from twelve.
/// Ticks: outer radius, inner radius of every fifth and of the others,
/// and half their widths as angles.
const TICK_OUTER: i32 = 217;
const TICK_INNER: (i32, i32) = (207, 210);
const TICK_HALF: (i32, i32) = (4, 2);
/// The second dot's distance from the middle and its radius.
const DOT: (i32, i32) = (221, 4);
/// The two arcs: inner and outer radius; the battery's from bottom left
/// up to top left, the steps' from top right down to bottom right.
const ARC: (i32, i32) = (176, 184);
const BATTERY_SPAN: (i32, i32) = (2150, 3250);
const STEP_SPAN: (i32, i32) = (350, 1450);
/// A dash of an arc and the gap after it (the design's 4 and 2 pixels).
const DASH: (i32, i32) = (13, 6);
/// The two small blocks of text beside the time: left edge of the left
/// one, right edge of the right one, top of the label, top of the value.
const SIDE_X: (i32, i32) = (60, 394);
const SIDE_Y: (i32, i32) = (201, 227);
const SIDE_WIDTH: i32 = 50;
/// The top of the date tag.
const TAG_Y: i32 = 95;
/// The big digits: left edges, and the tops of the hours and the minutes.
const DIGIT_X: [i32; 2] = [C - NW_HERO.width as i32 + 1, C - 1];
const DIGIT_Y: [i32; 2] = [133, 220];
/// The pills: left edges, top, width, height, and the cut of their corners.
const PILL_X: [i32; 2] = [149, 231];
const PILL: (i32, i32, i32, i32) = (309, 74, 30, 7);

/// Which kind of element `n` is, for timing the drawing: 0 = a ring
/// segment (ticks, dot, arcs), 1 = a big digit or its glow, 2 = text,
/// 3 = a pill; 4 and 5 are not used by this face.
pub fn kind(n: usize) -> usize {
    match n {
        FIRST_TICK..RIM_TOP | BATTERY_ARC | STEP_ARC => 0,
        n if (FIRST_GLOW..HEART_PILL).contains(&n) && n != DATE => 1,
        HEART_PILL | MESSAGE_PILL => 3,
        _ => 2,
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

/// A tick's inner radius and half width.
fn tick(k: i32) -> (i32, i32) {
    if k % 5 == 0 { (TICK_INNER.0, TICK_HALF.0) } else { (TICK_INNER.1, TICK_HALF.1) }
}

/// The middle of the second dot.
fn dot_at(s: &State) -> Point {
    let second = s.time.map_or(0, |t| t.second as i32);
    let (dx, dy) = direction(second * 60);
    Point::new(C + dx * DOT.0 / 16384, C + dy * DOT.0 / 16384)
}

/// Which of the four big digits `n` (0 to 3) shows, if the time is known.
fn digit(n: usize, s: &State) -> Option<u8> {
    let t = s.time?;
    let value = if n < 2 { t.hour } else { t.minute };
    Some(if n % 2 == 0 { value / 10 } else { value % 10 })
}

/// The top left corner of big digit `n` (0 to 3).
fn digit_at(n: usize) -> Point {
    Point::new(DIGIT_X[n % 2], DIGIT_Y[n / 2])
}

/// The date as the tag shows it, e.g. "TUE 24 NOV".
fn date_text(s: &State) -> Label {
    const DAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
    const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    let mut label = Label::new();
    match s.time {
        Some(t) => {
            let day = DAYS[(t.weekday as usize + 6) % 7];
            let month = MONTHS[(t.month as usize + 11) % 12];
            let _ = write!(label, "{} {:02} {}", day, t.day, month);
        }
        None => {
            let _ = write!(label, "NO CLOCK");
        }
    }
    label
}

/// The rectangle element `n` covers while it shows `s`.
pub fn rect(n: usize, s: &State) -> Rectangle {
    match n {
        FIRST_TICK..SECOND_DOT => {
            let k = (n - FIRST_TICK) as i32;
            let (inner, half) = tick(k);
            segment_box(C, C, inner, TICK_OUTER, k * 60 - half, k * 60 + half)
        }
        SECOND_DOT => Rectangle::with_center(dot_at(s), Size::new(2 * DOT.1 as u32 + 5, 2 * DOT.1 as u32 + 5)).intersection(&whole()),
        MARKER => Rectangle::new(Point::new(C - 8, 3), Size::new(17, 16)),
        RIM_TOP => NW_RIM_TOP.rect(NW_RIM_TOP_AT.0, NW_RIM_TOP_AT.1),
        RIM_BOTTOM => NW_RIM_BOTTOM.rect(NW_RIM_BOTTOM_AT.0, NW_RIM_BOTTOM_AT.1),
        BATTERY_ARC => segment_box(C, C, ARC.0, ARC.1, BATTERY_SPAN.0, BATTERY_SPAN.1),
        STEP_ARC => segment_box(C, C, ARC.0, ARC.1, STEP_SPAN.0, STEP_SPAN.1),
        BATTERY_TEXT => Rectangle::new(Point::new(SIDE_X.0, SIDE_Y.0), Size::new(SIDE_WIDTH as u32, (SIDE_Y.1 - SIDE_Y.0) as u32 + NW_MONO.height as u32)),
        STEP_TEXT => Rectangle::new(Point::new(SIDE_X.1 - SIDE_WIDTH, SIDE_Y.0), Size::new(SIDE_WIDTH as u32, (SIDE_Y.1 - SIDE_Y.0) as u32 + NW_MONO.height as u32)),
        FIRST_GLOW..DATE => NW_HERO_GLOW.rect(digit_at(n - FIRST_GLOW).x - NW_HERO_GLOW_PAD, digit_at(n - FIRST_GLOW).y - NW_HERO_GLOW_PAD, 1),
        // As wide as the widest date, so that a shorter one leaves nothing behind.
        DATE => theme::tag_rect(C, TAG_Y, "WED 00 MAY"),
        FIRST_DIGIT..HEART_PILL => NW_HERO.rect(digit_at(n - FIRST_DIGIT).x, digit_at(n - FIRST_DIGIT).y, 1),
        _ => Rectangle::new(Point::new(PILL_X[n - HEART_PILL], PILL.0), Size::new(PILL.1 as u32, PILL.2 as u32)),
    }
}

/// A number that changes exactly when what element `n` shows changes.
pub fn key(n: usize, s: &State) -> u32 {
    match n {
        FIRST_TICK..SECOND_DOT => s.time.is_some_and(|t| (n - FIRST_TICK) as u8 <= t.second) as u32,
        SECOND_DOT => s.time.map_or(60, |t| t.second as u32),
        BATTERY_ARC | BATTERY_TEXT => s.battery.map_or(u32::MAX, |(percent, _)| percent as u32),
        FIRST_GLOW..DATE => digit(n - FIRST_GLOW, s).map_or(10, |d| d as u32),
        DATE => s.time.map_or(0, |t| (t.weekday as u32) << 16 | (t.month as u32) << 8 | t.day as u32),
        FIRST_DIGIT..HEART_PILL => digit(n - FIRST_DIGIT, s).map_or(10, |d| d as u32),
        HEART_PILL => s.heart.map_or(u32::MAX, |bpm| bpm as u32),
        // The marker, the rim texts, and what has no data yet.
        _ => 0,
    }
}

/// Draw element `n` as it shows `s`. Only pixels inside `clip` are worked
/// out; the background there must already be black.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, n: usize, s: &State) {
    match n {
        FIRST_TICK..SECOND_DOT => {
            let k = (n - FIRST_TICK) as i32;
            let (inner, half) = tick(k);
            let color = match (key(n, s), k % 5) {
                (0, _) => STEEL,
                (_, 0) => SIGNAL_HOT,
                _ => SIGNAL,
            };
            ring_segment(target, clip, C, C, inner, TICK_OUTER, k * 60 - half, k * 60 + half, color);
        }
        SECOND_DOT => {
            if s.time.is_some() {
                let at = dot_at(s);
                ring_segment(target, clip, at.x, at.y, 0, DOT.1, 0, 3600, SIGNAL_HOT);
            }
        }
        MARKER => {
            let triangle = Triangle::new(Point::new(C, 4), Point::new(C + 8, 18), Point::new(C - 8, 18));
            let _ = triangle.into_styled(PrimitiveStyle::with_fill(SIGNAL)).draw(&mut target.clipped(clip));
        }
        RIM_TOP => target.art(clip, NW_RIM_TOP_AT.0, NW_RIM_TOP_AT.1, &NW_RIM_TOP, CYAN),
        RIM_BOTTOM => target.art(clip, NW_RIM_BOTTOM_AT.0, NW_RIM_BOTTOM_AT.1, &NW_RIM_BOTTOM, CHROME),
        BATTERY_ARC => {
            ring_segment(target, clip, C, C, ARC.0, ARC.1, BATTERY_SPAN.0, BATTERY_SPAN.1, INK);
            if let Some((percent, _)) = s.battery {
                let to = BATTERY_SPAN.0 + (BATTERY_SPAN.1 - BATTERY_SPAN.0) * percent.min(100) as i32 / 100;
                dashes(target, clip, BATTERY_SPAN.0, to, CYAN);
            }
        }
        STEP_ARC => {
            // No step count yet: the track only.
            ring_segment(target, clip, C, C, ARC.0, ARC.1, STEP_SPAN.0, STEP_SPAN.1, INK);
        }
        BATTERY_TEXT => {
            target.text(clip, SIDE_X.0, SIDE_Y.0, &NW_LABEL, "BAT", CYAN);
            let mut value = Label::new();
            match s.battery {
                Some((percent, _)) => {
                    let _ = write!(value, "{}%", percent);
                }
                None => {
                    let _ = write!(value, "--%");
                }
            }
            target.text(clip, SIDE_X.0, SIDE_Y.1, &NW_MONO, value.as_str(), BONE);
        }
        STEP_TEXT => {
            // Right-aligned; the label's last letter carries its spacing.
            let label = "STEP";
            target.text(clip, SIDE_X.1 - NW_LABEL.measure(label) + 3, SIDE_Y.0, &NW_LABEL, label, STEEL_2);
            let value = "----";
            target.text(clip, SIDE_X.1 - NW_MONO.measure(value), SIDE_Y.1, &NW_MONO, value, STEEL_2);
        }
        FIRST_GLOW..DATE => {
            let i = n - FIRST_GLOW;
            if let Some(d) = digit(i, s) {
                let at = digit_at(i);
                let color = if i < 2 { SIGNAL } else { BONE_GLOW };
                target.glyph(clip, at.x - NW_HERO_GLOW_PAD, at.y - NW_HERO_GLOW_PAD, &NW_HERO_GLOW, b'0' + d, color);
            }
        }
        DATE => {
            theme::tag(target, clip, C, TAG_Y, date_text(s).as_str(), CYAN);
        }
        FIRST_DIGIT..HEART_PILL => {
            let i = n - FIRST_DIGIT;
            if let Some(d) = digit(i, s) {
                let at = digit_at(i);
                target.glyph(clip, at.x, at.y, &NW_HERO, b'0' + d, if i < 2 { SIGNAL } else { BONE });
            }
        }
        HEART_PILL => {
            let mut value = Label::new();
            match s.heart {
                Some(bpm) => {
                    let _ = write!(value, "{:03}", bpm.min(999));
                }
                None => {
                    let _ = write!(value, "---");
                }
            }
            pill(target, clip, PILL_X[0], &NW_HEART, value.as_str(), SIGNAL);
        }
        _ => {
            // No count of unread messages yet.
            pill(target, clip, PILL_X[1], &NW_MESSAGE, "x--", STEEL_2);
        }
    }
}

/// The lit part of an arc, from `from` to `to`, in dashes.
fn dashes<D: Paint>(target: &mut D, clip: &Rectangle, from: i32, to: i32, color: Rgb565) {
    let mut at = from;
    while at < to {
        ring_segment(target, clip, C, C, ARC.0, ARC.1, at, (at + DASH.0).min(to), color);
        at += DASH.0 + DASH.1;
    }
}

/// A pill: a thin outline with two corners cut, an icon and a value.
fn pill<D: Paint>(target: &mut D, clip: &Rectangle, x: i32, icon: &crate::services::font::Art, value: &str, color: Rgb565) {
    let (y, w, h, cut) = PILL;
    theme::frame(target, clip, &Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32)), cut, color);
    let inner = icon.width as i32 + 6 + NW_MONO.measure(value);
    let left = x + (w - inner) / 2;
    target.art(clip, left, y + (h - icon.height as i32) / 2, icon, color);
    target.text(clip, left + icon.width as i32 + 6, y + (h - NW_MONO.height as i32) / 2, &NW_MONO, value, color);
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

// --- Text -----------------------------------------------------------------------

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
