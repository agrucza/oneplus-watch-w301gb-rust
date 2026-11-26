//! The compass page: a dial that turns under a fixed mark, and the
//! heading as a number.
//!
//! The chip (MMC5603, behind the Apollo) is switched on while this page
//! is on a lit screen and off again when it is left (the main task does
//! that, told through `shared`). The Apollo's report carries the heading
//! in degrees ready made. The page lets the dial follow it with a short
//! delay.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, Triangle};

use super::face::{direction, ring_segment};
use super::theme::{self, BONE, C, CHROME, CYAN, SIGNAL, STEEL_2};
use crate::services::font::Paint;
use crate::services::font_data::{NW_MONO, NW_NUMBER, NW_TITLE};

// Angles are tenths of a degree, clockwise from twelve o'clock.
/// The dial's ticks: outer radius, inner radius of those every 30
/// degrees and of the others, and half their widths as angles.
const TICK_OUTER: i32 = 212;
const TICK_INNER: (i32, i32) = (192, 202);
const TICK_HALF: (i32, i32) = (5, 2);
/// The four letters' distance from the middle.
const LETTERS: i32 = 166;

/// What the page shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct View {
    /// The heading in whole degrees, 0 to 359.
    pub degrees: u16,
    /// The chip's driver calls itself calibrated.
    pub calibrated: bool,
}

/// The shown heading after `passed_ms` more, on its way from `shown` to
/// `reported`: it follows with a delay of about a third of a second, the
/// short way round, so that the dial does not jump with every twitch of
/// the arm.
pub fn follow(shown: f32, reported: f32, passed_ms: u32) -> f32 {
    let mut apart = reported - shown;
    if apart > 180.0 {
        apart -= 360.0;
    } else if apart < -180.0 {
        apart += 360.0;
    }
    let share = (passed_ms as f32 / 300.0).min(1.0);
    let mut next = shown + apart * share;
    if next < 0.0 {
        next += 360.0;
    } else if next >= 360.0 {
        next -= 360.0;
    }
    next
}

/// A heading in whole degrees, 0 to 359.
pub fn whole_degrees(heading: f32) -> u16 {
    ((heading + 0.5) as u32 % 360) as u16
}

/// The name of the direction `degrees` points in, of sixteen.
pub fn name(degrees: u16) -> &'static str {
    const NAMES: [&str; 16] = ["N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW"];
    // Each name covers 22.5 degrees, half of them to either side.
    NAMES[((degrees as u32 * 2 + 22) / 45 % 16) as usize]
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, view: Option<View>) {
    theme::clear(target, clip);
    let Some(view) = view else {
        theme::title(target, clip, "COMPASS", CYAN);
        theme::centred(target, clip, 200, &NW_MONO, "// WAITING FOR THE COMPASS", BONE);
        return;
    };
    let heading = view.degrees;
    // The dial: north stands where the heading puts it, the watch's
    // twelve o'clock being the way it points.
    let north = -(heading as i32) * 10;
    for k in 0..72 {
        let angle = north + k * 50;
        let (inner, half) = if k % 6 == 0 { (TICK_INNER.0, TICK_HALF.0) } else { (TICK_INNER.1, TICK_HALF.1) };
        let color = if k == 0 { SIGNAL } else if k % 6 == 0 { BONE } else { STEEL_2 };
        ring_segment(target, clip, C, C, inner, TICK_OUTER, angle - half, angle + half, color);
    }
    for (n, letter) in ["N", "E", "S", "W"].iter().enumerate() {
        let (dx, dy) = direction(north + n as i32 * 900);
        let (x, y) = (C + dx * LETTERS / 16384, C + dy * LETTERS / 16384);
        let color: Rgb565 = if n == 0 { SIGNAL } else { BONE };
        // The title font's letters carry their spacing on the right.
        target.text(clip, x - (NW_TITLE.measure(letter) - 3) / 2, y - NW_TITLE.height as i32 / 2, &NW_TITLE, letter, color);
    }
    // The mark the dial turns under.
    let mark = Triangle::new(Point::new(C, 20), Point::new(C + 9, 4), Point::new(C - 9, 4));
    let _ = mark.into_styled(PrimitiveStyle::with_fill(CYAN)).draw(&mut target.clipped(clip));

    // The middle: the heading, its name, and a word about how far the
    // chip is to be trusted.
    let digit = NW_NUMBER.width as i32;
    for (n, value) in [heading / 100, heading / 10 % 10, heading % 10].into_iter().enumerate() {
        target.glyph(clip, C - 3 * digit / 2 + n as i32 * digit, C - 44, &NW_NUMBER, b'0' + value as u8, BONE);
    }
    let name = name(heading);
    target.text(clip, C - (NW_TITLE.measure(name) - 3) / 2, C, &NW_TITLE, name, CYAN);
    let hint = if view.calibrated { "" } else { "NOT CALIBRATED" };
    theme::centred(target, clip, C + 44, &NW_MONO, hint, CHROME);
}
