//! The status page: what the watch knows about itself, as a list of
//! labels and values in the look of `super::theme`.
//!
//! Under the title stands a line with date and time, then one row for
//! each thing: its label on the left, its value beside it. Every value is
//! padded to a fixed width, so a row always covers the same rectangle
//! (`line_rect`).
//!
//! The page is put together in two steps, so that only what changed has
//! to be drawn: `compose` works out the text of every row, and
//! `draw_line` draws one of them. What never changes (title, labels'
//! dividers) belongs to the background.

use core::fmt::{self, Write};

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle, Rectangle};

use super::face::Time;
use super::theme::{self, BONE, C, CHROME, CYAN, GREEN, SIGNAL, STEEL, YELLOW};
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_MONO};

pub use super::theme::whole;

/// What the page shows.
pub struct Status<'a> {
    pub time: Option<Time>,
    pub uptime_s: u64,
    /// Percent, millivolts, USB power present.
    pub battery: Option<(u32, f32, bool)>,
    /// Three temperature sensors, degrees Celsius.
    pub temperatures: Option<[f32; 3]>,
    /// Pascal, metres.
    pub barometer: Option<(f32, f32)>,
    /// Beats per minute; 0 while there is no result yet.
    pub heart_bpm: Option<u32>,
    /// Ambient light in lux.
    pub lux: Option<u32>,
    pub bluetooth_connected: bool,
    /// Whether the watch is on the wrist, as it should be shown (see
    /// `app::WearView`).
    pub worn: Option<bool>,
    /// A heart-rate measurement is running.
    pub measuring_heart: bool,
    /// Fixed brightness step, 1-based, and the number of steps; `None`
    /// while the brightness follows the light.
    pub brightness_step: Option<(usize, usize)>,
    /// What the panel is set to, 0 to 255.
    pub brightness: u8,
    pub last_button: &'a str,
    /// Where the glass was last touched, if that is newer than the button.
    pub last_touch: Option<(u16, u16)>,
}

/// The value of a row, in capitals, padded with spaces to the row's width.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Line {
    text: [u8; 24],
    width: usize,
    color: Rgb565,
}

impl Line {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.width]).unwrap_or("")
    }
}

/// Collects the text of a row as it is written, in capitals.
struct Writer {
    text: [u8; 24],
    len: usize,
}

impl Writer {
    fn new() -> Self {
        Self { text: [b' '; 24], len: 0 }
    }

    /// The finished value for row `row` of the layout.
    fn line(self, row: usize, color: Rgb565) -> Line {
        Line { text: self.text, width: LAYOUT[row].1.min(self.text.len()), color }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.text.len() && b.is_ascii() {
                self.text[self.len] = b.to_ascii_uppercase();
                self.len += 1;
            }
        }
        Ok(())
    }
}

/// Number of rows on the page, the line with date and time included.
pub const LINES: usize = 10;
/// Per row: its label and the width of its value in characters. The
/// first has no label and stands in the middle.
const LAYOUT: [(&str, usize); LINES] = [
    ("", 24),
    ("BAT", 20),
    ("TEMP", 20),
    ("BARO", 20),
    ("LIGHT", 20),
    ("WRIST", 20),
    ("LINK", 20),
    ("INPUT", 20),
    ("UP", 20),
    ("PANEL", 20),
];
/// The top of the line with date and time, the top of the first row
/// under it and the distance from row to row.
const TIME_Y: i32 = 76;
const ROWS_Y: i32 = 108;
const PITCH: i32 = 29;
/// The left edge of the labels and of the values.
const LABEL_X: i32 = 88;
const VALUE_X: i32 = 176;

fn top(row: usize) -> i32 {
    if row == 0 { TIME_Y } else { ROWS_Y + (row as i32 - 1) * PITCH }
}

/// The rectangle the value of `row` covers, whatever its text.
pub fn line_rect(row: usize) -> Rectangle {
    let width = LAYOUT[row].1;
    let x = if row == 0 { C - NW_MONO.width as i32 * width as i32 / 2 } else { VALUE_X };
    NW_MONO.rect(x, top(row), width)
}

/// The rectangle the dot for a touch at (x, y) covers.
pub fn dot_rect(x: u16, y: u16) -> Rectangle {
    Rectangle::with_center(Point::new(x as i32, y as i32), Size::new(15, 15))
}

/// Paint inside `area` what the page shows whatever the values are:
/// black, the title, the labels and the dividers between the rows.
pub fn background<D: Paint>(target: &mut D, area: &Rectangle) {
    let _ = target.fill_solid(area, theme::BLACK);
    theme::title(target, area, "SYS.STATUS", SIGNAL);
    for row in 1..LINES {
        let y = top(row);
        target.text(area, LABEL_X, y, &NW_LABEL, LAYOUT[row].0, CYAN);
        if row + 1 < LINES {
            theme::rule(target, area, LABEL_X, y + PITCH - 3, 2 * (C - LABEL_X), STEEL);
        }
    }
}

/// Mark the place of a touch with a dot.
pub fn dot<D: DrawTarget<Color = Rgb565>>(target: &mut D, x: u16, y: u16) {
    let _ = Circle::with_center(Point::new(x as i32, y as i32), 13)
        .into_styled(PrimitiveStyle::with_fill(YELLOW))
        .draw(target);
}

/// Draw the value of `row`, inside `clip` only, over a background that
/// is already painted.
pub fn draw_line<D: Paint>(target: &mut D, clip: &Rectangle, row: usize, line: &Line) {
    let at = line_rect(row).top_left;
    target.text(clip, at.x, at.y, &NW_MONO, line.as_str(), line.color);
}

/// Work out the text of every row.
pub fn compose(s: &Status<'_>) -> [Line; LINES] {
    const DAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
    let mut time = Writer::new();
    match s.time {
        Some(t) => {
            let day = DAYS[(t.weekday as usize + 6) % 7];
            let _ = write!(time, "{} {:04}-{:02}-{:02}  {:02}:{:02}:{:02}", day, t.year, t.month, t.day, t.hour, t.minute, t.second);
        }
        None => {
            let _ = write!(time, "     CLOCK NOT SET");
        }
    }

    let mut battery = Writer::new();
    match s.battery {
        Some((percent, mv, usb)) => {
            let _ = write!(battery, "{}%  {:.2} V{}", percent, mv / 1000.0, if usb { "  USB" } else { "" });
        }
        None => {
            let _ = write!(battery, "--");
        }
    }
    let battery_color = match s.battery {
        Some((percent, _, _)) if percent < 20 => SIGNAL,
        Some(_) => GREEN,
        None => CHROME,
    };

    let mut temperatures = Writer::new();
    match s.temperatures {
        Some(t) => {
            let _ = write!(temperatures, "{:.1}  {:.1}  {:.1} C", t[0], t[1], t[2]);
        }
        None => {
            let _ = write!(temperatures, "--");
        }
    }

    let mut pressure = Writer::new();
    match s.barometer {
        Some((pascal, metres)) => {
            let _ = write!(pressure, "{:.1} hPa  {:.0} m", pascal / 100.0, metres);
        }
        None => {
            let _ = write!(pressure, "--");
        }
    }

    let mut light = Writer::new();
    match s.lux {
        Some(lux) => {
            let _ = write!(light, "{} lux", lux);
        }
        None => {
            let _ = write!(light, "--");
        }
    }

    let mut wear = Writer::new();
    let _ = write!(wear, "{}", match s.worn {
        Some(true) => "worn",
        Some(false) => "not worn",
        None => "--",
    });

    let mut link = Writer::new();
    let _ = write!(link, "{}", if s.bluetooth_connected { "connected" } else { "idle" });

    let mut event = Writer::new();
    match s.last_touch {
        _ if s.measuring_heart => match s.heart_bpm {
            Some(bpm) if bpm != 0 => {
                let _ = write!(event, "heart {} bpm", bpm);
            }
            _ => {
                let _ = write!(event, "heart: measuring");
            }
        },
        Some((x, y)) => {
            let _ = write!(event, "touch {}, {}", x, y);
        }
        None => {
            let _ = write!(event, "{}", s.last_button);
        }
    }

    let mut up = Writer::new();
    let (h, m, sec) = (s.uptime_s / 3600, s.uptime_s / 60 % 60, s.uptime_s % 60);
    let _ = write!(up, "{:02}:{:02}:{:02}", h, m, sec);

    let mut brightness = Writer::new();
    match s.brightness_step {
        Some((step, of)) => {
            let _ = write!(brightness, "level {}/{}  {}", step, of, s.brightness);
        }
        None => {
            let _ = write!(brightness, "auto {}", s.brightness);
        }
    }

    [
        time.line(0, CHROME),
        battery.line(1, battery_color),
        temperatures.line(2, BONE),
        pressure.line(3, BONE),
        light.line(4, BONE),
        wear.line(5, BONE),
        link.line(6, if s.bluetooth_connected { GREEN } else { CHROME }),
        event.line(7, YELLOW),
        up.line(8, BONE),
        brightness.line(9, CHROME),
    ]
}
