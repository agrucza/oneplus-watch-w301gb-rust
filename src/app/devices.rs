//! The page of paired devices, opened from the pairing page: one device
//! at a time, with a button to the next and one to remove it. Removing
//! asks first, on the confirmation screen.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::confirm::{self, Question};
use super::theme::{self, BLACK, BONE, CHROME, CYAN, SIGNAL, STEEL, YELLOW};
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_MONO, NW_TITLE};

/// What the page shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct View {
    /// How many devices there are, and which of them is shown (from 0).
    pub count: u8,
    pub index: u8,
    /// The stock firmware's type of the one shown, and its address.
    pub kind: u8,
    pub address: [u8; 6],
    /// It opened the serial port since the start, and no other entry
    /// is the same device: removing it locks that PC out.
    pub serial: bool,
    /// The question whether to remove it is up.
    pub asking: bool,
    /// The table could not be written when it last changed.
    pub unsaved: bool,
}

pub fn next_rect() -> Rectangle {
    Rectangle::new(Point::new(72, 270), Size::new(146, 66))
}

pub fn remove_rect() -> Rectangle {
    Rectangle::new(Point::new(236, 270), Size::new(146, 66))
}

/// What a tap does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tap {
    Next,
    /// Ask whether to remove the device shown.
    Ask,
    /// The answer to that question.
    Answer(bool),
}

/// What a tap at (x, y) does. The last device cannot be removed:
/// without any, Bluetooth is not started at all.
pub fn tap_at(x: i32, y: i32, view: &View) -> Option<Tap> {
    let at = Point::new(x, y);
    if view.asking {
        confirm::answer_at(x, y).map(Tap::Answer)
    } else if view.count > 1 && next_rect().contains(at) {
        Some(Tap::Next)
    } else if view.count > 1 && remove_rect().contains(at) {
        Some(Tap::Ask)
    } else {
        None
    }
}

/// The question before a device is removed.
pub fn question(view: &View) -> Question {
    let detail = if view.serial { ["THIS IS THE PC THAT SENDS", "FIRMWARE AND FETCHES LOGS"] } else { ["IT WILL HAVE TO BE", "PAIRED AGAIN"] };
    Question { title: "BLUETOOTH", color: SIGNAL, question: "REMOVE DEVICE?", detail, no: "KEEP", yes: "REMOVE", hint: "", number: None, danger: true }
}

/// An address as it is written, in capitals.
fn written(address: [u8; 6]) -> [u8; 17] {
    let mut text = [b':'; 17];
    for (i, byte) in address.iter().enumerate() {
        text[3 * i] = b"0123456789ABCDEF"[(byte >> 4) as usize];
        text[3 * i + 1] = b"0123456789ABCDEF"[(byte & 15) as usize];
    }
    text
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, view: &View) {
    theme::clear(target, clip);
    if view.asking {
        confirm::draw(target, clip, &question(view));
        // Which one: its address, where a number to compare would stand.
        let text = written(view.address);
        theme::centred(target, clip, 204, &NW_LABEL, core::str::from_utf8(&text).unwrap_or(""), BONE);
        return;
    }
    theme::title(target, clip, "PAIRED DEVICES", CYAN);
    if view.count == 0 {
        theme::centred(target, clip, 150, &NW_TITLE, "NONE", CHROME);
        return;
    }
    let digit = |n: u8| b'0' + n.min(9);
    let place = [digit(view.index + 1), b' ', b'O', b'F', b' ', digit(view.count)];
    theme::centred(target, clip, 78, &NW_TITLE, core::str::from_utf8(&place).unwrap_or(""), BONE);
    // The stock firmware's types; what is paired here gets the third.
    let kind = match view.kind {
        0 => "PHONE",
        1 => "PHONE, LOW ENERGY",
        _ => "DEVICE",
    };
    theme::centred(target, clip, 130, &NW_MONO, kind, CHROME);
    let text = written(view.address);
    theme::centred(target, clip, 158, &NW_LABEL, core::str::from_utf8(&text).unwrap_or(""), BONE);
    if view.serial {
        theme::centred(target, clip, 200, &NW_MONO, "THE PC THAT SENDS FIRMWARE", YELLOW);
    }
    if view.unsaved {
        theme::centred(target, clip, 226, &NW_MONO, "LAST CHANGE NOT WRITTEN", SIGNAL);
    }
    if view.count < 2 {
        theme::centred(target, clip, 290, &NW_MONO, "THE LAST DEVICE STAYS", CHROME);
        return;
    }
    for (rect, word, color, filled) in [(next_rect(), "NEXT", STEEL, false), (remove_rect(), "REMOVE", SIGNAL, true)] {
        let y = rect.top_left.y + (rect.size.height as i32 - NW_TITLE.height as i32) / 2;
        let x = rect.top_left.x + (rect.size.width as i32 - NW_TITLE.measure(word)) / 2;
        if filled {
            theme::plate(target, clip, &rect, 10, color);
        } else {
            theme::frame(target, clip, &rect, 10, color);
        }
        target.text(clip, x, y, &NW_TITLE, word, if filled { BLACK } else { BONE });
    }
}
