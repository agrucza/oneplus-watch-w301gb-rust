//! The status screen: what the watch knows about itself, as text.
//!
//! The screen is round, so the lines are centred and the widest ones sit in
//! the middle. Every line is drawn with a background and padded to a fixed
//! width, so redrawing a line replaces the old text without clearing the
//! frame first.

use core::fmt::{self, Write};

use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use profont::{PROFONT_18_POINT, PROFONT_24_POINT};

use crate::drivers::apollo::messages::Readings;
use crate::drivers::panel::{Frame, WIDTH};
use crate::services::canvas::Canvas;
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

/// A line of text of fixed capacity; longer text is cut off.
struct Line {
    text: [u8; 28],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Self { text: [b' '; 28], len: 0 }
    }

    /// The text, centred in `width` characters by padding with spaces.
    fn centred(&mut self, width: usize) -> &str {
        let width = width.min(self.text.len());
        let len = self.len.min(width);
        let left = (width - len) / 2;
        self.text.copy_within(..len, left);
        self.text[..left].fill(b' ');
        self.text[left + len..].fill(b' ');
        core::str::from_utf8(&self.text[..width]).unwrap_or("")
    }
}

impl fmt::Write for Line {
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

/// Paint the frame black; call once before the first `draw`.
pub fn clear(frame: &mut Frame) {
    Canvas(frame).clear_to(BACKGROUND);
}

/// Mark the place of a touch with a dot.
pub fn dot(frame: &mut Frame, x: u16, y: u16) {
    let _ = Circle::with_center(Point::new(x as i32, y as i32), 13)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::MAGENTA))
        .draw(&mut Canvas(frame));
}

/// Draw all lines of the status screen.
pub fn draw(frame: &mut Frame, s: &Status<'_>) {
    let mut canvas = Canvas(frame);
    let centre = TextStyleBuilder::new().alignment(Alignment::Center).baseline(Baseline::Top).build();
    let mut put = |y: i32, big: bool, color: Rgb565, width: usize, line: &mut Line| {
        let font = if big { &PROFONT_24_POINT } else { &PROFONT_18_POINT };
        let style = MonoTextStyleBuilder::new().font(font).text_color(color).background_color(BACKGROUND).build();
        let _ = Text::with_text_style(line.centred(width), Point::new(WIDTH as i32 / 2, y), style, centre)
            .draw(&mut canvas);
    };

    let now = clock::now();

    let mut line = Line::new();
    match now {
        Some(t) => {
            let _ = write!(line, "{} {:04}-{:02}-{:02}", t.weekday_name(), t.year, t.month, t.day);
        }
        None => {
            let _ = write!(line, "W301 Rust");
        }
    }
    put(52, false, DIM, 14, &mut line);

    let mut line = Line::new();
    match now {
        Some(t) => {
            let _ = write!(line, "{:02}:{:02}:{:02}", t.hour, t.minute, t.second);
        }
        None => {
            let _ = write!(line, "clock not set");
        }
    }
    put(92, true, Rgb565::WHITE, 14, &mut line);

    let mut line = Line::new();
    match s.readings.and_then(|r| r.battery) {
        Some((percent, mv, usb)) => {
            let _ = write!(line, "{}%  {:.2} V{}", percent, mv / 1000.0, if usb { "  USB" } else { "" });
        }
        None => {
            let _ = write!(line, "battery --");
        }
    }
    let color = match s.readings.and_then(|r| r.battery) {
        Some((percent, _, _)) if percent < 20 => Rgb565::RED,
        _ => Rgb565::GREEN,
    };
    put(146, true, color, 20, &mut line);

    let mut line = Line::new();
    match s.readings.and_then(|r| r.temperatures) {
        Some(t) => {
            let _ = write!(line, "{:.1}  {:.1}  {:.1} C", t[0], t[1], t[2]);
        }
        None => {
            let _ = write!(line, "temperatures --");
        }
    }
    put(196, true, Rgb565::WHITE, 22, &mut line);

    let mut line = Line::new();
    match s.readings.and_then(|r| r.barometer) {
        Some((pascal, metres)) => {
            let _ = write!(line, "{:.1} hPa  {:.0} m", pascal / 100.0, metres);
        }
        None => {
            let _ = write!(line, "pressure --");
        }
    }
    put(246, true, Rgb565::WHITE, 22, &mut line);

    let mut line = Line::new();
    let worn = match s.worn {
        Some(true) => "worn",
        Some(false) => "not worn",
        None => "wear --",
    };
    let _ = write!(line, "{}   BT {}", worn, if s.bluetooth_connected { "connected" } else { "idle" });
    put(300, false, Rgb565::CYAN, 26, &mut line);

    let mut line = Line::new();
    match s.last_touch {
        _ if s.measuring_heart => match s.readings.and_then(|r| r.heart_bpm) {
            Some(bpm) if bpm != 0 => {
                let _ = write!(line, "heart {} bpm", bpm);
            }
            _ => {
                let _ = write!(line, "heart: measuring");
            }
        },
        Some((x, y)) => {
            let _ = write!(line, "touch {}, {}", x, y);
        }
        None => {
            let _ = write!(line, "{}", s.last_button);
        }
    }
    put(336, false, Rgb565::YELLOW, 22, &mut line);

    let mut line = Line::new();
    match s.lux {
        Some(lux) => {
            let _ = write!(line, "{} lux", lux);
        }
        None => {
            let _ = write!(line, "light --");
        }
    }
    let (h, m, sec) = (s.uptime_s / 3600, s.uptime_s / 60 % 60, s.uptime_s % 60);
    let _ = write!(line, "  up {:02}:{:02}:{:02}", h, m, sec);
    put(368, false, Rgb565::WHITE, 24, &mut line);

    let mut line = Line::new();
    match s.brightness_step {
        Some((step, of)) => {
            let _ = write!(line, "level {}/{}", step, of);
        }
        None => {
            let _ = write!(line, "auto {}", s.brightness);
        }
    }
    put(396, false, DIM, 10, &mut line);
}
