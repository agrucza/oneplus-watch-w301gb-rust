//! The sounds app: the watch's own sound files, to be played by a tap.
//!
//! The files are the stock firmware's, on the storage chip. A tap on a
//! row plays it; a tap while something plays ends that.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BLACK, BONE, CHROME, GREEN, SIZE, STEEL};
use crate::services::font::Paint;
use crate::services::font_data::{NW_ICON_PLAY, NW_LABEL, NW_MONO};

/// The sounds: what the row says, and the file on the image volume.
pub const SOUNDS: [(&str, &str); 6] = [
    ("ALARM", "packetA/audio/alarm_clock.wav"),
    ("CALL", "packetA/audio/calling.wav"),
    ("MESSAGE", "packetA/audio/sms.wav"),
    ("LOW BATTERY", "packetA/audio/low_battery.wav"),
    ("CHARGING", "packetA/audio/connect_charging.wav"),
    ("TEST TONE", "packetA/audio/spkT.wav"),
];

/// The row after the sounds: record two seconds from the microphone and
/// play them back. As a number it follows the sounds'; the one after it
/// stands for playing the recording.
pub const RECORD: usize = SOUNDS.len();
pub const REPLAY: usize = RECORD + 1;
const ROWS: usize = SOUNDS.len() + 1;

/// The sounds that tell of something, as numbers in `SOUNDS`.
pub const MESSAGE: usize = 2;
pub const LOW_BATTERY: usize = 3;
pub const CHARGING: usize = 4;

const LEFT: i32 = 72;
const WIDTH: i32 = SIZE - 2 * LEFT;
const TOP: i32 = 72;
const ROW: i32 = 42;

/// The rectangle of row `n`.
pub fn row_rect(n: usize) -> Rectangle {
    Rectangle::new(Point::new(LEFT, TOP + n as i32 * ROW), Size::new(WIDTH as u32, ROW as u32 - 6))
}

/// The sound a tap at (x, y) lands on, if any.
pub fn tap_at(x: i32, y: i32) -> Option<usize> {
    (0..ROWS).find(|&n| row_rect(n).contains(Point::new(x, y)))
}

/// Draw the part of the page that lies in `clip`; `playing` is the sound
/// on its way, if one is.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, playing: Option<usize>) {
    theme::clear(target, clip);
    theme::title(target, clip, "SOUNDS", GREEN);
    for (n, label) in SOUNDS.iter().map(|(label, _)| *label).chain(["RECORD 2 S"]).enumerate() {
        let row = row_rect(n);
        // What plays is a plate in green with black on it.
        let ink = if playing == Some(n) {
            theme::plate(target, clip, &row, 8, GREEN);
            BLACK
        } else {
            theme::frame(target, clip, &row, 8, STEEL);
            BONE
        };
        let (x, y) = (row.top_left.x, row.top_left.y);
        let height = row.size.height as i32;
        target.art(clip, x + 12, y + (height - NW_ICON_PLAY.height as i32) / 2, &NW_ICON_PLAY, if playing == Some(n) { BLACK } else { GREEN });
        target.text(clip, x + 52, y + (height - NW_LABEL.height as i32) / 2, &NW_LABEL, label, ink);
    }
    let hint = if playing.is_some() { "TAP TO STOP" } else { "TAP TO PLAY" };
    theme::centred(target, clip, TOP + ROWS as i32 * ROW + 6, &NW_MONO, hint, CHROME);
}
