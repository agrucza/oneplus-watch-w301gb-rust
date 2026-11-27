//! The page for pairing a new device over Bluetooth, opened from the
//! settings. Only while it is shown can the watch be found by devices
//! that do not know it; a device that then asks is put to the wearer
//! on the confirmation screen, with the number to compare.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BLACK, BONE, CHROME, CYAN, GREEN, SIGNAL, YELLOW};
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_MONO, NW_TITLE};

/// How far a pairing is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// The chip has not been told yet.
    Starting,
    /// The watch can be found; nothing has asked.
    Ready,
    /// The wearer said yes; the device has not finished.
    Accepted,
    /// The wearer said no, or did not answer in time.
    Denied,
    /// The chip handed over the device's record.
    Paired,
    /// The chip said the pairing failed.
    Failed,
    /// No place is free for another device.
    Full,
    /// Paired, but the table could not be written.
    NotKept,
}

/// What the page shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct View {
    pub state: State,
    /// The last four digits of the name the watch is found under; zeros
    /// while not known.
    pub suffix: [u8; 4],
}

/// The name the watch is found under, as the stock firmware builds it.
pub const NAME: &str = "OnePlus Watch ";

/// The button that leads to the page of paired devices.
pub fn devices_rect() -> Rectangle {
    Rectangle::new(Point::new(127, 300), Size::new(200, 56))
}

pub fn devices_at(x: i32, y: i32) -> bool {
    devices_rect().contains(Point::new(x, y))
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, view: &View) {
    theme::clear(target, clip);
    theme::title(target, clip, "PAIR DEVICE", CYAN);
    let (state, color, lines) = match view.state {
        State::Starting => ("STARTING", CHROME, ["", ""]),
        State::Ready => ("READY", CYAN, ["SEARCH FOR THIS WATCH", "ON THE OTHER DEVICE"]),
        State::Accepted => ("ACCEPTED", YELLOW, ["WAITING FOR THE", "DEVICE TO FINISH"]),
        State::Denied => ("DENIED", SIGNAL, ["THE DEVICE WAS", "NOT PAIRED"]),
        State::Paired => ("PAIRED", GREEN, ["THE DEVICE IS KEPT", ""]),
        State::Failed => ("FAILED", SIGNAL, ["THE PAIRING DID", "NOT GO THROUGH"]),
        State::Full => ("FULL", SIGNAL, ["NO PLACE IS FREE:", "REMOVE A DEVICE FIRST"]),
        State::NotKept => ("NOT KEPT", SIGNAL, ["PAIRED, BUT NOT STORED:", "GONE AFTER A RESTART"]),
    };
    theme::centred(target, clip, 78, &NW_TITLE, state, color);
    theme::centred(target, clip, 124, &NW_MONO, lines[0], CHROME);
    theme::centred(target, clip, 146, &NW_MONO, lines[1], CHROME);
    if view.suffix != [0; 4] && view.state != State::Full {
        theme::centred(target, clip, 184, &NW_MONO, "THE WATCH IS FOUND AS", CHROME);
        let mut name = [0u8; 18];
        name[..14].copy_from_slice(NAME.as_bytes());
        name[14..].copy_from_slice(&view.suffix);
        // The label font has capitals only; the name is shown in them.
        name.make_ascii_uppercase();
        theme::centred(target, clip, 208, &NW_LABEL, core::str::from_utf8(&name).unwrap_or(""), BONE);
        theme::centred(target, clip, 252, &NW_MONO, "WHILE THIS PAGE IS SHOWN, 2 MIN", CHROME);
    }
    let button = devices_rect();
    theme::plate(target, clip, &button, 10, CYAN);
    let y = button.top_left.y + (button.size.height as i32 - NW_TITLE.height as i32) / 2;
    theme::centred(target, clip, y, &NW_TITLE, "DEVICES", BLACK);
}
