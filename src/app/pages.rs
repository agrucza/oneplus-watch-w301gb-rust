//! The pages the screen can show, and which gesture leads from which
//! page to which. What the stock app does is in docs/hardware-plan.md,
//! 3.1.

use super::motion::Motion;
use crate::services::gestures::Direction;

/// What the screen shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    /// The watch face: the first of the pages side by side.
    Face,
    /// The page of status text beside it: the last of them.
    Status,
    /// The panel pulled over the face from the top edge.
    Above,
    /// The panel pulled over the face from the bottom edge.
    Below,
    /// Opened by a long press on the watch face. In the stock app that
    /// leads to another screen, probably the one to choose a face on.
    Faces,
    /// Opened by a long press on the status page. In the stock app a
    /// long press on a page beside the face leads to another screen,
    /// probably the one to arrange those pages on.
    Cards,
    /// Restart, power off and the way back to the stock firmware
    /// (`super::power`).
    Power,
    /// A time of day being set (`super::picker`), opened from the
    /// settings and left by its two buttons.
    Picker,
}

/// How many frames the pages have between them (`Page::frames`).
pub const FRAMES: usize = 9;

impl Page {
    /// A number for each page, from 0, for tables with an entry a page.
    pub const COUNT: usize = 8;
    pub fn index(self) -> usize {
        match self {
            Page::Face => 0,
            Page::Status => 1,
            Page::Above => 2,
            Page::Below => 3,
            Page::Faces => 4,
            Page::Cards => 5,
            Page::Power => 6,
            Page::Picker => 7,
        }
    }

    /// The frames that are this page's own: the first one's number and
    /// how many. Every page keeps its picture, so that it is there when
    /// the page is wanted again. A page whose picture changes while it
    /// is shown has two frames, to draw into the one that is not on the
    /// screen; a page that always looks the same has one, and so has
    /// the power page, which changes only on a tap: its frame is drawn
    /// into between two pictures, and the panel shows what it was sent
    /// until the next one. The same goes for the page a time is set on.
    /// The two pages opened by a long press share a frame: only one of
    /// them can be on the screen, and whichever is opened is drawn
    /// afresh (`shared_frame`).
    pub fn frames(self) -> (usize, usize) {
        match self {
            Page::Face => (0, 2),
            Page::Status => (2, 2),
            Page::Above => (4, 1),
            Page::Below => (5, 1),
            Page::Faces => (6, 1),
            Page::Cards => (6, 1),
            Page::Picker => (7, 1),
            Page::Power => (8, 1),
        }
    }
}

impl Page {
    /// True for a page whose frame another page uses too: its picture
    /// may be gone when it is opened.
    pub fn shared_frame(self) -> bool {
        matches!(self, Page::Faces | Page::Cards)
    }
}

/// Where a drag that starts off in `direction` on `page` leads, and how
/// the picture moves on the way there. `None` if nothing lies that way.
pub fn drag(page: Page, direction: Direction) -> Option<(Page, Motion)> {
    Some(match (page, direction) {
        (Page::Face, Direction::Left) => (Page::Status, Motion::Pages { leftwards: true }),
        (Page::Status, Direction::Right) => (Page::Face, Motion::Pages { leftwards: false }),
        (Page::Face, Direction::Down) => (Page::Above, Motion::PanelIn { from_top: true }),
        (Page::Face, Direction::Up) => (Page::Below, Motion::PanelIn { from_top: false }),
        (Page::Above, Direction::Up) => (Page::Face, Motion::PanelOut { to_top: true }),
        (Page::Below, Direction::Down) => (Page::Face, Motion::PanelOut { to_top: false }),
        // The first and the last of the pages side by side: nothing lies
        // further out, the page only gives.
        (Page::Face, Direction::Right) => (Page::Face, Motion::Give { leftwards: false }),
        (Page::Status, Direction::Left) => (Page::Status, Motion::Give { leftwards: true }),
        _ => return None,
    })
}

/// The page a long press on `page` opens, if any.
pub fn long_press(page: Page) -> Option<Page> {
    match page {
        Page::Face => Some(Page::Faces),
        Page::Status => Some(Page::Cards),
        _ => None,
    }
}

/// The page a tap on `page` leads to, if any: the pages a long press
/// opened are left by a tap, back to where they were opened from.
pub fn tap(page: Page) -> Option<Page> {
    match page {
        Page::Faces => Some(Page::Face),
        Page::Cards => Some(Page::Status),
        _ => None,
    }
}
