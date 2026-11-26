//! The launcher: the page beside the watch face from which every other
//! page is opened, after the design's "radial launcher".
//!
//! The apps stand as tiles on a ring. One of them is chosen: its piece
//! of the outer ring is lit and the middle says which it is. A tap on
//! another tile chooses that one; a tap on the chosen tile or on the
//! middle opens it.
//!
//! The ring has room for eight tiles. With more apps than that it turns
//! under the finger, and tiles come in and go out at the bottom.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::face::{direction, ring_segment};
use super::theme::{self, BLACK, C, CHROME, CYAN, GREEN, INK_3, SIGNAL, STEEL, YELLOW};
use crate::services::font::{Art, Paint};
use crate::services::font_data::{NW_ICON_BELL, NW_ICON_COMPASS, NW_ICON_GEAR, NW_ICON_PLAY, NW_ICON_POWER, NW_ICON_SIGNAL, NW_MONO, NW_TITLE};

/// What the launcher opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum App {
    /// The quick settings (the panel that is also pulled down over the
    /// watch face).
    Settings,
    Alerts,
    Status,
    Compass,
    /// Plays the speaker's test tone; has no page.
    Sound,
    Power,
}

/// The apps in the order they stand on the ring, clockwise from the top.
pub const APPS: [App; 6] = [App::Settings, App::Alerts, App::Status, App::Compass, App::Sound, App::Power];

/// An app's name, icon and colour.
fn look(app: App) -> (&'static str, &'static Art, Rgb565) {
    match app {
        App::Settings => ("SETTINGS", &NW_ICON_GEAR, SIGNAL),
        App::Alerts => ("ALERTS", &NW_ICON_BELL, YELLOW),
        App::Status => ("STATUS", &NW_ICON_SIGNAL, CYAN),
        App::Compass => ("COMPASS", &NW_ICON_COMPASS, CYAN),
        App::Sound => ("SOUND", &NW_ICON_PLAY, GREEN),
        App::Power => ("POWER", &NW_ICON_POWER, SIGNAL),
    }
}

// Angles are tenths of a degree, clockwise from twelve o'clock.
/// From one tile to the next, and how many tiles the ring has room for.
const STEP: i32 = 450;
const ROOM: i32 = 8;
/// The tiles: their distance from the middle, their side and the cut of
/// their corners.
const RING: i32 = 150;
const TILE: i32 = 62;
const CUT: i32 = 10;
/// The outer ring, of which the chosen app's piece is lit: inner and
/// outer radius, and half the piece as an angle.
const OUTER: (i32, i32) = (203, 213);
const PIECE: i32 = 180;
/// A tap this near the middle opens the chosen app.
const MIDDLE: i32 = 86;

pub struct Launcher {
    /// Which of `APPS` is chosen.
    chosen: usize,
    /// How far the ring is turned: the angle that stands at the top.
    turn: i32,
}

/// What a tap did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Nothing,
    /// Another app is chosen: the page is to be drawn again.
    Chosen,
    Open(App),
}

impl Launcher {
    pub const fn new() -> Self {
        Self { chosen: 0, turn: 0 }
    }

    /// The whole way round the ring as the apps stand on it: a full
    /// turn, or more than that with more apps than it has room for.
    fn way_round(count: usize) -> i32 {
        (count as i32).max(ROOM) * STEP
    }

    /// Where app `n` of `count` stands, as an angle from the top between
    /// -180 and 180 degrees; `None` while it is out of sight at the
    /// bottom of a ring too full to show everything.
    fn angle(&self, n: usize, count: usize) -> Option<i32> {
        let round = Self::way_round(count);
        let mut angle = (n as i32 * STEP - self.turn).rem_euclid(round);
        if angle > round / 2 {
            angle -= round;
        }
        // On a ring that shows everything nothing is ever out of sight.
        (round == 3600 || angle.abs() <= 1800 - STEP / 2).then_some(angle)
    }

    /// The middle of the tile of app `n`, if it is in sight.
    fn tile_at(&self, n: usize, count: usize) -> Option<Point> {
        let (dx, dy) = direction(self.angle(n, count)?);
        Some(Point::new(C + dx * RING / 16384, C + dy * RING / 16384))
    }

    /// The rectangle of the tile of app `n` of `APPS`, if it is in sight.
    pub fn tile_rect(&self, n: usize) -> Option<Rectangle> {
        self.tile_at(n, APPS.len()).map(|middle| Rectangle::with_center(middle, Size::new(TILE as u32, TILE as u32)))
    }

    /// A tap at (x, y).
    pub fn tap(&mut self, x: i32, y: i32) -> Outcome {
        let at = Point::new(x, y);
        if let Some(n) = (0..APPS.len()).find(|&n| self.tile_rect(n).is_some_and(|tile| tile.contains(at))) {
            if n == self.chosen {
                return Outcome::Open(APPS[n]);
            }
            self.chosen = n;
            return Outcome::Chosen;
        }
        let (dx, dy) = (x - C, y - C);
        if dx * dx + dy * dy <= MIDDLE * MIDDLE {
            return Outcome::Open(APPS[self.chosen]);
        }
        Outcome::Nothing
    }

    /// Not called yet: with eight apps or fewer the ring does not turn,
    /// and before it does, turning it and swiping back to the watch face
    /// have to be told apart.
    ///
    /// The finger, at (x, y), moved by (dx, dy): a ring with more apps
    /// than it has room for turns with it. True if it turned.
    #[allow(dead_code)]
    pub fn turn_by(&mut self, x: i32, y: i32, dx: i32, dy: i32) -> bool {
        let (rx, ry) = (x - C, y - C);
        let distance = rx * rx + ry * ry;
        if APPS.len() as i32 <= ROOM || distance < MIDDLE * MIDDLE {
            return false;
        }
        // The part of the movement that goes round the middle, as an
        // angle: 573 tenths of a degree to the radian.
        let turned = (rx * dy - ry * dx) * 573 / distance;
        self.turn = (self.turn - turned).rem_euclid(Self::way_round(APPS.len()));
        turned != 0
    }
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, launcher: &Launcher) {
    theme::clear(target, clip);
    let (name, _, color) = look(APPS[launcher.chosen]);
    // The outer ring, and the chosen app's piece of it.
    ring_segment(target, clip, C, C, OUTER.0, OUTER.1, 0, 3600, INK_3);
    if let Some(angle) = launcher.angle(launcher.chosen, APPS.len()) {
        ring_segment(target, clip, C, C, OUTER.0, OUTER.1, angle - PIECE, angle + PIECE, color);
    }
    ring_segment(target, clip, C, C, MIDDLE, MIDDLE + 1, 0, 3600, STEEL);

    for (n, &app) in APPS.iter().enumerate() {
        let Some(tile) = launcher.tile_rect(n) else {
            continue;
        };
        let (_, icon, color) = look(app);
        // The chosen one is a plate with its icon in black.
        let ink = if n == launcher.chosen {
            theme::plate(target, clip, &tile, CUT, color);
            BLACK
        } else {
            theme::frame(target, clip, &tile, CUT, color);
            color
        };
        let at = tile.top_left + Point::new((TILE - icon.width as i32) / 2, (TILE - icon.height as i32) / 2);
        target.art(clip, at.x, at.y, icon, ink);
    }

    // The middle: which app of how many, its name, and what a tap does.
    let mut count = *b"APP 00 / 00";
    let (chosen, all) = (launcher.chosen + 1, APPS.len());
    (count[4], count[5]) = (b'0' + (chosen / 10 % 10) as u8, b'0' + (chosen % 10) as u8);
    (count[9], count[10]) = (b'0' + (all / 10 % 10) as u8, b'0' + (all % 10) as u8);
    theme::centred(target, clip, C - 52, &NW_MONO, core::str::from_utf8(&count).unwrap_or(""), CHROME);
    // The title font's letters carry their spacing on the right.
    target.text(clip, C - (NW_TITLE.measure(name) - 3) / 2, C - 22, &NW_TITLE, name, color);
    theme::centred(target, clip, C + 24, &NW_MONO, "TAP TO OPEN", CHROME);
}
