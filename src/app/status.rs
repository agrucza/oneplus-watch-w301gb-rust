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
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use profont::{PROFONT_18_POINT, PROFONT_24_POINT};

use crate::drivers::apollo::messages::Readings;
use crate::drivers::light;
use crate::drivers::panel::{Frame, WIDTH};
use crate::services::canvas::Canvas;

const BACKGROUND: Rgb565 = Rgb565::BLACK;
const DIM: Rgb565 = Rgb565::new(15, 31, 15); // mid grey

/// What the screen shows besides the sensor values.
pub struct Status<'a> {
    pub uptime_s: u64,
    pub readings: Option<&'a Readings>,
    /// Raw light sensor values.
    pub light: Option<light::Raw>,
    pub bluetooth_connected: bool,
    /// Brightness step, 1-based, and the number of steps.
    pub brightness: (usize, usize),
    pub last_button: &'a str,
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

    let mut line = Line::new();
    let _ = write!(line, "W301 Rust");
    put(52, false, DIM, 12, &mut line);

    let mut line = Line::new();
    let (h, m, sec) = (s.uptime_s / 3600, s.uptime_s / 60 % 60, s.uptime_s % 60);
    let _ = write!(line, "up {:02}:{:02}:{:02}", h, m, sec);
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
    let worn = match s.readings.and_then(|r| r.worn) {
        Some(true) => "worn",
        Some(false) => "not worn",
        None => "wear --",
    };
    let _ = write!(line, "{}   BT {}", worn, if s.bluetooth_connected { "connected" } else { "idle" });
    put(300, false, Rgb565::CYAN, 26, &mut line);

    let mut line = Line::new();
    let _ = write!(line, "{}", s.last_button);
    put(336, false, Rgb565::YELLOW, 22, &mut line);

    let mut line = Line::new();
    match s.light {
        Some(raw) => {
            let _ = write!(line, "{} lux", light::LightSensor::lux(raw));
        }
        None => {
            let _ = write!(line, "light --");
        }
    }
    put(368, false, Rgb565::WHITE, 20, &mut line);

    let mut line = Line::new();
    let _ = write!(line, "level {}/{}", s.brightness.0, s.brightness.1);
    put(396, false, DIM, 10, &mut line);
}
