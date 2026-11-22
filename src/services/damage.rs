//! Keeping track of which parts of the picture changed, so only those are
//! drawn again.
//!
//! Whatever draws an element reports the element's rectangle when its
//! content changes (`add`). When a picture is due, the rectangles reported
//! since that frame was last drawn are taken (`take`) and only they are
//! repainted.
//!
//! There are two frames (see `drivers::panel`): the one drawn into next
//! still holds the picture from two pictures ago. A change therefore has
//! to be repainted in both, so every report goes onto two lists, one per
//! frame, and each frame empties its own list when it is drawn. The stock
//! app's display layer does the same with its two frame buffers.

use embedded_graphics::primitives::Rectangle;

/// Rectangles one list can hold before it gives up and says "everything".
/// Large enough for a ring of sixty segments changing at once.
const CAPACITY: usize = 96;

/// What has to be repainted in one frame.
#[derive(Clone, Copy)]
pub struct Changes {
    rects: [Rectangle; CAPACITY],
    len: usize,
    /// The whole frame, whatever `rects` says.
    all: bool,
}

impl Changes {
    const fn nothing() -> Self {
        Self { rects: [Rectangle::zero(); CAPACITY], len: 0, all: false }
    }

    const fn everything() -> Self {
        Self { rects: [Rectangle::zero(); CAPACITY], len: 0, all: true }
    }

    pub fn is_empty(&self) -> bool {
        !self.all && self.len == 0
    }

    /// The rectangles to repaint; for "everything" the one rectangle
    /// `whole`.
    pub fn rects(&self, whole: Rectangle) -> impl Iterator<Item = Rectangle> + '_ {
        let all = self.all.then_some(whole);
        let listed = if self.all { &self.rects[..0] } else { &self.rects[..self.len] };
        all.into_iter().chain(listed.iter().copied())
    }

    /// The area to repaint, in pixels (overlaps counted twice); for the log.
    pub fn area(&self, whole: Rectangle) -> u32 {
        self.rects(whole).map(|r| r.size.width * r.size.height).sum()
    }

    fn add(&mut self, rect: Rectangle) {
        if self.all || rect.is_zero_sized() {
            return;
        }
        // Already covered by one that is listed?
        if self.rects[..self.len].iter().any(|r| r.intersection(&rect) == rect) {
            return;
        }
        if self.len == CAPACITY {
            self.all = true;
            return;
        }
        self.rects[self.len] = rect;
        self.len += 1;
    }
}

/// The two lists, one per frame.
pub struct Damage {
    pending: [Changes; 2],
}

impl Damage {
    /// Both frames need drawing completely: nothing is in them yet.
    pub const fn new() -> Self {
        Self { pending: [Changes::everything(); 2] }
    }

    /// The content inside `rect` changed.
    pub fn add(&mut self, rect: Rectangle) {
        for list in &mut self.pending {
            list.add(rect);
        }
    }

    /// Both frames now hold the picture as it should be.
    pub fn clear(&mut self) {
        self.pending = [Changes::nothing(); 2];
    }

    /// True if `frame` has something to repaint.
    pub fn pending(&self, frame: usize) -> bool {
        !self.pending[frame].is_empty()
    }

    /// What `frame` has to repaint; its list is empty afterwards.
    pub fn take(&mut self, frame: usize) -> Changes {
        core::mem::replace(&mut self.pending[frame], Changes::nothing())
    }
}
