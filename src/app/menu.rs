//! The settings app: every setting there is as a row of a list that
//! scrolls. (The panel pulled down over the watch face holds the ones
//! wanted quickly, with a button each; see `super::settings`.)
//!
//! A row shows a setting's name and what it is set to. A tap on it
//! switches it or takes the next of its choices; the rows with a daily
//! time lead to the page for setting one. The list follows the finger
//! and runs on a little when it is let go.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::settings::{HeartMonitor, Settings, Tap};
use super::theme::{self, BONE, C, CHROME, CYAN, SIGNAL, SIZE, STEEL, STEEL_2};
use crate::services::font::Paint;
use crate::services::font_data::{NW_ICON_NEXT, NW_LABEL, NW_MONO};

/// What a row is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    /// Opens the panel with the quick settings.
    Quick,
    Brightness,
    DisplayOff,
    RaiseToWake,
    Quiet,
    MoveAlert,
    StepGoal,
    HeartMonitor,
    Volume,
}

pub const ROWS: [Row; 9] = [Row::Quick, Row::Brightness, Row::DisplayOff, Row::Volume, Row::RaiseToWake, Row::Quiet, Row::MoveAlert, Row::StepGoal, Row::HeartMonitor];

/// The left edge and the width of the rows, and the height of one.
const LEFT: i32 = 66;
const WIDTH: i32 = SIZE - 2 * LEFT;
pub const ROW: i32 = 52;
/// The window the list shows through: its top and its height.
const TOP: i32 = 76;
const HEIGHT: i32 = 300;
/// The switch of a row that has one: width and height.
const SWITCH: (i32, i32) = (64, 30);
/// How far right of the rows the mark for the list's place stands.
const MARK_GAP: i32 = 14;

/// The window the list shows through.
pub fn window() -> Rectangle {
    Rectangle::new(Point::new(LEFT, TOP), Size::new(WIDTH as u32, HEIGHT as u32))
}

/// What has to be drawn again when the list moves: the window and the
/// mark beside it that says how far down the list it is.
pub fn moving_rect() -> Rectangle {
    Rectangle::new(Point::new(LEFT, TOP), Size::new((WIDTH + MARK_GAP + 3) as u32, HEIGHT as u32))
}

/// How far the list can be moved up at most.
pub fn longest() -> i32 {
    (ROWS.len() as i32 * ROW - HEIGHT).max(0)
}

/// What a tap on the list did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// Open the panel with the quick settings.
    Quick,
    /// Change a setting, or open the page for one of the two times.
    Setting(Tap),
}

/// What a tap at (x, y) lands on, with the list moved up by `moved`.
pub fn tap_at(x: i32, y: i32, moved: i32) -> Option<Hit> {
    if !window().contains(Point::new(x, y)) {
        return None;
    }
    let row = *ROWS.get(((y - TOP + moved) / ROW) as usize)?;
    // In a row with a time, the switch switches and the rest of the row
    // leads to setting the time; in a row with a switch alone, only the
    // switch switches.
    let on_switch = x >= LEFT + WIDTH - SWITCH.0 - 12;
    Some(match (row, on_switch) {
        (Row::Quick, _) => Hit::Quick,
        (Row::Brightness, _) => Hit::Setting(Tap::NextBrightness),
        (Row::DisplayOff, _) => Hit::Setting(Tap::NextDisplayOff),
        (Row::StepGoal, _) => Hit::Setting(Tap::NextStepGoal),
        (Row::HeartMonitor, _) => Hit::Setting(Tap::NextHeartMonitor),
        (Row::Volume, _) => Hit::Setting(Tap::NextVolume),
        (Row::RaiseToWake, true) => Hit::Setting(Tap::RaiseToWake),
        (Row::Quiet, true) => Hit::Setting(Tap::Quiet),
        (Row::MoveAlert, true) => Hit::Setting(Tap::MoveReminder),
        (Row::Quiet, false) => Hit::Setting(Tap::PickQuietTime),
        (Row::MoveAlert, false) => Hit::Setting(Tap::PickMoveTime),
        (Row::RaiseToWake, false) => return None,
    })
}

/// A short text put together in place.
struct Text {
    bytes: [u8; 12],
    len: usize,
}

impl Text {
    fn new() -> Self {
        Self { bytes: [0; 12], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Draw the part of the page that lies in `clip`, with the list moved up
/// by `moved`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, settings: &Settings, moved: i32) {
    use core::fmt::Write;
    theme::clear(target, clip);
    theme::title(target, clip, "SETTINGS", SIGNAL);
    // Only what shows through the window.
    let window = window().intersection(clip);
    if window.is_zero_sized() {
        return;
    }
    for (n, &row) in ROWS.iter().enumerate() {
        let y = TOP + n as i32 * ROW - moved;
        if y + ROW <= TOP || y >= TOP + HEIGHT {
            continue;
        }
        let mut value = Text::new();
        let time = |value: &mut Text, (from, to): (u16, u16)| {
            let _ = write!(value, "{:02}:{:02}-{:02}:{:02}", from / 60, from % 60, to / 60, to % 60);
        };
        let (label, switch) = match row {
            Row::Quick => ("QUICK SETTINGS", None),
            Row::Brightness => {
                match settings.brightness {
                    None => {
                        let _ = write!(value, "AUTO");
                    }
                    Some(step) => {
                        let _ = write!(value, "{} OF 4", step + 1);
                    }
                }
                ("BRIGHTNESS", None)
            }
            Row::DisplayOff => {
                let _ = write!(value, "{} S", settings.display_off_s);
                ("DISPLAY OFF", None)
            }
            Row::RaiseToWake => ("RAISE TO WAKE", Some(settings.raise_to_wake)),
            Row::Quiet => {
                time(&mut value, settings.quiet_time);
                ("QUIET", Some(settings.quiet))
            }
            Row::MoveAlert => {
                time(&mut value, settings.move_time);
                ("MOVE ALERT", Some(settings.move_reminder))
            }
            Row::StepGoal => {
                let _ = write!(value, "{}", settings.step_goal);
                ("STEP GOAL", None)
            }
            Row::Volume => {
                let _ = write!(value, "{}", ["QUIET", "LOW", "MEDIUM", "LOUD"][settings.volume as usize % 4]);
                ("VOLUME", None)
            }
            Row::HeartMonitor => {
                let _ = write!(value, "{}", match settings.heart_monitor {
                    HeartMonitor::Off => "OFF",
                    HeartMonitor::Periodic => "10 MIN",
                    HeartMonitor::Always => "ALWAYS",
                });
                ("HEART RATE", None)
            }
        };
        target.text(&window, LEFT, y + (ROW - NW_LABEL.height as i32) / 2, &NW_LABEL, label, BONE);
        let mut right = LEFT + WIDTH;
        match (row, switch) {
            (_, Some(on)) => {
                right -= SWITCH.0;
                theme::switch(target, &window, right, y + (ROW - SWITCH.1) / 2, SWITCH, on);
                right -= 12;
            }
            (Row::Quick, _) => {
                right -= NW_ICON_NEXT.width as i32;
                target.art(&window, right, y + (ROW - NW_ICON_NEXT.height as i32) / 2, &NW_ICON_NEXT, CYAN);
            }
            _ => {}
        }
        let color = if switch.is_some() { CHROME } else { CYAN };
        target.text(&window, right - NW_MONO.measure(value.as_str()), y + (ROW - NW_MONO.height as i32) / 2, &NW_MONO, value.as_str(), color);
        let line = Rectangle::new(Point::new(LEFT, y + ROW - 1), Size::new(WIDTH as u32, 1));
        let _ = target.fill_solid(&line.intersection(&window), STEEL);
    }
    // How far down the list the window is: a mark at the right, only
    // when there is more than fits.
    if longest() > 0 {
        let all = ROWS.len() as i32 * ROW;
        let (track_top, track) = (C - 60, 120);
        let (mark, at) = (track * HEIGHT / all, track * moved / all);
        let x = LEFT + WIDTH + MARK_GAP;
        let _ = target.fill_solid(&Rectangle::new(Point::new(x, track_top), Size::new(3, track as u32)).intersection(clip), STEEL);
        let _ = target.fill_solid(&Rectangle::new(Point::new(x, track_top + at), Size::new(3, mark as u32)).intersection(clip), STEEL_2);
    }
}

// --- Scrolling -------------------------------------------------------------------

/// A touch that moved less than this is a tap and moves nothing.
const TAP: i32 = 15;
/// The finger's speed is taken over this long before it lifts.
const SPEED_MS: u64 = 90;
/// A list let go runs on, losing this share of its speed every
/// millisecond (in thousandths), and stops below this speed.
const SLOWING: i64 = 4;
const SLOWEST: i32 = 40;

/// How far the list is moved up, and how the finger moves it.
pub struct Scroll {
    /// Pixels the list is moved up, in thousandths (the running on needs
    /// the fraction).
    moved: i64,
    hold: Option<Hold>,
    /// Running on after being let go: pixels a second, and when it was
    /// last carried on.
    run: Option<(i32, u64)>,
}

struct Hold {
    /// Where the finger came down and where the list stood then.
    start_y: i32,
    start_moved: i32,
    furthest: i32,
    samples: [(u64, i32); 4],
}

impl Scroll {
    pub const fn new() -> Self {
        Self { moved: 0, hold: None, run: None }
    }

    /// How far the list is moved up, in pixels.
    pub fn moved(&self) -> i32 {
        (self.moved / 1000) as i32
    }

    fn set(&mut self, moved: i32) -> bool {
        let before = self.moved();
        self.moved = moved.clamp(0, longest()) as i64 * 1000;
        self.moved() != before
    }

    /// Where the finger is at `now_ms` (`None`: it lifted). True if the
    /// list has moved and is to be drawn again.
    pub fn touch(&mut self, at: Option<(i32, i32)>, now_ms: u64) -> bool {
        match (at, self.hold.as_mut()) {
            (Some((x, y)), None) => {
                // Catching a running list stops it.
                self.run = None;
                if window().contains(Point::new(x, y)) {
                    self.hold = Some(Hold { start_y: y, start_moved: self.moved(), furthest: 0, samples: [(now_ms, y); 4] });
                }
                false
            }
            (Some((_, y)), Some(hold)) => {
                hold.furthest = hold.furthest.max((y - hold.start_y).abs());
                hold.samples.rotate_left(1);
                hold.samples[3] = (now_ms, y);
                // Not before it is clearly no tap: a tap would nudge the
                // list otherwise.
                if hold.furthest < TAP {
                    return false;
                }
                let moved = hold.start_moved - (y - hold.start_y);
                self.set(moved)
            }
            (None, Some(hold)) => {
                let (last_at, last_y) = hold.samples[3];
                let (first_at, first_y) = hold.samples.iter().copied().find(|&(at, _)| last_at - at <= SPEED_MS).unwrap_or((last_at, last_y));
                // The list moves up as the finger does.
                let speed = if last_at > first_at { (first_y - last_y) * 1000 / (last_at - first_at) as i32 } else { 0 };
                let dragged = hold.furthest >= TAP;
                self.hold = None;
                if dragged && speed.abs() >= SLOWEST {
                    self.run = Some((speed, now_ms));
                }
                false
            }
            (None, None) => false,
        }
    }

    /// Carry a list that is running on to where it is at `now_ms`. True
    /// if it has moved and is to be drawn again.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        let Some((speed, last)) = self.run else {
            return false;
        };
        let passed = now_ms.saturating_sub(last).min(100) as i64;
        if passed == 0 {
            return false;
        }
        let before = self.moved();
        self.moved = (self.moved + speed as i64 * passed).clamp(0, longest() as i64 * 1000);
        let slower = (speed as i64 * (1000 - SLOWING * passed).max(0) / 1000) as i32;
        let at_an_end = self.moved == 0 || self.moved == longest() as i64 * 1000;
        self.run = (slower.abs() >= SLOWEST && !at_an_end).then_some((slower, now_ms));
        self.moved() != before
    }
}
