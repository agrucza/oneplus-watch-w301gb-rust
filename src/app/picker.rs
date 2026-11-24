//! A page to set one time on: wheels to turn with the finger, hours and
//! minutes or hours, minutes and seconds, and "cancel" and "save" under
//! them.
//!
//! A wheel is taken hold of where the finger comes down and follows it
//! from the first pixel. Let go, it runs out to a value and rests there:
//! the nearest one, or after a flick one further on. A tap above or
//! below the middle row steps by one. Whoever opens the page says what
//! it is for (the title) and gets the time back on "save".
//!
//! The page takes the finger's places as they come from the touch
//! controller (`touch`), not the gestures made of them: a gesture only
//! begins after 15 pixels, which is a quarter of the way to the next
//! value. Taps still come as gestures (`tap`).

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BLACK, BONE, C, CYAN, SIGNAL, STEEL, STEEL_2};
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_NUMBER};

/// The distance from one value of a wheel to the next.
pub const PITCH: i32 = 54;
/// The top of the three rows a wheel shows; the middle one holds the value.
const TOP: i32 = 96;
const MIDDLE: i32 = TOP + PITCH;
/// The middles of the wheels when there are two and when there are
/// three, and half a wheel's width.
const WHEEL_X: [[i32; 3]; 2] = [[C - 72, C + 72, 0], [C - 104, C, C + 104]];
const HALF: i32 = 46;
/// How many values each wheel has: hours, minutes, seconds.
const RANGE: [i32; 3] = [24, 60, 60];
/// How far a flick carries: the finger's speed is taken to last this long.
const FLICK_MS: i32 = 150;
/// The finger's speed is taken over this long before it lifts.
const SPEED_MS: u64 = 90;
/// A touch that moved less than this is a tap and turns nothing (the
/// same distance at which a gesture becomes a drag).
const TAP: i32 = 15;
/// How long a wheel takes to run out: at least and at most, and the
/// milliseconds added for every pixel it has to go.
const RUN_MS: (u64, u64) = (140, 650);
const RUN_MS_PER_PIXEL: u64 = 2;
/// The two buttons: top, height, width, and their left edges.
const BUTTON: (i32, i32, i32) = (292, 44, 146);
const BUTTON_X: [i32; 2] = [C - 8 - 146, C + 8];

pub struct Picker {
    pub title: &'static str,
    /// How many wheels there are: two (hours, minutes) or three (and
    /// seconds).
    wheels: usize,
    /// Hour, minute and second as they stand.
    value: [i32; 3],
    /// The wheel under the finger.
    hold: Option<Hold>,
    /// A wheel running out after it was let go.
    run: Option<Run>,
}

struct Hold {
    wheel: usize,
    /// Where the finger came down, how far below that it is now, and the
    /// furthest it has been from there.
    start_y: i32,
    moved: i32,
    furthest: i32,
    /// The finger's latest places, oldest first, for its speed.
    samples: [(u64, i32); 4],
}

struct Run {
    wheel: usize,
    /// How far below its value the wheel stood when it was let go, when
    /// that was, how long the way back takes, and where it stands now.
    from: i32,
    began: u64,
    lasts: u64,
    now: i32,
}

/// What a tap did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Nothing,
    /// A wheel stepped: the wheels have to be drawn again.
    Stepped,
    Cancel,
    /// "Save", with the time in seconds since midnight.
    Save(u32),
}

impl Picker {
    /// The page for setting a time of day in hours and minutes, which
    /// now is `minutes` since midnight.
    pub fn new(title: &'static str, minutes: u16) -> Self {
        Self { wheels: 2, ..Self::with_seconds(title, minutes as u32 * 60) }
    }

    /// The page for setting a time in hours, minutes and seconds (a time
    /// of day, or a length of time under a day), which now is `seconds`.
    /// Nothing opens it yet: no page of the firmware has a time with
    /// seconds to set.
    #[allow(dead_code)]
    pub fn with_seconds(title: &'static str, seconds: u32) -> Self {
        let value = [(seconds / 3600 % 24) as i32, (seconds / 60 % 60) as i32, (seconds % 60) as i32];
        Self { title, wheels: 3, value, hold: None, run: None }
    }

    /// The time the wheels rest on, in seconds.
    pub fn seconds(&self) -> u32 {
        (self.value[0] * 3600 + self.value[1] * 60 + if self.wheels == 3 { self.value[2] } else { 0 }) as u32
    }

    /// The window wheel `n` shows through (0 hours, 1 minutes, 2 seconds).
    pub fn wheel_rect(&self, n: usize) -> Rectangle {
        Rectangle::new(Point::new(WHEEL_X[self.wheels - 2][n] - HALF, TOP), Size::new(2 * HALF as u32, 3 * PITCH as u32))
    }

    /// The wheel that shows at (x, y), if any.
    fn wheel_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.wheels).find(|&n| self.wheel_rect(n).contains(Point::new(x, y)))
    }

    /// Where the finger is at `now_ms` (`None`: it lifted). True if a
    /// wheel has moved and is to be drawn again.
    pub fn touch(&mut self, at: Option<(i32, i32)>, now_ms: u64) -> bool {
        match (at, self.hold.as_mut()) {
            (Some((x, y)), None) => {
                let Some(wheel) = self.wheel_at(x, y) else {
                    return false;
                };
                // A wheel caught while it runs out stops where it was going.
                self.run = None;
                self.hold = Some(Hold { wheel, start_y: y, moved: 0, furthest: 0, samples: [(now_ms, y); 4] });
                true
            }
            (Some((_, y)), Some(hold)) => {
                let moved = y - hold.start_y;
                let changed = moved != hold.moved;
                hold.moved = moved;
                hold.furthest = hold.furthest.max(moved.abs());
                hold.samples.rotate_left(1);
                hold.samples[3] = (now_ms, y);
                changed
            }
            (None, Some(hold)) => {
                let (wheel, moved) = (hold.wheel, hold.moved);
                // The speed over the last moment before lifting, in
                // pixels a second downwards.
                let (last_at, last_y) = hold.samples[3];
                let (first_at, first_y) = hold.samples.iter().copied().find(|&(at, _)| last_at - at <= SPEED_MS).unwrap_or((last_at, last_y));
                let speed = if last_at > first_at { (last_y - first_y) * 1000 / (last_at - first_at) as i32 } else { 0 };
                // A tap turns nothing; what it did to the picture goes back.
                let steps = if hold.furthest < TAP { 0 } else { nearest(moved + speed * FLICK_MS / 1000) };
                self.hold = None;
                // Moving the wheel down brings the smaller values to the middle.
                self.value[wheel] = (self.value[wheel] - steps).rem_euclid(RANGE[wheel]);
                let from = moved - steps * PITCH;
                if from != 0 {
                    let lasts = (RUN_MS.0 + from.unsigned_abs() as u64 * RUN_MS_PER_PIXEL).min(RUN_MS.1);
                    self.run = Some(Run { wheel, from, began: now_ms, lasts, now: from });
                }
                true
            }
            (None, None) => false,
        }
    }

    /// Carry a wheel that is running out on to where it is at `now_ms`.
    /// True if it has moved and is to be drawn again.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        let Some(run) = self.run.as_mut() else {
            return false;
        };
        let gone = now_ms.saturating_sub(run.began);
        if gone >= run.lasts {
            self.run = None;
            return true;
        }
        // Fast at first, slower and slower towards the value: what is
        // left of the way is the cube of what is left of the time.
        let left = (run.lasts - gone) as i64;
        let lasts = run.lasts as i64;
        let now = (run.from as i64 * left * left * left / (lasts * lasts * lasts)) as i32;
        let changed = now != run.now;
        run.now = now;
        changed
    }

    /// A tap at (x, y).
    pub fn tap(&mut self, x: i32, y: i32) -> Outcome {
        let at = Point::new(x, y);
        if button_rect(0).contains(at) {
            return Outcome::Cancel;
        }
        if button_rect(1).contains(at) {
            return Outcome::Save(self.seconds());
        }
        let Some(n) = self.wheel_at(x, y) else {
            return Outcome::Nothing;
        };
        // The value above the middle comes down, the one below comes up.
        self.run = None;
        let step = if y < MIDDLE { -1 } else if y >= MIDDLE + PITCH { 1 } else { return Outcome::Nothing };
        self.value[n] = (self.value[n] + step).rem_euclid(RANGE[n]);
        Outcome::Stepped
    }

    /// For wheel `n`: the value nearest the middle right now and how far
    /// below the middle it stands, in pixels.
    fn shown(&self, n: usize) -> (i32, i32) {
        let below = match (&self.hold, &self.run) {
            (Some(hold), _) if hold.wheel == n => hold.moved,
            (_, Some(run)) if run.wheel == n => run.now,
            _ => 0,
        };
        let steps = nearest(below);
        ((self.value[n] - steps).rem_euclid(RANGE[n]), below - steps * PITCH)
    }
}

/// How many values a wheel moved `pixels` down has passed, to the nearest.
fn nearest(pixels: i32) -> i32 {
    (pixels + PITCH / 2).div_euclid(PITCH)
}

/// What has to be drawn again when a wheel turns: the windows of all
/// wheels there can be and what lies between them.
pub fn wheels_rect() -> Rectangle {
    let (left, right) = (WHEEL_X[1][0] - HALF, WHEEL_X[1][2] + HALF);
    Rectangle::new(Point::new(left, TOP), Size::new((right - left) as u32, 3 * PITCH as u32))
}

/// "Cancel" (0) and "save" (1).
pub fn button_rect(n: usize) -> Rectangle {
    Rectangle::new(Point::new(BUTTON_X[n], BUTTON.0), Size::new(BUTTON.2 as u32, BUTTON.1 as u32))
}

/// Two digits of `value` with their middle at `cx` and their top at `y`.
fn two_digits<D: Paint>(target: &mut D, clip: &Rectangle, cx: i32, y: i32, value: i32, color: Rgb565) {
    let width = NW_NUMBER.width as i32;
    target.glyph(clip, cx - width, y, &NW_NUMBER, b'0' + (value / 10 % 10) as u8, color);
    target.glyph(clip, cx, y, &NW_NUMBER, b'0' + (value % 10) as u8, color);
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, picker: &Picker) {
    theme::clear(target, clip);
    theme::title(target, clip, picker.title, CYAN);
    let inset = (PITCH - NW_NUMBER.height as i32) / 2;
    let middles = &WHEEL_X[picker.wheels - 2];
    for n in 0..picker.wheels {
        // Only what shows through the wheel's window.
        let window = picker.wheel_rect(n).intersection(clip);
        if !window.is_zero_sized() {
            let (value, below) = picker.shown(n);
            for k in -2..=2 {
                let y = MIDDLE + k * PITCH + below;
                // The value in the middle row is the one that counts; its
                // colour says so. (Two red lines marked the row off at
                // first, which the user found distracting.)
                let color = if (y - MIDDLE).abs() < PITCH / 2 { BONE } else { STEEL_2 };
                two_digits(target, &window, middles[n], y + inset, (value + k).rem_euclid(RANGE[n]), color);
            }
        }
        // A colon between this wheel and the next.
        if n + 1 < picker.wheels {
            let between = (middles[n] + middles[n + 1]) / 2;
            target.glyph(clip, between - NW_NUMBER.width as i32 / 2, MIDDLE + inset, &NW_NUMBER, b':', SIGNAL);
        }
    }

    for (n, text) in ["CANCEL", "SAVE"].into_iter().enumerate() {
        let rect = button_rect(n);
        let ink = if n == 1 {
            theme::plate(target, clip, &rect, 8, SIGNAL);
            BLACK
        } else {
            theme::frame(target, clip, &rect, 8, STEEL);
            BONE
        };
        // The label font's letters carry their spacing on the right.
        let x = rect.top_left.x + (BUTTON.2 - NW_LABEL.measure(text) + 3) / 2;
        target.text(clip, x, rect.top_left.y + (BUTTON.1 - NW_LABEL.height as i32) / 2, &NW_LABEL, text, ink);
    }
}
