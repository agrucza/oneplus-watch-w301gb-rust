//! The look every page shares, after the "Nightwatch OS" design: its
//! colours, and its parts (page title, tag, chamfered frame and plate,
//! handle bar) drawn with the design's three fonts.
//!
//! The design is made for small web pictures; its measures are scaled to
//! this panel here (text of 9 to 12 pixels there is 17 to 19 here, a
//! title 28). Its glows are only where a bitmap for them exists (the big
//! digits of the watch face); its scan lines and vignette are left out.
//!
//! Like the pages, this draws into any `Paint` target, so the preview on
//! the PC shows the same pictures.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};

use crate::services::font::{Font, Paint};
use crate::services::font_data::{NW_LABEL, NW_TITLE};

/// The picture is `SIZE` x `SIZE`, round, centred on (`C`, `C`).
pub const SIZE: i32 = 454;
pub const C: i32 = SIZE / 2;

/// A colour of the design, from its 24-bit value.
pub const fn rgb(hex: u32) -> Rgb565 {
    Rgb565::new((hex >> 19) as u8 & 31, (hex >> 10) as u8 & 63, (hex >> 3) as u8 & 31)
}

// The design's colours (`colors_and_type.css`). Signal red is the main
// one; cyan for labels; yellow for what is chosen and for warnings; green
// for what is well.
pub const BLACK: Rgb565 = Rgb565::BLACK;
pub const SIGNAL: Rgb565 = rgb(0xFF003C);
pub const SIGNAL_HOT: Rgb565 = rgb(0xFF3355);
pub const CYAN: Rgb565 = rgb(0x00F0FF);
pub const YELLOW: Rgb565 = rgb(0xFFEE00);
pub const GREEN: Rgb565 = rgb(0x00FF9C);
/// A slightly lifted surface: the track of an arc.
pub const INK: Rgb565 = rgb(0x0B0D12);
/// Borders and dividers.
pub const STEEL: Rgb565 = rgb(0x2A2F3A);
/// Text that is switched off.
pub const STEEL_2: Rgb565 = rgb(0x474D5A);
/// Muted text.
pub const CHROME: Rgb565 = rgb(0x8A93A3);
/// Body text.
pub const BONE: Rgb565 = rgb(0xE6E9EE);

/// The whole picture.
pub fn whole() -> Rectangle {
    Rectangle::new(Point::zero(), Size::new(SIZE as u32, SIZE as u32))
}

/// Paint `area` black: the background of every page.
pub fn clear<D: Paint>(target: &mut D, area: &Rectangle) {
    let _ = target.fill_solid(area, BLACK);
}

/// `text` with its middle at the middle of the screen.
pub fn centred<D: Paint>(target: &mut D, clip: &Rectangle, y: i32, font: &Font, text: &str, color: Rgb565) {
    target.text(clip, C - font.measure(text) / 2, y, font, text, color);
}

/// A line one pixel high from `x` to `x + width`.
pub fn rule<D: Paint>(target: &mut D, clip: &Rectangle, x: i32, y: i32, width: i32, color: Rgb565) {
    let line = Rectangle::new(Point::new(x, y), Size::new(width.max(0) as u32, 1));
    let _ = target.fill_solid(&line.intersection(clip), color);
}

/// Where a page's title stands and where the line under it runs.
pub const TITLE_Y: i32 = 28;
pub const TITLE_RULE_Y: i32 = 66;
const TITLE_RULE_HALF: i32 = 104;

/// The head of a page: its title in the page's colour over a line.
pub fn title<D: Paint>(target: &mut D, clip: &Rectangle, text: &str, color: Rgb565) {
    centred(target, clip, TITLE_Y, &NW_TITLE, text, color);
    rule(target, clip, C - TITLE_RULE_HALF, TITLE_RULE_Y, 2 * TITLE_RULE_HALF, color);
}

/// How far a tag's sides lean, and its height.
const TAG_LEAN: i32 = 6;
const TAG_HEIGHT: i32 = 27;

/// The rectangle a tag with `text` covers, its middle at `cx`.
pub fn tag_rect(cx: i32, y: i32, text: &str) -> Rectangle {
    let width = NW_LABEL.measure(text) + 14 + TAG_LEAN;
    Rectangle::new(Point::new(cx - width / 2, y), Size::new(width as u32, TAG_HEIGHT as u32))
}

/// A tag: black capitals on a coloured box whose sides lean to the right.
pub fn tag<D: Paint>(target: &mut D, clip: &Rectangle, cx: i32, y: i32, text: &str, color: Rgb565) {
    let tag = tag_rect(cx, y, text);
    for row in 0..TAG_HEIGHT {
        let lean = TAG_LEAN - TAG_LEAN * row / (TAG_HEIGHT - 1);
        let line = Rectangle::new(Point::new(tag.top_left.x + lean, y + row), Size::new((tag.size.width as i32 - TAG_LEAN) as u32, 1));
        let _ = target.fill_solid(&line.intersection(clip), color);
    }
    target.text(clip, tag.top_left.x + TAG_LEAN / 2 + 8, y + 1, &NW_LABEL, text, BLACK);
}

/// A frame: a thin outline with the top left and the bottom right corner
/// cut off by `cut` pixels.
pub fn frame<D: Paint>(target: &mut D, clip: &Rectangle, rect: &Rectangle, cut: i32, color: Rgb565) {
    let (x, y) = (rect.top_left.x, rect.top_left.y);
    let (right, bottom) = (x + rect.size.width as i32 - 1, y + rect.size.height as i32 - 1);
    let corners = [
        Point::new(x + cut, y),
        Point::new(right, y),
        Point::new(right, bottom - cut),
        Point::new(right - cut, bottom),
        Point::new(x, bottom),
        Point::new(x, y + cut),
        Point::new(x + cut, y),
    ];
    let mut clipped = target.clipped(clip);
    for pair in corners.windows(2) {
        let _ = Line::new(pair[0], pair[1]).into_styled(PrimitiveStyle::with_stroke(color, 1)).draw(&mut clipped);
    }
}

/// A plate: the same shape as a frame, filled. What is chosen or
/// switched on is a plate in its colour with black text on it.
pub fn plate<D: Paint>(target: &mut D, clip: &Rectangle, rect: &Rectangle, cut: i32, color: Rgb565) {
    let (width, height) = (rect.size.width as i32, rect.size.height as i32);
    for row in 0..height {
        let left = (cut - row).max(0);
        let right = (cut - (height - 1 - row)).max(0);
        let line = Rectangle::new(Point::new(rect.top_left.x + left, rect.top_left.y + row), Size::new((width - left - right).max(0) as u32, 1));
        let _ = target.fill_solid(&line.intersection(clip), color);
    }
}

/// The bar a page is held or left by, its top edge at `y`.
pub fn handle<D: Paint>(target: &mut D, clip: &Rectangle, y: i32, color: Rgb565) {
    let bar = Rectangle::new(Point::new(C - 40, y), Size::new(80, 4));
    let _ = target.fill_solid(&bar.intersection(clip), color);
}
