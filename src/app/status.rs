//! The status screen: what the watch knows about itself, as text.
//!
//! The screen is round, so the lines are centred and the widest ones sit in
//! the middle. Every line is drawn with a background and padded to a fixed
//! width, so a line always covers the same rectangle (`line_rect`).
//!
//! The screen is put together in two steps, so that only what changed has
//! to be drawn: `compose` works out the text of every line, and
//! `draw_line` draws one of them.

use core::fmt::{self, Write};

use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use profont::{PROFONT_18_POINT, PROFONT_24_POINT};

use crate::drivers::apollo::messages::Readings;
use crate::drivers::panel::{HEIGHT, WIDTH};
use crate::services::clock;

const BACKGROUND: Rgb565 = Rgb565::BLACK;
const DIM: Rgb565 = Rgb565::new(15, 31, 15); // mid grey

/// What the screen shows besides the sensor values.
pub struct Status<'a> {
    pub uptime_s: u64,
    pub readings: Option<&'a Readings>,
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

/// A line of text, padded with spaces to its fixed width and centred in it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Line {
    text: [u8; 28],
    width: usize,
    color: Rgb565,
}

impl Line {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.width]).unwrap_or("")
    }
}

/// Collects the text of a line as it is written.
struct Writer {
    text: [u8; 28],
    len: usize,
}

impl Writer {
    fn new() -> Self {
        Self { text: [b' '; 28], len: 0 }
    }

    /// The finished line for row `row` of the layout.
    fn line(mut self, row: usize, color: Rgb565) -> Line {
        let width = LAYOUT[row].2.min(self.text.len());
        let len = self.len.min(width);
        let left = (width - len) / 2;
        self.text.copy_within(..len, left);
        self.text[..left].fill(b' ');
        self.text[left + len..].fill(b' ');
        Line { text: self.text, width, color }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.text.len() && b.is_ascii() {
                self.text[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

/// Number of lines on the screen.
pub const LINES: usize = 9;
/// Per line: top edge, large font or small, width in characters.
const LAYOUT: [(i32, bool, usize); LINES] = [
    (52, false, 14),
    (92, true, 14),
    (146, true, 20),
    (196, true, 22),
    (246, true, 22),
    (300, false, 26),
    (336, false, 22),
    (368, false, 24),
    (396, false, 10),
];

fn font(big: bool) -> &'static embedded_graphics::mono_font::MonoFont<'static> {
    if big {
        &PROFONT_24_POINT
    } else {
        &PROFONT_18_POINT
    }
}

/// The whole picture.
pub fn whole() -> Rectangle {
    Rectangle::new(Point::zero(), Size::new(WIDTH as u32, HEIGHT as u32))
}

/// The rectangle line `row` covers, whatever its text.
pub fn line_rect(row: usize) -> Rectangle {
    let (y, big, width) = LAYOUT[row];
    let f = font(big);
    let w = (f.character_size.width + f.character_spacing) * width as u32;
    Rectangle::new(Point::new(WIDTH as i32 / 2 - w as i32 / 2 - 1, y), Size::new(w + 2, f.character_size.height))
}

/// The rectangle the dot for a touch at (x, y) covers.
pub fn dot_rect(x: u16, y: u16) -> Rectangle {
    Rectangle::with_center(Point::new(x as i32, y as i32), Size::new(15, 15))
}

/// Paint the background inside `area`.
pub fn background<D: DrawTarget<Color = Rgb565>>(target: &mut D, area: &Rectangle) {
    let _ = target.fill_solid(area, BACKGROUND);
}

/// Mark the place of a touch with a dot.
pub fn dot<D: DrawTarget<Color = Rgb565>>(target: &mut D, x: u16, y: u16) {
    let _ = Circle::with_center(Point::new(x as i32, y as i32), 13)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::MAGENTA))
        .draw(target);
}

/// Draw line `row`.
pub fn draw_line<D: DrawTarget<Color = Rgb565>>(target: &mut D, row: usize, line: &Line) {
    let (y, big, _) = LAYOUT[row];
    let centre = TextStyleBuilder::new().alignment(Alignment::Center).baseline(Baseline::Top).build();
    let style = MonoTextStyleBuilder::new().font(font(big)).text_color(line.color).background_color(BACKGROUND).build();
    let _ = Text::with_text_style(line.as_str(), Point::new(WIDTH as i32 / 2, y), style, centre).draw(target);
}

/// Work out the text of every line.
pub fn compose(s: &Status<'_>) -> [Line; LINES] {
    let now = clock::now();

    let mut date = Writer::new();
    match now {
        Some(t) => {
            let _ = write!(date, "{} {:04}-{:02}-{:02}", t.weekday_name(), t.year, t.month, t.day);
        }
        None => {
            let _ = write!(date, "W301 Rust");
        }
    }

    let mut time = Writer::new();
    match now {
        Some(t) => {
            let _ = write!(time, "{:02}:{:02}:{:02}", t.hour, t.minute, t.second);
        }
        None => {
            let _ = write!(time, "clock not set");
        }
    }

    let mut battery = Writer::new();
    match s.readings.and_then(|r| r.battery) {
        Some((percent, mv, usb)) => {
            let _ = write!(battery, "{}%  {:.2} V{}", percent, mv / 1000.0, if usb { "  USB" } else { "" });
        }
        None => {
            let _ = write!(battery, "battery --");
        }
    }
    let battery_color = match s.readings.and_then(|r| r.battery) {
        Some((percent, _, _)) if percent < 20 => Rgb565::RED,
        _ => Rgb565::GREEN,
    };

    let mut temperatures = Writer::new();
    match s.readings.and_then(|r| r.temperatures) {
        Some(t) => {
            let _ = write!(temperatures, "{:.1}  {:.1}  {:.1} C", t[0], t[1], t[2]);
        }
        None => {
            let _ = write!(temperatures, "temperatures --");
        }
    }

    let mut pressure = Writer::new();
    match s.readings.and_then(|r| r.barometer) {
        Some((pascal, metres)) => {
            let _ = write!(pressure, "{:.1} hPa  {:.0} m", pascal / 100.0, metres);
        }
        None => {
            let _ = write!(pressure, "pressure --");
        }
    }

    let mut wear = Writer::new();
    let worn = match s.worn {
        Some(true) => "worn",
        Some(false) => "not worn",
        None => "wear --",
    };
    let _ = write!(wear, "{}   BT {}", worn, if s.bluetooth_connected { "connected" } else { "idle" });

    let mut event = Writer::new();
    match s.last_touch {
        _ if s.measuring_heart => match s.readings.and_then(|r| r.heart_bpm) {
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

    let mut light = Writer::new();
    match s.lux {
        Some(lux) => {
            let _ = write!(light, "{} lux", lux);
        }
        None => {
            let _ = write!(light, "light --");
        }
    }
    let (h, m, sec) = (s.uptime_s / 3600, s.uptime_s / 60 % 60, s.uptime_s % 60);
    let _ = write!(light, "  up {:02}:{:02}:{:02}", h, m, sec);

    let mut brightness = Writer::new();
    match s.brightness_step {
        Some((step, of)) => {
            let _ = write!(brightness, "level {}/{}", step, of);
        }
        None => {
            let _ = write!(brightness, "auto {}", s.brightness);
        }
    }

    [
        date.line(0, DIM),
        time.line(1, Rgb565::WHITE),
        battery.line(2, battery_color),
        temperatures.line(3, Rgb565::WHITE),
        pressure.line(4, Rgb565::WHITE),
        wear.line(5, Rgb565::CYAN),
        event.line(6, Rgb565::YELLOW),
        light.line(7, Rgb565::WHITE),
        brightness.line(8, DIM),
    ]
}
