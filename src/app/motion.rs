//! How the picture moves while one page gives way to another under the
//! finger, and when letting go finishes the change.

use super::face::SIZE;

/// How the picture moves while one page gives way to another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    /// Two pages side by side move to the left (the new one comes from
    /// the right) or to the right.
    Pages { leftwards: bool },
    /// A panel comes in over the watch face, from the top edge or the
    /// bottom edge. The face stays where it is.
    PanelIn { from_top: bool },
    /// A panel goes back out over the edge it came from, and the watch
    /// face under it shows again.
    PanelOut { to_top: bool },
    /// No page lies the way the finger pulls: the page gives a little,
    /// to the left or to the right, and goes back when the finger lets
    /// go.
    Give { leftwards: bool },
}

/// One of the up to two pieces a picture of a move is made of: a
/// rectangle of a page's picture, shown at its own size somewhere on the
/// screen. The display controller shows each piece straight from the
/// page's frame, so a picture of a move costs no copying.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Piece {
    /// Whose picture: the page being left (`false`) or the one coming.
    pub coming: bool,
    /// The rectangle's top left corner in that picture.
    pub from_x: i32,
    pub from_y: i32,
    /// Where on the screen it goes.
    pub to_x: i32,
    pub to_y: i32,
    pub width: i32,
    pub height: i32,
}

/// How far a page gives when nothing lies that way. The stock app's
/// number (`0x80F8CFE`).
pub const GIVE: i32 = 50;

/// A flick: letting go faster than this many pixels a second finishes a
/// change however short the pull was. Our own number, tried on the watch
/// (40 of 40 swipes); stock wants 14 pixels between the last two places
/// it sampled (`0x80F8D00`), which is more at any likely rate.
const FLICK: i32 = 400;

impl Motion {
    /// How far the picture moves altogether, in pixels.
    pub fn extent(self) -> usize {
        match self {
            Motion::Give { .. } => GIVE as usize,
            // The screen is as high as it is wide.
            _ => SIZE as usize,
        }
    }

    /// How much of a movement of (x, y) goes the way this motion goes.
    /// Also for speeds.
    pub fn along(self, x: i32, y: i32) -> i32 {
        match self {
            Motion::Pages { leftwards: true } | Motion::Give { leftwards: true } => -x,
            Motion::Pages { leftwards: false } | Motion::Give { leftwards: false } => x,
            Motion::PanelIn { from_top: true } | Motion::PanelOut { to_top: false } => y,
            Motion::PanelIn { from_top: false } | Motion::PanelOut { to_top: true } => -y,
        }
    }

    /// The pieces of the picture when the move has got `offset` pixels
    /// on its way (0: all of the page being left; `extent`: all of the
    /// page coming). The pieces never overlap; a piece that would be
    /// empty is left out. Where no piece is, the screen is black.
    pub fn pieces(self, offset: usize) -> [Option<Piece>; 2] {
        let o = (offset.min(self.extent())) as i32;
        // A piece as wide or as high as the screen, cut to `width` x
        // `height`; nothing if that is empty.
        let piece = |coming, from_x, from_y, to_x, to_y, width, height| (width > 0 && height > 0).then_some(Piece { coming, from_x, from_y, to_x, to_y, width, height });
        match self {
            // Side by side: the page being left moves out, the other
            // follows it in.
            Motion::Pages { leftwards: true } => [piece(false, o, 0, 0, 0, SIZE - o, SIZE), piece(true, 0, 0, SIZE - o, 0, o, SIZE)],
            Motion::Pages { leftwards: false } => [piece(false, 0, 0, o, 0, SIZE - o, SIZE), piece(true, SIZE - o, 0, 0, 0, o, SIZE)],
            // The panel's far end comes in first; the face under it
            // stays where it is and shows as far as it is not covered.
            Motion::PanelIn { from_top: true } => [piece(false, 0, o, 0, o, SIZE, SIZE - o), piece(true, 0, SIZE - o, 0, 0, SIZE, o)],
            Motion::PanelIn { from_top: false } => [piece(false, 0, 0, 0, 0, SIZE, SIZE - o), piece(true, 0, 0, 0, SIZE - o, SIZE, o)],
            // The same pictures backwards: the panel is the page being
            // left, and so much of it has gone already.
            Motion::PanelOut { to_top: true } => [piece(false, 0, o, 0, 0, SIZE, SIZE - o), piece(true, 0, SIZE - o, 0, SIZE - o, SIZE, o)],
            Motion::PanelOut { to_top: false } => [piece(false, 0, 0, 0, o, SIZE, SIZE - o), piece(true, 0, 0, 0, 0, SIZE, o)],
            // One page, moved aside.
            Motion::Give { leftwards: true } => [piece(false, o, 0, 0, 0, SIZE - o, SIZE), None],
            Motion::Give { leftwards: false } => [piece(false, 0, 0, o, 0, SIZE - o, SIZE), None],
        }
    }

    /// Whether letting go after `travel` pixels at `speed` pixels a
    /// second, both counted the way this motion goes, finishes the
    /// change; otherwise the picture goes back. Far enough is the stock
    /// app's share: a third of the width for pages (`0x80F8CE8`), half
    /// the height for the panels (`0x80F9326`). A page that only gives
    /// always goes back.
    pub fn finishes(self, travel: i32, speed: i32) -> bool {
        match self {
            Motion::Pages { .. } => travel > SIZE / 3 || speed > FLICK,
            Motion::PanelIn { .. } | Motion::PanelOut { .. } => travel > SIZE / 2 || speed > FLICK,
            Motion::Give { .. } => false,
        }
    }
}
