//! The GPS page: what the satellite receiver reports while it searches
//! and once it has a position. The receiver runs from the opening of
//! the page until it is left, through a dark display too.

use core::fmt::Write as _;

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BONE, CHROME, CYAN, GREEN, STEEL, STEEL_2, YELLOW};
use crate::drivers::apollo::messages::Gps;
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_MONO, NW_TITLE};

/// What the page shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct View {
    /// The receiver's latest report; none before its first.
    pub report: Option<Gps>,
    /// Seconds since the receiver was switched on.
    pub seconds: u32,
    /// The receiver ran its longest and was switched off.
    pub stopped: bool,
}

/// A line of text put together on the spot.
struct Line {
    bytes: [u8; 40],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Self { bytes: [0; 40], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let n = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&text.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Degrees in ten-millionths as "52.51630 N": five places, which is
/// about a metre.
pub fn degrees(value_e7: i32, positive: char, negative: char, into: &mut impl core::fmt::Write) {
    let size = value_e7.unsigned_abs();
    let _ = write!(into, "{}.{:05} {}", size / 10_000_000, size % 10_000_000 / 100, if value_e7 < 0 { negative } else { positive });
}

const BARS_LEFT: i32 = 96;
const BAR_STEP: i32 = 22;
const BAR_WIDTH: i32 = 16;
const BARS_BASE: i32 = 262;
/// Two pixels for a dB-Hz of signal, up to 50.
const BAR_MOST: u8 = 50;

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, view: &View) {
    theme::clear(target, clip);
    theme::title(target, clip, "GPS", CYAN);
    let report = view.report.unwrap_or_default();
    let (state, color) = match view.report {
        None if view.stopped => ("STOPPED", CHROME),
        None => ("STARTING", CHROME),
        Some(r) if r.fix => ("POSITION", GREEN),
        Some(_) => ("SEARCHING", YELLOW),
    };
    theme::centred(target, clip, 78, &NW_TITLE, state, color);
    let mut line = Line::new();
    let _ = write!(line, "{} SATELLITES IN VIEW, {} USED", report.in_view, report.used);
    theme::centred(target, clip, 122, &NW_MONO, line.as_str(), BONE);

    // The strongest satellites' signals as bars: green is good enough
    // for a position, yellow weak, grey hardly there.
    theme::rule(target, clip, BARS_LEFT - 6, BARS_BASE, 12 * BAR_STEP + 6, STEEL);
    for (n, &[_, signal]) in report.strongest.iter().enumerate() {
        if signal == 0 {
            continue;
        }
        let height = 2 * signal.min(BAR_MOST) as i32;
        let bar = Rectangle::new(Point::new(BARS_LEFT + n as i32 * BAR_STEP, BARS_BASE - height), Size::new(BAR_WIDTH as u32, height as u32));
        let color = if signal >= 30 { GREEN } else if signal >= 20 { YELLOW } else { STEEL_2 };
        let _ = target.fill_solid(&bar.intersection(clip), color);
    }

    if report.fix {
        let mut line = Line::new();
        degrees(report.lat_e7, 'N', 'S', &mut line);
        theme::centred(target, clip, 276, &NW_LABEL, line.as_str(), BONE);
        let mut line = Line::new();
        degrees(report.lon_e7, 'E', 'W', &mut line);
        theme::centred(target, clip, 306, &NW_LABEL, line.as_str(), BONE);
        let mut line = Line::new();
        let _ = write!(line, "{}.{} KM/H, FOUND IN {} S", report.speed_dmh / 10, report.speed_dmh % 10, report.ttff_ms / 1000);
        theme::centred(target, clip, 342, &NW_MONO, line.as_str(), CHROME);
    } else if view.stopped {
        theme::centred(target, clip, 280, &NW_MONO, "SWITCHED OFF AFTER 15 MINUTES", CHROME);
        theme::centred(target, clip, 306, &NW_MONO, "OPEN THE PAGE AGAIN TO SEARCH", CHROME);
    } else {
        theme::centred(target, clip, 280, &NW_MONO, "NEEDS OPEN SKY OR A WINDOW", CHROME);
        theme::centred(target, clip, 306, &NW_MONO, "THE FIRST POSITION TAKES MINUTES", CHROME);
        let mut line = Line::new();
        let _ = write!(line, "ON FOR {} S", view.seconds);
        theme::centred(target, clip, 342, &NW_MONO, line.as_str(), color);
    }
}
