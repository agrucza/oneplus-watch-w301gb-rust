//! The pages that have nothing of their own to show yet: the two panels
//! pulled over the watch face, and the two pages a long press opens.
//!
//! The stock app has all four (see docs/hardware-plan.md, 3.1). What
//! they hold there (notifications, quick settings, the faces to choose
//! from, the pages to arrange) this firmware does not have yet, so each
//! only says which one it is and how to leave it.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::face::SIZE;
use super::pages::Page;
use crate::services::font::{Font, Paint};
use crate::services::font_data::{LARGE, MEDIUM};

const TITLE: Rgb565 = Rgb565::WHITE;
const NOTE: Rgb565 = Rgb565::new(18, 37, 18);
const HANDLE: Rgb565 = Rgb565::new(14, 28, 14);

/// How one of these pages looks.
struct Look {
    /// Dark, and not the black of the watch face, so that a panel's edge
    /// shows while it moves.
    background: Rgb565,
    title: &'static str,
    leave: &'static str,
    /// The top edge of a bar at the edge a panel is held by.
    handle: Option<i32>,
}

fn look(page: Page) -> Option<Look> {
    Some(match page {
        Page::Above => Look { background: Rgb565::new(2, 9, 8), title: "Panel from above", leave: "push up to close", handle: Some(SIZE - 36) },
        Page::Below => Look { background: Rgb565::new(7, 8, 2), title: "Panel from below", leave: "push down to close", handle: Some(30) },
        Page::Faces => Look { background: Rgb565::new(5, 4, 8), title: "Watch faces", leave: "tap to go back", handle: None },
        Page::Cards => Look { background: Rgb565::new(2, 10, 3), title: "Pages", leave: "tap to go back", handle: None },
        Page::Face | Page::Status => return None,
    })
}

/// True for the pages drawn here.
pub fn draws(page: Page) -> bool {
    look(page).is_some()
}

/// Draw the part of `page` that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, page: Page) {
    let Some(look) = look(page) else {
        return;
    };
    let _ = target.fill_solid(clip, look.background);
    let centred = |target: &mut D, y: i32, font: &Font, text: &str, color: Rgb565| {
        let x = SIZE / 2 - font.width as i32 * text.len() as i32 / 2;
        target.text(clip, x, y, font, text, color);
    };
    centred(target, 150, &LARGE, look.title, TITLE);
    centred(target, 210, &MEDIUM, "nothing here yet", NOTE);
    centred(target, 250, &MEDIUM, look.leave, NOTE);
    if let Some(y) = look.handle {
        let handle = Rectangle::new(Point::new(SIZE / 2 - 40, y), Size::new(80, 6));
        let _ = target.fill_solid(&handle.intersection(clip), HANDLE);
    }
}
