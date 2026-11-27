//! The transfer page: what is going over Bluetooth, how far it is, and a
//! way to end it; before that, the question whether it may happen at
//! all, on the confirmation screen. It comes up by itself whenever a firmware package
//! arrives or a file is sent to the PC, so that neither happens behind
//! the wearer's back.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::confirm::{self, Question};
use super::theme::{self, BLACK, BONE, CHROME, CYAN, SIGNAL, STEEL, YELLOW};
use crate::services::font::Paint;
use crate::services::font_data::{NW_MONO, NW_NUMBER, NW_TITLE};
use crate::services::progress::Transfer;

const BAR: Rectangle = Rectangle::new(Point::new(87, 226), Size::new(280, 14));

/// The button that ends the transfer.
pub fn cancel_rect() -> Rectangle {
    Rectangle::new(Point::new(127, 290), Size::new(200, 62))
}

/// The question an offer puts to the wearer, for the confirmation
/// screen (`super::confirm`); `None` for a transfer under way. `number`
/// is the one a pairing is to be compared by.
pub fn question(transfer: Transfer, number: u32) -> Option<Question> {
    let (color, question, detail) = match transfer {
        Transfer::Offered => (YELLOW, "NEW FIRMWARE?", ["A PC ASKS TO SEND", "FIRMWARE TO THIS WATCH"]),
        Transfer::OfferedGet => (CYAN, "TRANSFER FILE?", ["A PC ASKS FOR A FILE", "FROM THIS WATCH"]),
        Transfer::OfferedPut => (CYAN, "TRANSFER FILE?", ["A PC ASKS TO WRITE", "A FILE TO THIS WATCH"]),
        Transfer::OfferedPairing => (CYAN, "PAIR DEVICE?", ["ACCEPT ONLY IF THE DEVICE", "SHOWS THIS NUMBER"]),
        Transfer::Receiving | Transfer::Sending | Transfer::Waiting => return None,
    };
    let number = (transfer == Transfer::OfferedPairing).then_some(number);
    Some(Question { title: "BLUETOOTH", color, question, detail, no: "DENY", yes: "ACCEPT", hint: "UPPER BUTTON : ACCEPT", number, danger: false })
}

/// Whether a tap at (x, y) is on the button.
pub fn cancel_at(x: i32, y: i32) -> bool {
    cancel_rect().contains(Point::new(x, y))
}

/// Draw the part of the page that lies in `clip`: the transfer and how
/// many percent of it are through; for a pairing, its number.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, transfer: Transfer, percent: u8, number: u32) {
    theme::clear(target, clip);
    if let Some(question) = question(transfer, number) {
        confirm::draw(target, clip, &question);
        return;
    }
    let (color, what, note) = match transfer {
        Transfer::Receiving => (YELLOW, "RECEIVING FIRMWARE", "FROM A PC OVER BLUETOOTH"),
        Transfer::Waiting => (CYAN, "ACCEPTED", "WAITING FOR THE PC TO BEGIN"),
        _ => (CYAN, "SENDING A FILE", "TO A PC OVER BLUETOOTH"),
    };
    theme::title(target, clip, "BLUETOOTH", color);
    theme::centred(target, clip, 92, &NW_TITLE, what, BONE);
    theme::centred(target, clip, 128, &NW_MONO, note, CHROME);
    // The number font has the digits and the colon only.
    // Nothing to count while the PC has not begun.
    if transfer != Transfer::Waiting {
        let percent = percent.min(100);
        let digits = [b'0' + percent / 100, b'0' + percent / 10 % 10, b'0' + percent % 10];
        let from = if percent >= 100 { 0 } else if percent >= 10 { 1 } else { 2 };
        theme::centred(target, clip, 166, &NW_NUMBER, core::str::from_utf8(&digits[from..]).unwrap_or(""), color);
        theme::frame(target, clip, &BAR, 4, STEEL);
        let done = (BAR.size.width as i32 - 4) * percent as i32 / 100;
        if done > 0 {
            let filled = Rectangle::new(BAR.top_left + Point::new(2, 2), Size::new(done as u32, BAR.size.height - 4));
            let _ = target.fill_solid(&filled.intersection(clip), color);
        }
    }
    let button = cancel_rect();
    theme::plate(target, clip, &button, 10, SIGNAL);
    let y = button.top_left.y + (button.size.height as i32 - NW_TITLE.height as i32) / 2;
    theme::centred(target, clip, y, &NW_TITLE, "CANCEL", BLACK);
}
