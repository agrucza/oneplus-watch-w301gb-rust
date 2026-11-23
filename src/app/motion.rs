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
}

impl Motion {
    /// How far the picture moves altogether, in pixels.
    pub fn extent(self) -> usize {
        // The screen is as high as it is wide.
        SIZE as usize
    }

    /// How much of a movement of (x, y) goes the way this motion goes.
    /// Also for speeds.
    pub fn along(self, x: i32, y: i32) -> i32 {
        match self {
            Motion::Pages { leftwards: true } => -x,
            Motion::Pages { leftwards: false } => x,
            Motion::PanelIn { from_top: true } | Motion::PanelOut { to_top: false } => y,
            Motion::PanelIn { from_top: false } | Motion::PanelOut { to_top: true } => -y,
        }
    }

    /// Whether letting go after `travel` pixels finishes the change. The
    /// stock app's shares: a third of the width for pages (`0x80F8CE8`),
    /// half the height for the panels (`0x80F9326`).
    pub fn far_enough(self, travel: i32) -> bool {
        match self {
            Motion::Pages { .. } => travel > SIZE / 3,
            _ => travel > SIZE / 2,
        }
    }
}
