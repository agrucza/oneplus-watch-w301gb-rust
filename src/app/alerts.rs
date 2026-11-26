//! What the watch has to tell its wearer, newest first, and the page
//! that lists it: the panel pulled up over the watch face, the design's
//! "alerts".
//!
//! The one thing that raises an alert so far is the Apollo's reminder to
//! move. The list is kept in memory only and starts empty.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BONE, CHROME, GREEN, SIZE, YELLOW};
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_MONO};

/// What an alert is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The Apollo's sedentary reminder: sitting for long.
    Move,
    /// The battery has got low.
    LowBattery,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Alert {
    pub kind: Kind,
    /// When it came, as hour and minute; `None` without a clock.
    pub at: Option<(u8, u8)>,
}

/// How many alerts the page has room for.
pub const SHOWN: usize = 3;

/// The latest alerts, newest first.
#[derive(Clone, Copy)]
pub struct Alerts {
    list: [Option<Alert>; SHOWN],
}

impl Alerts {
    pub const fn new() -> Self {
        Self { list: [None; SHOWN] }
    }

    /// Put `alert` at the top; the oldest drops out.
    pub fn add(&mut self, alert: Alert) {
        self.list.rotate_right(1);
        self.list[0] = Some(alert);
    }

    pub fn count(&self) -> usize {
        self.list.iter().flatten().count()
    }
}

/// An alert's colour, title and text.
fn look(kind: Kind) -> (Rgb565, &'static str, &'static str) {
    match kind {
        Kind::Move => (GREEN, "TIME TO MOVE", "SITTING FOR A WHILE"),
        Kind::LowBattery => (YELLOW, "LOW BATTERY", "CHARGE SOON"),
    }
}

/// The card of alert `n`.
pub fn card_rect(n: usize) -> Rectangle {
    Rectangle::new(Point::new(62, 86 + n as i32 * 92), Size::new((SIZE - 124) as u32, 82))
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, alerts: &Alerts) {
    theme::clear(target, clip);
    theme::title(target, clip, "ALERTS", YELLOW);
    for (n, alert) in alerts.list.iter().flatten().enumerate() {
        let (color, title, text) = look(alert.kind);
        let card = card_rect(n);
        theme::frame(target, clip, &card, 10, color);
        let (x, y) = (card.top_left.x + 16, card.top_left.y);
        target.text(clip, x, y + 4, &NW_LABEL, title, color);
        target.text(clip, x, y + 28, &NW_MONO, text, BONE);
        let at = time_text(alert.at);
        target.text(clip, x, y + 52, &NW_MONO, core::str::from_utf8(&at).unwrap_or(""), CHROME);
    }
    if alerts.count() == 0 {
        theme::centred(target, clip, 200, &NW_MONO, "// NO ALERTS", BONE);
    }
    theme::centred(target, clip, 372, &NW_MONO, "PUSH DOWN TO CLOSE", CHROME);
    // The bar the panel is held by, and a line along the edge it ends
    // at, which shows while it moves over the face.
    theme::handle(target, clip, 14, YELLOW);
    let edge = Rectangle::new(Point::zero(), Size::new(SIZE as u32, 2));
    let _ = target.fill_solid(&edge.intersection(clip), YELLOW);
}

/// ">> 14:05", or ">> --:--" without a clock.
fn time_text(at: Option<(u8, u8)>) -> [u8; 8] {
    let mut text = *b">> --:--";
    if let Some((hour, minute)) = at {
        text[3] = b'0' + hour / 10 % 10;
        text[4] = b'0' + hour % 10;
        text[6] = b'0' + minute / 10 % 10;
        text[7] = b'0' + minute % 10;
    }
    text
}
