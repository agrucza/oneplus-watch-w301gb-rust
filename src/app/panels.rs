//! The pages that have nothing of their own to show yet: the two pages a
//! long press opens.
//!
//! The stock app has all four (see docs/hardware-plan.md, 3.1). What
//! they hold there (notifications, quick settings, the faces to choose
//! from, the pages to arrange) this firmware does not have yet, so each
//! only says which one it is and how to leave it. The names and colours
//! are the design's.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::pages::Page;
use super::theme::{self, C, CHROME, GREEN, SIGNAL};
use crate::services::font::{Art, Paint};
use crate::services::font_data::{NW_ICON_DIAMOND, NW_ICON_GRID, NW_MONO};

/// How one of these pages looks.
struct Look {
    title: &'static str,
    color: Rgb565,
    icon: &'static Art,
    leave: &'static str,
}

fn look(page: Page) -> Option<Look> {
    Some(match page {
        Page::Faces => Look { title: "FACES", color: SIGNAL, icon: &NW_ICON_DIAMOND, leave: "TAP TO GO BACK" },
        Page::Cards => Look { title: "PAGES", color: GREEN, icon: &NW_ICON_GRID, leave: "TAP TO GO BACK" },
        // The panels are the settings (`super::settings`) and the alerts
        // (`super::alerts`).
        Page::Face | Page::Launcher | Page::Status | Page::Above | Page::Below | Page::Power | Page::Picker | Page::Settings | Page::Compass | Page::Sounds | Page::Transfer | Page::Gps | Page::Pair | Page::Devices => return None,
    })
}

/// True for the pages drawn here.
pub fn draws(page: Page) -> bool {
    look(page).is_some()
}

/// The box the page's icon sits in.
const BOX: i32 = 92;
const BOX_Y: i32 = 124;

/// Draw the part of `page` that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, page: Page) {
    let Some(look) = look(page) else {
        return;
    };
    let _ = target.fill_solid(clip, theme::BLACK);
    theme::title(target, clip, look.title, look.color);
    let frame = Rectangle::new(Point::new(C - BOX / 2, BOX_Y), Size::new(BOX as u32, BOX as u32));
    theme::frame(target, clip, &frame, 16, look.color);
    target.art(clip, C - look.icon.width as i32 / 2, BOX_Y + (BOX - look.icon.height as i32) / 2, look.icon, look.color);
    theme::centred(target, clip, BOX_Y + BOX + 30, &NW_MONO, "// NOTHING HERE YET", theme::BONE);
    theme::centred(target, clip, BOX_Y + BOX + 62, &NW_MONO, look.leave, CHROME);
}
