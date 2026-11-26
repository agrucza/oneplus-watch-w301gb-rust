//! What a finger on the glass means: a tap, a long press, or a drag in
//! one of four directions.
//!
//! The touch controller only says where the finger is and when it lifts.
//! This turns that into gestures. The kinds of gesture are the stock
//! app's (read in its image, version B.70; see docs/hardware-plan.md,
//! 3.1); the numbers are our own, tried on the watch, and more forgiving
//! than stock's.
//!
//! - Once the finger is `START` pixels from where it went down, in x or
//!   in y, the touch is a **drag**, and it is horizontal or vertical for
//!   as long as it lasts: horizontal if it had moved further in x than in
//!   y at that moment, vertical otherwise (the rule of stock's
//!   `0x8047780`). The direction given with the start is the way the
//!   finger went then; it may come back later, the line it moves on
//!   stays. Stock decides after 3 pixels; here it is 15, so that a finger
//!   that wobbles while tapping still taps.
//! - A touch that is no drag after `LONG_PRESS` is a **long press**,
//!   reported while the finger is still down. Stock counts 60 ticks
//!   (`0x80F6F80`), of a length that was not read.
//! - A touch that lifts before either is a **tap**.
//!
//! After a long press the touch gives nothing more (what stock does then
//! was not read). The speed at lift-off is taken over `SPEED_WINDOW`,
//! where stock takes the last two places it sampled; places reach us
//! less evenly than that.

use embassy_time::{Duration, Instant};

use crate::drivers::touch::TouchEvent;

/// A touch this many pixels from where it began, in x or in y, is a drag.
const START: i32 = 15;
const LONG_PRESS: Duration = Duration::from_millis(600);
/// The speed at lift-off is taken over about this long.
const SPEED_WINDOW: Duration = Duration::from_millis(90);
/// A touch that begins this soon after a tap lifted is that same touch
/// reported a second time, not a second tap: no finger lifts and comes
/// down again that fast. Seen on the watch (2026-11-24): two taps 54 ms
/// apart from one touch, of which the first chose a tile in the launcher
/// and the second opened it; on the power page the second would have
/// done what the first only chose.
const BOUNCE: Duration = Duration::from_millis(100);
/// A touch no longer than this is short enough to judge by its average
/// speed, when the speed at lift-off cannot be told.
const SHORT_TOUCH: Duration = Duration::from_millis(400);

/// The way the finger went when a drag began.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// The direction of a drag that is `dx`, `dy` from where it began
    /// when it is recognised.
    fn of(dx: i32, dy: i32) -> Self {
        if dy.abs() < dx.abs() {
            if dx < 0 {
                Direction::Left
            } else {
                Direction::Right
            }
        } else if dy < 0 {
            Direction::Up
        } else {
            Direction::Down
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture {
    Tap { x: i32, y: i32 },
    LongPress { x: i32, y: i32 },
    /// A drag began at (x, y) and went off in `direction`; `dx`, `dy` is
    /// how far it has got already.
    DragStart { x: i32, y: i32, direction: Direction, dx: i32, dy: i32 },
    /// The finger is now `dx`, `dy` from where the drag began. Only the
    /// part along the drag's line counts.
    Drag { dx: i32, dy: i32 },
    /// The finger lifted `dx`, `dy` from where the drag began, moving at
    /// `speed_x`, `speed_y` pixels a second.
    DragEnd { dx: i32, dy: i32, speed_x: i32, speed_y: i32 },
}

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    x: i32,
    y: i32,
}

pub struct Gestures {
    /// Where and when the current touch began; `None` with no finger down.
    began: Option<Sample>,
    /// Where the finger was last seen.
    last: Sample,
    dragging: bool,
    long_press_reported: bool,
    /// Recent places of the finger, oldest first, for the lift-off speed.
    trail: [Sample; 6],
    trail_len: usize,
    /// When the last tap lifted, and whether the touch now on the glass
    /// began so soon after it that it is that tap again (`BOUNCE`).
    tapped_at: Option<Instant>,
    bounce: bool,
}

impl Gestures {
    pub fn new() -> Self {
        let nowhere = Sample { at: Instant::now(), x: 0, y: 0 };
        Self { began: None, last: nowhere, dragging: false, long_press_reported: false, trail: [nowhere; 6], trail_len: 0, tapped_at: None, bounce: false }
    }

    /// Take what the touch controller reported on this pass (`None` if
    /// nothing new). Call on every pass, so that a long press is noticed
    /// while the finger rests.
    pub fn feed(&mut self, event: Option<TouchEvent>, now: Instant) -> Option<Gesture> {
        match event {
            Some(TouchEvent::Down { x, y }) => {
                let here = Sample { at: now, x: x as i32, y: y as i32 };
                let Some(began) = self.began else {
                    self.began = Some(here);
                    self.last = here;
                    self.dragging = false;
                    self.long_press_reported = false;
                    self.bounce = self.tapped_at.is_some_and(|tapped| now.duration_since(tapped) < BOUNCE);
                    self.trail_len = 0;
                    self.remember(here);
                    return None;
                };
                self.last = here;
                self.remember(here);
                let (dx, dy) = (here.x - began.x, here.y - began.y);
                if self.dragging {
                    return Some(Gesture::Drag { dx, dy });
                }
                if !self.long_press_reported && (dx.abs() >= START || dy.abs() >= START) {
                    self.dragging = true;
                    return Some(Gesture::DragStart { x: began.x, y: began.y, direction: Direction::of(dx, dy), dx, dy });
                }
                self.resting(began, now)
            }
            Some(TouchEvent::Up) => {
                let began = self.began.take()?;
                let (dx, dy) = (self.last.x - began.x, self.last.y - began.y);
                if self.dragging {
                    self.dragging = false;
                    let (speed_x, speed_y) = self.speed(began, now);
                    return Some(Gesture::DragEnd { dx, dy, speed_x, speed_y });
                }
                // A long press was already reported when it happened, and
                // a tap reported twice counts once.
                if self.long_press_reported || self.bounce {
                    return None;
                }
                self.tapped_at = Some(now);
                Some(Gesture::Tap { x: began.x, y: began.y })
            }
            None => match self.began {
                Some(began) if !self.dragging => self.resting(began, now),
                _ => None,
            },
        }
    }

    /// The finger is down and has not moved: a long press once it has
    /// been there long enough.
    fn resting(&mut self, began: Sample, now: Instant) -> Option<Gesture> {
        if !self.long_press_reported && now.duration_since(began.at) >= LONG_PRESS {
            self.long_press_reported = true;
            return Some(Gesture::LongPress { x: began.x, y: began.y });
        }
        None
    }

    fn remember(&mut self, sample: Sample) {
        if self.trail_len == self.trail.len() {
            self.trail.copy_within(1.., 0);
            self.trail_len -= 1;
        }
        self.trail[self.trail_len] = sample;
        self.trail_len += 1;
    }

    /// Pixels a second at lift-off: from the oldest remembered place that
    /// is not older than `SPEED_WINDOW` to the latest.
    ///
    /// When too few places arrived in that window to tell (the firmware
    /// was busy drawing and read the touch controller rarely), a touch
    /// that was short altogether counts with its average speed since it
    /// began: a finger that crossed the glass in a quarter of a second
    /// was a swipe, however few places of it were seen. Zero if the
    /// finger rested before it lifted.
    fn speed(&self, began: Sample, now: Instant) -> (i32, i32) {
        let per_second = |from: Sample, to: Sample| {
            let ms = to.at.duration_since(from.at).as_millis() as i32;
            (ms >= 10).then(|| ((to.x - from.x) * 1000 / ms, (to.y - from.y) * 1000 / ms))
        };
        let latest = self.last;
        if now.duration_since(latest.at) > SPEED_WINDOW {
            return (0, 0);
        }
        let recent = self.trail[..self.trail_len].iter().find(|s| latest.at.duration_since(s.at) <= SPEED_WINDOW);
        if let Some(speed) = recent.and_then(|&from| per_second(from, latest)) {
            return speed;
        }
        if latest.at.duration_since(began.at) <= SHORT_TOUCH {
            return per_second(began, latest).unwrap_or((0, 0));
        }
        (0, 0)
    }
}
