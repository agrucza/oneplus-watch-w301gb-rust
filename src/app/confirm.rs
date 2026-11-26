//! The confirmation screen: a question with a no and a yes, the same for
//! everything that asks one. What asks gives the words (`Question`),
//! draws with `draw` and reads a tap with `answer_at`.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BLACK, BONE, CHROME, GREEN, SIGNAL};
use crate::services::font::Paint;
use crate::services::font_data::{NW_MONO, NW_TITLE};

/// What is asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Question {
    /// The page's title and its colour: who asks.
    pub title: &'static str,
    pub color: Rgb565,
    /// The question itself, in a few words.
    pub question: &'static str,
    /// Two lines that say what a yes will do.
    pub detail: [&'static str; 2],
    /// The words on the two buttons.
    pub no: &'static str,
    pub yes: &'static str,
    /// A last line, for another way to answer; may be empty.
    pub hint: &'static str,
}

/// The two buttons: no on the left, yes on the right.
pub fn no_rect() -> Rectangle {
    Rectangle::new(Point::new(72, 250), Size::new(146, 70))
}

pub fn yes_rect() -> Rectangle {
    Rectangle::new(Point::new(236, 250), Size::new(146, 70))
}

/// The answer a tap at (x, y) gives, if it is on a button.
pub fn answer_at(x: i32, y: i32) -> Option<bool> {
    let at = Point::new(x, y);
    if yes_rect().contains(at) {
        Some(true)
    } else if no_rect().contains(at) {
        Some(false)
    } else {
        None
    }
}

/// Draw the part of the screen that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, question: &Question) {
    theme::clear(target, clip);
    theme::title(target, clip, question.title, question.color);
    theme::centred(target, clip, 100, &NW_TITLE, question.question, BONE);
    theme::centred(target, clip, 146, &NW_MONO, question.detail[0], CHROME);
    theme::centred(target, clip, 170, &NW_MONO, question.detail[1], CHROME);
    // The no is a frame, the yes a plate: the yes is what stands out.
    for (rect, word, color, filled) in [(no_rect(), question.no, SIGNAL, false), (yes_rect(), question.yes, GREEN, true)] {
        let y = rect.top_left.y + (rect.size.height as i32 - NW_TITLE.height as i32) / 2;
        let x = rect.top_left.x + (rect.size.width as i32 - NW_TITLE.measure(word)) / 2;
        if filled {
            theme::plate(target, clip, &rect, 10, color);
        } else {
            theme::frame(target, clip, &rect, 10, color);
        }
        target.text(clip, x, y, &NW_TITLE, word, if filled { BLACK } else { color });
    }
    theme::centred(target, clip, 346, &NW_MONO, question.hint, CHROME);
}
