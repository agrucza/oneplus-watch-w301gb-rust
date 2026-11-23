//! The power page: restart, power off, and the way back to the stock
//! firmware. A tap on a row chooses it; a second tap on the same row
//! does it; a tap anywhere else takes the choice back.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BLACK, BONE, CHROME, CYAN, SIGNAL, SIZE, YELLOW};
use crate::services::font::{Art, Paint};
use crate::services::font_data::{NW_ICON_NEXT, NW_ICON_POWER, NW_ICON_SYNC, NW_MONO, NW_TITLE};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Restart,
    PowerOff,
    Stock,
}

pub const CHOICES: [Choice; 3] = [Choice::Restart, Choice::PowerOff, Choice::Stock];

const TOP: i32 = 112;
const ROW: i32 = 76;
const LEFT: i32 = 62;

/// The rectangle a row covers.
pub fn row_rect(n: usize) -> Rectangle {
    Rectangle::new(Point::new(LEFT, TOP + n as i32 * ROW), Size::new((SIZE - 2 * LEFT) as u32, (ROW - 10) as u32))
}

/// The choice a tap at (x, y) lands on, if any.
pub fn choice_at(x: i32, y: i32) -> Option<Choice> {
    (0..CHOICES.len()).find(|&n| row_rect(n).contains(Point::new(x, y))).map(|n| CHOICES[n])
}

/// A row's colour and icon: cyan for what is harmless, red for what
/// switches the watch off, yellow for what takes it to another firmware.
fn look(choice: Choice) -> (Rgb565, &'static Art) {
    match choice {
        Choice::Restart => (CYAN, &NW_ICON_SYNC),
        Choice::PowerOff => (SIGNAL, &NW_ICON_POWER),
        Choice::Stock => (YELLOW, &NW_ICON_NEXT),
    }
}

/// Draw the part of the page that lies in `clip`. `chosen` is the row
/// tapped once; `stays` says whether a restart comes back to this
/// firmware (it does once a PC has been in touch since the start).
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, chosen: Option<Choice>, stays: bool) {
    let _ = target.fill_solid(clip, BLACK);
    theme::title(target, clip, "POWER", SIGNAL);
    for (n, &choice) in CHOICES.iter().enumerate() {
        let rect = row_rect(n);
        let is_chosen = chosen == Some(choice);
        let (color, icon) = look(choice);
        let (title, note) = match (choice, is_chosen) {
            (Choice::Restart, false) if stays => ("RESTART", "BACK TO THIS FIRMWARE"),
            (Choice::Restart, false) => ("RESTART", "TO STOCK: NO PC SEEN YET"),
            (Choice::PowerOff, false) => ("POWER OFF", "ON: HOLD LOWER BUTTON"),
            (Choice::Stock, false) => ("STOCK FIRMWARE", "TAKES A FEW MINUTES"),
            (_, true) => ("TAP AGAIN", "ANYWHERE ELSE: CANCEL"),
        };
        // What is chosen is a plate in the row's colour with black on it.
        let (ink, dim) = if is_chosen {
            theme::plate(target, clip, &rect, 10, color);
            (BLACK, BLACK)
        } else {
            theme::frame(target, clip, &rect, 10, color);
            (color, CHROME)
        };
        let (x, y) = (rect.top_left.x, rect.top_left.y);
        target.art(clip, x + 18, y + (rect.size.height as i32 - icon.height as i32) / 2, icon, ink);
        target.text(clip, x + 60, y + 1, &NW_TITLE, title, if is_chosen { BLACK } else { BONE });
        target.text(clip, x + 60, y + 36, &NW_MONO, note, dim);
    }
    theme::centred(target, clip, 352, &NW_MONO, "UPPER BUTTON : BACK", CHROME);
}
