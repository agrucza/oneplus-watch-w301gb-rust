//! The power page: restart, power off, and the way back to the stock
//! firmware. A tap on a row chooses it; a second tap on the same row
//! does it; a tap anywhere else takes the choice back.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::face::SIZE;
use crate::services::font::{Font, Paint};
use crate::services::font_data::{LARGE, MEDIUM, SMALL};

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

const BACKGROUND: Rgb565 = Rgb565::BLACK;
const ROW_COLOR: Rgb565 = Rgb565::new(4, 9, 5);
const CHOSEN: Rgb565 = Rgb565::new(24, 14, 0);
const TEXT: Rgb565 = Rgb565::WHITE;
const DIM: Rgb565 = Rgb565::new(13, 26, 13);

/// Draw the part of the page that lies in `clip`. `chosen` is the row
/// tapped once; `stays` says whether a restart comes back to this
/// firmware (it does once a PC has been in touch since the start).
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, chosen: Option<Choice>, stays: bool) {
    let _ = target.fill_solid(clip, BACKGROUND);
    let centred = |target: &mut D, y: i32, font: &Font, text: &str, color: Rgb565| {
        let x = SIZE / 2 - font.width as i32 * text.len() as i32 / 2;
        target.text(clip, x, y, font, text, color);
    };
    centred(target, 56, &MEDIUM, "Power", DIM);
    for (n, &choice) in CHOICES.iter().enumerate() {
        let rect = row_rect(n);
        let is_chosen = chosen == Some(choice);
        let _ = target.fill_solid(&rect.intersection(clip), if is_chosen { CHOSEN } else { ROW_COLOR });
        let (title, note) = match (choice, is_chosen) {
            (Choice::Restart, false) if stays => ("Restart", "comes back to this firmware"),
            (Choice::Restart, false) => ("Restart", "goes to stock: no PC seen yet"),
            (Choice::PowerOff, false) => ("Power off", "on: hold the lower button"),
            (Choice::Stock, false) => ("Stock firmware", "takes a few minutes"),
            (_, true) => ("Tap again", "anywhere else: cancel"),
        };
        centred(target, rect.top_left.y + 6, &LARGE, title, TEXT);
        centred(target, rect.top_left.y + 42, &SMALL, note, if is_chosen { TEXT } else { DIM });
    }
    centred(target, 356, &SMALL, "upper button: back", DIM);
}
