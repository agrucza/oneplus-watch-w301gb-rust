//! What the user can set on the watch, the file it is kept in, and the
//! page it is set on: the panel pulled down over the watch face, the
//! design's "quick access".
//!
//! Three things for now, each of which the firmware acts on: how bright
//! the display is (by the light, or one of four fixed steps), after how
//! long without a finger or a button it goes dark, and whether raising
//! the wrist lights it.
//!
//! The file is text, a line a setting ("display_off=15"), so that it can
//! be read and written by hand. A line that is not understood is left
//! out; whatever is missing keeps its start value.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BLACK, BONE, CHROME, CYAN, SIZE, STEEL, YELLOW};
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_MONO};

/// The fixed brightness steps there are.
pub const LEVELS: u8 = 4;
/// The times to choose from after which the display goes dark, seconds.
pub const TIMEOUTS: [u16; 4] = [10, 15, 30, 60];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// `None`: the brightness follows the light. `Some(n)`: fixed step
    /// `n`, counted from 0.
    pub brightness: Option<u8>,
    /// Dark after this many seconds without a finger or a button.
    pub display_off_s: u16,
    /// Raising the wrist lights the display.
    pub raise_to_wake: bool,
}

/// What a tap on the page changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tap {
    Brightness(Option<u8>),
    DisplayOff(u16),
    RaiseToWake,
}

impl Settings {
    /// The settings of a watch that has no file yet.
    pub const START: Settings = Settings { brightness: None, display_off_s: 15, raise_to_wake: true };

    /// The settings a file's text holds.
    pub fn parse(text: &[u8]) -> Settings {
        let mut settings = Settings::START;
        for line in text.split(|&b| b == b'\n') {
            let Ok(line) = core::str::from_utf8(line) else {
                continue;
            };
            let Some((name, value)) = line.trim().split_once('=') else {
                continue;
            };
            match (name.trim(), value.trim()) {
                ("brightness", "auto") => settings.brightness = None,
                ("brightness", step) => {
                    if let Some(step) = step.parse::<u8>().ok().filter(|step| (1..=LEVELS).contains(step)) {
                        settings.brightness = Some(step - 1);
                    }
                }
                ("display_off", seconds) => {
                    if let Some(seconds) = seconds.parse().ok().filter(|seconds| TIMEOUTS.contains(seconds)) {
                        settings.display_off_s = seconds;
                    }
                }
                ("raise_to_wake", "on") => settings.raise_to_wake = true,
                ("raise_to_wake", "off") => settings.raise_to_wake = false,
                _ => {}
            }
        }
        settings
    }

    /// The file's text for these settings, written into `out`. Returns
    /// its length.
    pub fn text(&self, out: &mut [u8; 64]) -> usize {
        use core::fmt::Write;
        struct Into<'a>(&'a mut [u8; 64], usize);
        impl Write for Into<'_> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let n = s.len().min(self.0.len() - self.1);
                self.0[self.1..self.1 + n].copy_from_slice(&s.as_bytes()[..n]);
                self.1 += n;
                Ok(())
            }
        }
        let mut into = Into(out, 0);
        match self.brightness {
            None => {
                let _ = writeln!(into, "brightness=auto");
            }
            Some(step) => {
                let _ = writeln!(into, "brightness={}", step + 1);
            }
        }
        let _ = writeln!(into, "display_off={}", self.display_off_s);
        let _ = writeln!(into, "raise_to_wake={}", if self.raise_to_wake { "on" } else { "off" });
        into.1
    }

    /// The settings after `tap`.
    pub fn after(self, tap: Tap) -> Settings {
        match tap {
            Tap::Brightness(brightness) => Settings { brightness, ..self },
            Tap::DisplayOff(display_off_s) => Settings { display_off_s, ..self },
            Tap::RaiseToWake => Settings { raise_to_wake: !self.raise_to_wake, ..self },
        }
    }
}

// --- The page -------------------------------------------------------------------

/// The left edge and the width of everything on the page.
const LEFT: i32 = 62;
const WIDTH: i32 = SIZE - 2 * LEFT;
/// Buttons in a row: their height and the gap between them.
const BUTTON: i32 = 40;
const GAP: i32 = 6;
/// The tops of the two labels with their rows of buttons under them, and
/// of the row with the switch.
const BRIGHTNESS_Y: i32 = 80;
const DISPLAY_OFF_Y: i32 = 162;
const RAISE_Y: i32 = 248;
const LABEL_TO_ROW: i32 = 26;
/// The switch: width and height.
const SWITCH: (i32, i32) = (64, 30);

/// The button for automatic brightness (0) or for fixed step `n - 1`.
pub fn brightness_rect(n: usize) -> Rectangle {
    // "AUTO" is wider than the digits.
    let narrow = (WIDTH - 90 - LEVELS as i32 * GAP) / LEVELS as i32;
    let (x, width) = if n == 0 { (LEFT, 90) } else { (LEFT + 90 + GAP + (n as i32 - 1) * (narrow + GAP), narrow) };
    Rectangle::new(Point::new(x, BRIGHTNESS_Y + LABEL_TO_ROW), Size::new(width as u32, BUTTON as u32))
}

/// The button for `TIMEOUTS[n]`.
pub fn display_off_rect(n: usize) -> Rectangle {
    let width = (WIDTH - (TIMEOUTS.len() as i32 - 1) * GAP) / TIMEOUTS.len() as i32;
    Rectangle::new(Point::new(LEFT + n as i32 * (width + GAP), DISPLAY_OFF_Y + LABEL_TO_ROW), Size::new(width as u32, BUTTON as u32))
}

/// The row with the switch for raising the wrist.
pub fn raise_rect() -> Rectangle {
    Rectangle::new(Point::new(LEFT, RAISE_Y), Size::new(WIDTH as u32, 44))
}

/// What a tap at (x, y) changes, if it lands on anything.
pub fn tap_at(x: i32, y: i32) -> Option<Tap> {
    let at = Point::new(x, y);
    // A finger is wider than a button's edge: half the gap counts too.
    let hit = |r: Rectangle| Rectangle::new(r.top_left - Point::new(GAP / 2, GAP / 2), r.size + Size::new(GAP as u32, GAP as u32)).contains(at);
    if let Some(n) = (0..=LEVELS as usize).find(|&n| hit(brightness_rect(n))) {
        return Some(Tap::Brightness(n.checked_sub(1).map(|step| step as u8)));
    }
    if let Some(n) = (0..TIMEOUTS.len()).find(|&n| hit(display_off_rect(n))) {
        return Some(Tap::DisplayOff(TIMEOUTS[n]));
    }
    hit(raise_rect()).then_some(Tap::RaiseToWake)
}

/// A button of a row: a plate in yellow with black text if it is the
/// one chosen, a frame otherwise.
fn button<D: Paint>(target: &mut D, clip: &Rectangle, rect: &Rectangle, text: &str, chosen: bool) {
    let ink = if chosen {
        theme::plate(target, clip, rect, 6, YELLOW);
        BLACK
    } else {
        theme::frame(target, clip, rect, 6, STEEL);
        BONE
    };
    // The label font's letters carry their spacing on the right.
    let x = rect.top_left.x + (rect.size.width as i32 - NW_LABEL.measure(text) + 3) / 2;
    target.text(clip, x, rect.top_left.y + (BUTTON - NW_LABEL.height as i32) / 2, &NW_LABEL, text, ink);
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, settings: &Settings) {
    theme::clear(target, clip);
    theme::title(target, clip, "QUICK.ACCESS", CYAN);

    target.text(clip, LEFT, BRIGHTNESS_Y, &NW_LABEL, "BRIGHTNESS", CYAN);
    const STEPS: [&str; 5] = ["AUTO", "1", "2", "3", "4"];
    for (n, text) in STEPS.iter().enumerate().take(LEVELS as usize + 1) {
        let chosen = settings.brightness.map_or(0, |step| step as usize + 1) == n;
        button(target, clip, &brightness_rect(n), text, chosen);
    }

    target.text(clip, LEFT, DISPLAY_OFF_Y, &NW_LABEL, "DISPLAY OFF AFTER", CYAN);
    const TIMES: [&str; 4] = ["10 S", "15 S", "30 S", "60 S"];
    for (n, text) in TIMES.iter().enumerate() {
        button(target, clip, &display_off_rect(n), text, settings.display_off_s == TIMEOUTS[n]);
    }

    let row = raise_rect();
    theme::rule(target, clip, LEFT, row.top_left.y - 1, WIDTH, STEEL);
    target.text(clip, LEFT, row.top_left.y + 10, &NW_LABEL, "RAISE TO WAKE", BONE);
    theme::switch(target, clip, LEFT + WIDTH - SWITCH.0, row.top_left.y + (44 - SWITCH.1) / 2, SWITCH, settings.raise_to_wake);
    theme::rule(target, clip, LEFT, row.top_left.y + 44, WIDTH, STEEL);

    theme::centred(target, clip, 318, &NW_MONO, "PUSH UP TO CLOSE", CHROME);
    // The bar the panel is held by, and a line along the edge it ends
    // at, which shows while it moves over the face.
    theme::handle(target, clip, SIZE - 34, CYAN);
    let edge = Rectangle::new(Point::new(0, SIZE - 2), Size::new(SIZE as u32, 2));
    let _ = target.fill_solid(&edge.intersection(clip), CYAN);
}
