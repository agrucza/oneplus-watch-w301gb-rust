//! The pages that have nothing of their own to show yet: the two panels
//! pulled over the watch face, and the two pages a long press opens.
//!
//! The stock app has all four (see docs/hardware-plan.md, 3.1). What
//! they hold there (notifications, quick settings, the faces to choose
//! from, the pages to arrange) this firmware does not have yet, so each
//! only says which one it is and how to leave it. The names and colours
//! are the design's: quick access is pulled down from the top there.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::pages::Page;
use super::theme::{self, C, CHROME, CYAN, GREEN, SIGNAL, SIZE, YELLOW};
use crate::services::font::{Art, Paint};
use crate::services::font_data::{NW_ICON_BELL, NW_ICON_DIAMOND, NW_ICON_GEAR, NW_ICON_GRID, NW_MONO};

/// How one of these pages looks.
struct Look {
    title: &'static str,
    color: Rgb565,
    icon: &'static Art,
    leave: &'static str,
    /// The top edge of the bar at the edge a panel is held by, and the
    /// top of the line along that edge, which shows where the panel
    /// ends while it moves over the face.
    handle: Option<(i32, i32)>,
}

fn look(page: Page) -> Option<Look> {
    Some(match page {
        Page::Above => Look { title: "QUICK.ACCESS", color: CYAN, icon: &NW_ICON_GEAR, leave: "PUSH UP TO CLOSE", handle: Some((SIZE - 34, SIZE - 2)) },
        Page::Below => Look { title: "ALERTS", color: YELLOW, icon: &NW_ICON_BELL, leave: "PUSH DOWN TO CLOSE", handle: Some((14, 0)) },
        Page::Faces => Look { title: "FACES", color: SIGNAL, icon: &NW_ICON_DIAMOND, leave: "TAP TO GO BACK", handle: None },
        Page::Cards => Look { title: "PAGES", color: GREEN, icon: &NW_ICON_GRID, leave: "TAP TO GO BACK", handle: None },
        Page::Face | Page::Status | Page::Power => return None,
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
    if let Some((bar, edge)) = look.handle {
        theme::handle(target, clip, bar, look.color);
        let line = Rectangle::new(Point::new(0, edge), Size::new(SIZE as u32, 2));
        let _ = target.fill_solid(&line.intersection(clip), look.color);
    }
}
