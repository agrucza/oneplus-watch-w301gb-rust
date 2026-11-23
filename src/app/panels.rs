//! The two panels that are pulled over the watch face: one from the top
//! edge, one from the bottom edge.
//!
//! The stock app has two such panels on its watch face (see
//! docs/hardware-plan.md, 3.1). What they hold there (notifications,
//! quick settings) this firmware does not have yet, so each panel only
//! says which one it is.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::face::SIZE;
use crate::services::font::{Font, Paint};
use crate::services::font_data::{LARGE, MEDIUM};

/// The panels' backgrounds: dark, and not the black of the watch face, so
/// that a panel's edge shows while it moves.
const ABOVE: Rgb565 = Rgb565::new(2, 9, 8);
const BELOW: Rgb565 = Rgb565::new(7, 8, 2);
const TITLE: Rgb565 = Rgb565::WHITE;
const NOTE: Rgb565 = Rgb565::new(18, 37, 18);
const HANDLE: Rgb565 = Rgb565::new(14, 28, 14);

/// Draw the part of a panel that lies in `clip`: the one from the top
/// edge (`from_top`) or the one from the bottom edge.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, from_top: bool) {
    let _ = target.fill_solid(clip, if from_top { ABOVE } else { BELOW });
    let centred = |target: &mut D, y: i32, font: &Font, text: &str, color: Rgb565| {
        let x = SIZE / 2 - font.width as i32 * text.len() as i32 / 2;
        target.text(clip, x, y, font, text, color);
    };
    centred(target, 150, &LARGE, if from_top { "Panel from above" } else { "Panel from below" }, TITLE);
    centred(target, 210, &MEDIUM, "nothing here yet", NOTE);
    centred(target, 250, &MEDIUM, if from_top { "push up to close" } else { "push down to close" }, NOTE);
    // A bar at the edge the panel is held by: the one that came in last.
    let handle = Rectangle::new(Point::new(SIZE / 2 - 40, if from_top { SIZE - 36 } else { 30 }), Size::new(80, 6));
    let _ = target.fill_solid(&handle.intersection(clip), HANDLE);
}
