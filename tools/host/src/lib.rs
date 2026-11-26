//! The firmware's pure logic, built for the PC so it can be tested there.
//!
//! Each module below is the firmware's own source file. What such a file
//! needs from the rest of the firmware is stood in for here by the least
//! that will do.

pub mod drivers;
pub mod app;
pub mod services;

/// `services::gestures`: what the finger's reports are turned into.
#[cfg(test)]
mod gestures {
    use crate::drivers::touch::TouchEvent::{Down, Up};
    use crate::services::gestures::{Direction, Gesture, Gestures};
    use embassy_time::{Duration, Instant};

    fn at(ms: u64) -> Instant {
        Instant::from_ticks(0) + Duration::from_millis(1_000_000 + ms)
    }

    #[test]
    fn tap() {
        let mut g = Gestures::new();
        assert_eq!(g.feed(Some(Down { x: 100, y: 200 }), at(0)), None);
        assert_eq!(g.feed(None, at(50)), None);
        // A finger that wobbles is still no drag.
        assert_eq!(g.feed(Some(Down { x: 105, y: 196 }), at(80)), None);
        assert_eq!(g.feed(Some(Up), at(120)), Some(Gesture::Tap { x: 100, y: 200 }));
        assert_eq!(g.feed(None, at(2000)), None);
    }

    /// As the watch logged it: one touch reported as two, the second
    /// lifting 54 ms after the first.
    #[test]
    fn a_tap_reported_twice_counts_once() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 324, y: 126 }), at(0));
        assert_eq!(g.feed(Some(Up), at(60)), Some(Gesture::Tap { x: 324, y: 126 }));
        assert_eq!(g.feed(Some(Down { x: 328, y: 106 }), at(90)), None);
        assert_eq!(g.feed(Some(Up), at(114)), None);
        // A second tap by a finger, however quick, still counts.
        assert_eq!(g.feed(Some(Down { x: 326, y: 120 }), at(260)), None);
        assert_eq!(g.feed(Some(Up), at(320)), Some(Gesture::Tap { x: 326, y: 120 }));
        // And a drag that begins right after a tap is a drag.
        assert_eq!(g.feed(Some(Down { x: 100, y: 100 }), at(350)), None);
        assert!(matches!(g.feed(Some(Down { x: 160, y: 100 }), at(380)), Some(Gesture::DragStart { .. })));
    }

    #[test]
    fn up_without_down_is_nothing() {
        let mut g = Gestures::new();
        assert_eq!(g.feed(Some(Up), at(0)), None);
        assert_eq!(g.feed(None, at(5000)), None);
    }

    #[test]
    fn long_press_once_and_no_tap_after() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 10, y: 10 }), at(0));
        assert_eq!(g.feed(None, at(599)), None);
        assert_eq!(g.feed(None, at(600)), Some(Gesture::LongPress { x: 10, y: 10 }));
        assert_eq!(g.feed(None, at(1300)), None);
        assert_eq!(g.feed(Some(Down { x: 11, y: 10 }), at(1350)), None);
        assert_eq!(g.feed(Some(Up), at(1400)), None);
        // The next touch starts clean.
        g.feed(Some(Down { x: 10, y: 10 }), at(2000));
        assert_eq!(g.feed(Some(Up), at(2050)), Some(Gesture::Tap { x: 10, y: 10 }));
    }

    #[test]
    fn no_drag_after_long_press() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 200, y: 200 }), at(0));
        assert!(matches!(g.feed(None, at(1100)), Some(Gesture::LongPress { .. })));
        assert_eq!(g.feed(Some(Down { x: 100, y: 200 }), at(1200)), None);
        assert_eq!(g.feed(Some(Up), at(1300)), None);
    }

    /// The direction a touch from (200, 200) to (x, y) starts a drag in.
    fn start(x: u16, y: u16) -> Option<Direction> {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 200, y: 200 }), at(0));
        match g.feed(Some(Down { x, y }), at(20)) {
            Some(Gesture::DragStart { x: 200, y: 200, direction, dx, dy }) => {
                assert_eq!((dx, dy), (x as i32 - 200, y as i32 - 200));
                Some(direction)
            }
            None => None,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn drag_left() {
        assert_eq!(start(185, 200), Some(Direction::Left));
        assert_eq!(start(150, 240), Some(Direction::Left));
    }

    #[test]
    fn drag_right() {
        assert_eq!(start(215, 200), Some(Direction::Right));
        assert_eq!(start(250, 160), Some(Direction::Right));
    }

    #[test]
    fn drag_up() {
        assert_eq!(start(200, 185), Some(Direction::Up));
        assert_eq!(start(240, 150), Some(Direction::Up));
    }

    #[test]
    fn drag_down() {
        assert_eq!(start(200, 215), Some(Direction::Down));
        assert_eq!(start(160, 250), Some(Direction::Down));
    }

    /// Fifteen pixels in x or in y start a drag, fourteen do not; as far
    /// in x as in y is vertical, as in the stock app.
    #[test]
    fn where_a_drag_begins() {
        assert_eq!(start(214, 214), None);
        assert_eq!(start(186, 186), None);
        assert_eq!(start(215, 214), Some(Direction::Right));
        assert_eq!(start(215, 215), Some(Direction::Down));
        assert_eq!(start(185, 185), Some(Direction::Up));
        assert_eq!(start(215, 185), Some(Direction::Up));
    }

    /// The direction is decided once: a drag that began to the left stays
    /// one drag when the finger wanders up or comes back.
    #[test]
    fn direction_is_kept() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 300, y: 200 }), at(0));
        assert_eq!(
            g.feed(Some(Down { x: 280, y: 200 }), at(20)),
            Some(Gesture::DragStart { x: 300, y: 200, direction: Direction::Left, dx: -20, dy: 0 })
        );
        assert_eq!(g.feed(Some(Down { x: 270, y: 120 }), at(40)), Some(Gesture::Drag { dx: -30, dy: -80 }));
        assert_eq!(g.feed(Some(Down { x: 340, y: 120 }), at(60)), Some(Gesture::Drag { dx: 40, dy: -80 }));
    }

    #[test]
    fn drag_and_flick() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 300, y: 200 }), at(0));
        assert_eq!(g.feed(Some(Down { x: 290, y: 200 }), at(20)), None);
        assert_eq!(
            g.feed(Some(Down { x: 280, y: 202 }), at(40)),
            Some(Gesture::DragStart { x: 300, y: 200, direction: Direction::Left, dx: -20, dy: 2 })
        );
        assert_eq!(g.feed(None, at(50)), None);
        assert_eq!(g.feed(Some(Down { x: 260, y: 202 }), at(60)), Some(Gesture::Drag { dx: -40, dy: 2 }));
        assert_eq!(g.feed(Some(Down { x: 240, y: 202 }), at(80)), Some(Gesture::Drag { dx: -60, dy: 2 }));
        // 60 px in 80 ms = 750 px/s leftwards
        match g.feed(Some(Up), at(85)) {
            Some(Gesture::DragEnd { dx: -60, dy: 2, speed_x, speed_y }) => {
                assert_eq!(speed_x, -750);
                assert!(speed_y.abs() < 50, "{speed_y}");
            }
            other => panic!("{other:?}"),
        }
        // No long press while dragging, and none after.
        assert_eq!(g.feed(None, at(5000)), None);
    }

    /// Speed is told along y as along x.
    #[test]
    fn flick_down() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 200, y: 100 }), at(0));
        assert!(matches!(g.feed(Some(Down { x: 200, y: 130 }), at(30)), Some(Gesture::DragStart { direction: Direction::Down, .. })));
        g.feed(Some(Down { x: 200, y: 160 }), at(60));
        assert_eq!(g.feed(Some(Up), at(65)), Some(Gesture::DragEnd { dx: 0, dy: 60, speed_x: 0, speed_y: 1000 }));
    }

    #[test]
    fn slow_pull_that_rests_has_no_speed() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 300, y: 200 }), at(0));
        assert!(matches!(g.feed(Some(Down { x: 200, y: 200 }), at(100)), Some(Gesture::DragStart { .. })));
        assert_eq!(g.feed(None, at(1500)), None); // dragging: no long press
        assert_eq!(g.feed(Some(Up), at(2000)), Some(Gesture::DragEnd { dx: -100, dy: 0, speed_x: 0, speed_y: 0 }));
    }

    #[test]
    fn many_moves_do_not_overrun_the_trail() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 0, y: 0 }), at(0));
        for i in 1..200u16 {
            g.feed(Some(Down { x: i * 2, y: 0 }), at(i as u64 * 10));
        }
        // 2 px every 10 ms = 200 px/s
        match g.feed(Some(Up), at(1995)) {
            Some(Gesture::DragEnd { dx: 398, speed_x, .. }) => assert_eq!(speed_x, 200),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_samples_at_the_same_instant() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 0, y: 0 }), at(0));
        g.feed(Some(Down { x: 100, y: 0 }), at(0));
        assert_eq!(g.feed(Some(Up), at(0)), Some(Gesture::DragEnd { dx: 100, dy: 0, speed_x: 0, speed_y: 0 }));
    }

    /// As logged on the watch: a swipe of 233 ms of which only the start
    /// and one late place were seen.
    #[test]
    fn quick_swipe_seen_sparsely() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 127, y: 305 }), at(0));
        assert!(matches!(g.feed(Some(Down { x: 274, y: 305 }), at(225)), Some(Gesture::DragStart { .. })));
        match g.feed(Some(Up), at(233)) {
            Some(Gesture::DragEnd { dx: 147, speed_x, .. }) => assert_eq!(speed_x, 147 * 1000 / 225),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn long_touch_seen_sparsely_has_no_speed() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 100, y: 100 }), at(0));
        g.feed(Some(Down { x: 300, y: 100 }), at(450));
        assert_eq!(g.feed(Some(Up), at(455)), Some(Gesture::DragEnd { dx: 200, dy: 0, speed_x: 0, speed_y: 0 }));
    }
}

/// `app::motion`: how far a finger's movement carries a change of page.
#[cfg(test)]
mod motion {
    use crate::app::motion::Motion;

    #[test]
    fn pages_go_by_x() {
        let left = Motion::Pages { leftwards: true };
        let right = Motion::Pages { leftwards: false };
        assert_eq!(left.along(-100, 30), 100);
        assert_eq!(left.along(100, 30), -100);
        assert_eq!(right.along(100, -30), 100);
        assert_eq!(right.along(-100, -30), -100);
        assert_eq!(left.extent(), 454);
    }

    #[test]
    fn panels_go_by_y() {
        // In from the top and out over the bottom: downwards.
        assert_eq!(Motion::PanelIn { from_top: true }.along(30, 100), 100);
        assert_eq!(Motion::PanelOut { to_top: false }.along(30, 100), 100);
        assert_eq!(Motion::PanelIn { from_top: true }.along(30, -100), -100);
        // In from the bottom and out over the top: upwards.
        assert_eq!(Motion::PanelIn { from_top: false }.along(30, -100), 100);
        assert_eq!(Motion::PanelOut { to_top: true }.along(30, -100), 100);
        assert_eq!(Motion::PanelOut { to_top: true }.along(30, 100), -100);
        assert_eq!(Motion::PanelIn { from_top: true }.extent(), 454);
    }

    /// Pages change past a third of the width, panels past half the
    /// height, both also with a flick.
    #[test]
    fn what_finishes_a_change() {
        let page = Motion::Pages { leftwards: true };
        assert!(!page.finishes(151, 400));
        assert!(page.finishes(152, 0));
        assert!(page.finishes(20, 401));
        for panel in [Motion::PanelIn { from_top: true }, Motion::PanelIn { from_top: false }, Motion::PanelOut { to_top: true }, Motion::PanelOut { to_top: false }] {
            assert!(!panel.finishes(227, 400));
            assert!(panel.finishes(228, 0));
            assert!(panel.finishes(20, 401));
        }
    }

    /// A page with nothing beside it gives 50 pixels the way the finger
    /// goes, and always goes back.
    #[test]
    fn give_at_the_ends() {
        let right = Motion::Give { leftwards: false };
        let left = Motion::Give { leftwards: true };
        assert_eq!(right.extent(), 50);
        assert_eq!(right.along(30, 80), 30);
        assert_eq!(left.along(-30, 80), 30);
        assert_eq!(left.along(30, 80), -30);
        assert!(!right.finishes(50, 5000));
        assert!(!left.finishes(400, 5000));
    }
}

/// `app::motion`, the pictures of a move: what the screen shows, built
/// from the pieces as the display controller would show them.
#[cfg(test)]
mod pieces {
    use crate::app::motion::{Motion, Piece};

    const S: i32 = 454;
    const MOTIONS: [Motion; 8] = [
        Motion::Pages { leftwards: true },
        Motion::Pages { leftwards: false },
        Motion::PanelIn { from_top: true },
        Motion::PanelIn { from_top: false },
        Motion::PanelOut { to_top: true },
        Motion::PanelOut { to_top: false },
        Motion::Give { leftwards: true },
        Motion::Give { leftwards: false },
    ];

    /// What a screen pixel shows: which page's picture and which pixel
    /// of it; `None` is black. Panics if two pieces claim one pixel or a
    /// piece reaches outside the screen or the picture.
    fn screen(pieces: [Option<Piece>; 2]) -> Vec<Option<(bool, i32, i32)>> {
        let mut out = vec![None; (S * S) as usize];
        for p in pieces.into_iter().flatten() {
            assert!(p.width > 0 && p.height > 0, "{p:?}");
            for (start, size) in [(p.from_x, p.width), (p.from_y, p.height), (p.to_x, p.width), (p.to_y, p.height)] {
                assert!(start >= 0 && start + size <= S, "{p:?}");
            }
            for y in 0..p.height {
                for x in 0..p.width {
                    let at = &mut out[((p.to_y + y) * S + p.to_x + x) as usize];
                    assert!(at.is_none(), "overlap in {p:?}");
                    *at = Some((p.coming, p.from_x + x, p.from_y + y));
                }
            }
        }
        out
    }

    /// At the start all of the page being left, at the end all of the
    /// page coming (a page that gives only ever shows itself).
    #[test]
    fn start_and_end() {
        for motion in MOTIONS {
            let start = screen(motion.pieces(0));
            let end = screen(motion.pieces(motion.extent()));
            for y in 0..S {
                for x in 0..S {
                    assert_eq!(start[(y * S + x) as usize], Some((false, x, y)), "{motion:?}");
                    if !matches!(motion, Motion::Give { .. }) {
                        assert_eq!(end[(y * S + x) as usize], Some((true, x, y)), "{motion:?}");
                    }
                }
            }
        }
    }

    /// Every pixel of every picture on the way, against what the move is
    /// meant to look like.
    #[test]
    fn on_the_way() {
        for motion in MOTIONS {
            for o in [1, 7, 50, 151, 227, 300, 453].into_iter().filter(|&o| o as usize <= motion.extent()) {
                let got = screen(motion.pieces(o as usize));
                for y in 0..S {
                    for x in 0..S {
                        let expected = match motion {
                            // Two pages side by side, moved o pixels.
                            Motion::Pages { leftwards: true } => Some(if x + o < S { (false, x + o, y) } else { (true, x + o - S, y) }),
                            Motion::Pages { leftwards: false } => Some(if x >= o { (false, x - o, y) } else { (true, x - o + S, y) }),
                            // A panel o rows in, its far end first, over a
                            // face that does not move.
                            Motion::PanelIn { from_top: true } => Some(if y < o { (true, x, y + S - o) } else { (false, x, y) }),
                            Motion::PanelIn { from_top: false } => Some(if y >= S - o { (true, x, y - (S - o)) } else { (false, x, y) }),
                            // A panel o rows out, over a face that does
                            // not move.
                            Motion::PanelOut { to_top: true } => Some(if y < S - o { (false, x, y + o) } else { (true, x, y) }),
                            Motion::PanelOut { to_top: false } => Some(if y >= o { (false, x, y - o) } else { (true, x, y) }),
                            // One page moved aside, black behind it.
                            Motion::Give { leftwards: true } => (x + o < S).then_some((false, x + o, y)),
                            Motion::Give { leftwards: false } => (x >= o).then_some((false, x - o, y)),
                        };
                        assert_eq!(got[(y * S + x) as usize], expected, "{motion:?} at {o}: pixel {x}, {y}");
                    }
                }
            }
        }
    }

    /// Asking for more than the whole way is the whole way.
    #[test]
    fn beyond_the_end() {
        for motion in MOTIONS {
            assert_eq!(motion.pieces(motion.extent() + 100), motion.pieces(motion.extent()), "{motion:?}");
        }
    }
}

/// `app::pages`: which gesture leads from which page to which.
#[cfg(test)]
mod pages {
    use crate::app::motion::Motion;
    use crate::app::pages::{drag, long_press, tap, Page};
    use crate::services::gestures::Direction::{self, Down, Left, Right, Up};

    const PAGES: [Page; 10] = [Page::Face, Page::Launcher, Page::Status, Page::Above, Page::Below, Page::Faces, Page::Cards, Page::Power, Page::Picker, Page::Settings];
    const DIRECTIONS: [Direction; 4] = [Left, Right, Up, Down];

    /// Every page with every direction: the whole table.
    #[test]
    fn drags() {
        for page in PAGES {
            for direction in DIRECTIONS {
                let expected = match (page, direction) {
                    (Page::Face, Left) => Some((Page::Launcher, Motion::Pages { leftwards: true })),
                    (Page::Face, Right) => Some((Page::Face, Motion::Give { leftwards: false })),
                    (Page::Face, Down) => Some((Page::Above, Motion::PanelIn { from_top: true })),
                    (Page::Face, Up) => Some((Page::Below, Motion::PanelIn { from_top: false })),
                    (Page::Launcher, Right) => Some((Page::Face, Motion::Pages { leftwards: false })),
                    (Page::Launcher, Left) => Some((Page::Launcher, Motion::Give { leftwards: true })),
                    (Page::Above, Up) => Some((Page::Face, Motion::PanelOut { to_top: true })),
                    (Page::Below, Down) => Some((Page::Face, Motion::PanelOut { to_top: false })),
                    _ => None,
                };
                assert_eq!(drag(page, direction), expected, "{page:?} {direction:?}");
            }
        }
    }

    /// A drag carries the picture the way the finger went when it began.
    #[test]
    fn a_drag_moves_with_the_finger() {
        for page in PAGES {
            for direction in DIRECTIONS {
                if let Some((_, motion)) = drag(page, direction) {
                    let (x, y) = match direction {
                        Left => (-10, 0),
                        Right => (10, 0),
                        Up => (0, -10),
                        Down => (0, 10),
                    };
                    assert_eq!(motion.along(x, y), 10, "{page:?} {direction:?}");
                }
            }
        }
    }

    /// Every page has frames of its own, except those that say they share
    /// one, and together they are exactly the frames there are.
    #[test]
    fn frames_are_shared_out() {
        use crate::app::pages::FRAMES;
        let mut owner = [None; FRAMES];
        let mut numbers = [false; Page::COUNT];
        for page in PAGES {
            assert!(!numbers[page.index()], "{page:?}");
            numbers[page.index()] = true;
            let (first, count) = page.frames();
            assert!(count == 1 || count == 2, "{page:?}");
            for frame in first..first + count {
                if let Some(other) = owner[frame] {
                    let other: Page = other;
                    assert!(page.shared_frame() && other.shared_frame() && count == 1, "{page:?} and {other:?} both use frame {frame}");
                }
                owner[frame] = Some(page);
            }
        }
        assert!(owner.iter().all(Option::is_some));
        assert_eq!(PAGES.len(), Page::COUNT);
    }

    #[test]
    fn long_press_and_back() {
        assert_eq!(long_press(Page::Face), Some(Page::Faces));
        assert_eq!(long_press(Page::Status), Some(Page::Cards));
        for page in [Page::Launcher, Page::Above, Page::Below, Page::Faces, Page::Cards, Page::Power, Page::Picker, Page::Settings] {
            assert_eq!(long_press(page), None);
        }
        // A tap leaves what a long press opened, back to where it came from.
        for page in PAGES {
            match long_press(page) {
                Some(opened) => assert_eq!(tap(opened), Some(page)),
                None => {}
            }
        }
        for page in [Page::Face, Page::Launcher, Page::Status, Page::Above, Page::Below, Page::Power, Page::Picker, Page::Settings] {
            assert_eq!(tap(page), None);
        }
    }
}

/// `drivers::touch_report`: what the touch controller's reports say.
#[cfg(test)]
mod touch_report {
    use crate::drivers::touch_report::{parse, TouchReport};

    /// A report as the watch logged it: one finger at 216, 56.
    const FINGER: [u8; 17] = [0x11, 0x00, 0x01, 0x43, 0x0f, 0x01, 0x40, 0x00, 0xa0, 0xd8, 0x00, 0x38, 0x00, 0x19, 0x00, 0x00, 0x00];

    #[test]
    fn finger() {
        assert_eq!(parse(&FINGER), Some(TouchReport { large_object: false, finger: Some((216, 56)) }));
    }

    #[test]
    fn lift() {
        // The touch's event says "lifted".
        let mut report = FINGER;
        report[8] = 0x60;
        assert_eq!(parse(&report), Some(TouchReport { large_object: false, finger: None }));
        // No touches at all.
        let mut report = FINGER;
        report[5] = 0x00;
        assert_eq!(parse(&report), Some(TouchReport { large_object: false, finger: None }));
        // A touch report without a touch in it.
        assert_eq!(parse(&FINGER[..7]), Some(TouchReport { large_object: false, finger: None }));
    }

    /// Bit 5 of the count byte, with a finger and without.
    #[test]
    fn large_object() {
        let mut report = FINGER;
        report[5] = 0x21;
        assert_eq!(parse(&report), Some(TouchReport { large_object: true, finger: Some((216, 56)) }));
        report[5] = 0x20;
        assert_eq!(parse(&report), Some(TouchReport { large_object: true, finger: None }));
    }

    #[test]
    fn other_reports() {
        let mut report = FINGER;
        report[2] = 0x04;
        assert_eq!(parse(&report), None);
        assert_eq!(parse(&FINGER[..6]), None);
        assert_eq!(parse(&[]), None);
    }
}

/// `app::controls`: what the buttons do.
#[cfg(test)]
mod controls {
    use crate::app::controls::{Action, Bindings, Button, Hold, Press, Taps, DOUBLE_MS, LONG_MS, VERY_LONG_MS};
    use crate::app::pages::Page;

    #[test]
    fn the_table_at_start() {
        let b = Bindings::new();
        assert_eq!(b.action(Button::Upper, Press::Click), Action::HomeOrSleep);
        assert_eq!(b.action(Button::Upper, Press::DoubleClick), Action::Nothing);
        assert_eq!(b.action(Button::Upper, Press::Long), Action::PowerMenu);
        assert_eq!(b.action(Button::Upper, Press::VeryLong), Action::Restart);
        assert_eq!(b.action(Button::Lower, Press::Click), Action::Open(Page::Status));
        assert_eq!(b.action(Button::Lower, Press::DoubleClick), Action::StepBrightness);
        assert_eq!(b.action(Button::Lower, Press::Long), Action::ToggleHeartRate);
        assert_eq!(b.action(Button::Lower, Press::VeryLong), Action::Nothing);
    }

    #[test]
    fn changing_the_table() {
        let mut b = Bindings::new();
        assert!(b.set(Button::Lower, Press::Click, Action::StepBrightness));
        assert_eq!(b.action(Button::Lower, Press::Click), Action::StepBrightness);
        // The rest is as it was.
        assert_eq!(b.action(Button::Upper, Press::Click), Action::HomeOrSleep);
        assert_eq!(b.action(Button::Lower, Press::Long), Action::ToggleHeartRate);
        // The way out stays where it is, and nowhere else.
        assert!(!b.set(Button::Upper, Press::VeryLong, Action::Nothing));
        assert_eq!(b.action(Button::Upper, Press::VeryLong), Action::Restart);
        assert!(!b.set(Button::Lower, Press::Click, Action::Restart));
        assert_eq!(b.action(Button::Lower, Press::Click), Action::StepBrightness);
    }

    #[test]
    fn double_clicks_are_waited_for_only_if_they_do_something() {
        let mut b = Bindings::new();
        assert!(!b.waits_for_double(Button::Upper));
        assert!(b.waits_for_double(Button::Lower));
        assert!(b.set(Button::Lower, Press::DoubleClick, Action::Nothing));
        assert!(!b.waits_for_double(Button::Lower));
    }

    #[test]
    fn upper_click() {
        let mut h = Hold::new();
        assert_eq!(h.feed(false, 0), None);
        assert_eq!(h.feed(true, 100), None);
        assert_eq!(h.feed(true, 100 + LONG_MS - 1), None);
        assert_eq!(h.feed(false, 100 + LONG_MS - 1), Some(Press::Click));
        assert_eq!(h.feed(false, 5000), None);
    }

    /// A long press comes while the button is down, once, and no click
    /// when it comes up.
    #[test]
    fn upper_long() {
        let mut h = Hold::new();
        h.feed(true, 0);
        assert_eq!(h.feed(true, LONG_MS - 1), None);
        assert_eq!(h.feed(true, LONG_MS), Some(Press::Long));
        assert_eq!(h.feed(true, LONG_MS + 500), None);
        assert_eq!(h.feed(false, LONG_MS + 600), None);
        // The next press starts clean.
        h.feed(true, 20_000);
        assert_eq!(h.feed(false, 20_100), Some(Press::Click));
    }

    #[test]
    fn upper_very_long() {
        let mut h = Hold::new();
        h.feed(true, 0);
        assert_eq!(h.feed(true, LONG_MS), Some(Press::Long));
        assert_eq!(h.feed(true, VERY_LONG_MS - 1), None);
        assert_eq!(h.feed(true, VERY_LONG_MS), Some(Press::VeryLong));
        assert_eq!(h.feed(true, VERY_LONG_MS + 5000), None);
        assert_eq!(h.feed(false, VERY_LONG_MS + 6000), None);
        // Seen only after ten seconds (the firmware was busy): the very
        // long press, not the long one after it.
        let mut h = Hold::new();
        h.feed(true, 0);
        assert_eq!(h.feed(true, VERY_LONG_MS + 1), Some(Press::VeryLong));
        assert_eq!(h.feed(true, VERY_LONG_MS + 2), None);
    }

    #[test]
    fn lower_clicks_without_waiting() {
        let mut t = Taps::new();
        assert_eq!(t.pressed(0, false), Some(Press::Click));
        assert_eq!(t.pressed(100, false), Some(Press::Click));
        assert_eq!(t.tick(1000), None);
    }

    #[test]
    fn lower_click_and_double_click() {
        let mut t = Taps::new();
        // One press: a click once the time for a second has passed.
        assert_eq!(t.pressed(0, true), None);
        assert_eq!(t.tick(DOUBLE_MS), None);
        assert_eq!(t.tick(DOUBLE_MS + 1), Some(Press::Click));
        assert_eq!(t.tick(DOUBLE_MS + 500), None);
        // Two presses in time: a double click and no click.
        assert_eq!(t.pressed(1000, true), None);
        assert_eq!(t.pressed(1000 + DOUBLE_MS, true), Some(Press::DoubleClick));
        assert_eq!(t.tick(5000), None);
        // Two presses too far apart, with nothing looking in between:
        // the first one's click is lost to nobody, it comes with the
        // tick; the second starts over.
        assert_eq!(t.pressed(6000, true), None);
        assert_eq!(t.tick(6000 + DOUBLE_MS + 1), Some(Press::Click));
        assert_eq!(t.pressed(6000 + DOUBLE_MS + 50, true), None);
        assert_eq!(t.tick(6000 + 2 * DOUBLE_MS + 60), Some(Press::Click));
        // Three quick presses: a double click, then a click.
        assert_eq!(t.pressed(9000, true), None);
        assert_eq!(t.pressed(9100, true), Some(Press::DoubleClick));
        assert_eq!(t.pressed(9200, true), None);
        assert_eq!(t.tick(9200 + DOUBLE_MS + 1), Some(Press::Click));
    }
}

/// `app::power`: the rows of the power page.
#[cfg(test)]
mod power {
    use crate::app::power::{choice_at, row_rect, Choice, CHOICES};
    use embedded_graphics::prelude::*;

    #[test]
    fn a_tap_finds_its_row() {
        for (n, &choice) in CHOICES.iter().enumerate() {
            let middle = row_rect(n).center();
            assert_eq!(choice_at(middle.x, middle.y), Some(choice));
        }
        // Between and beside the rows: nothing.
        assert_eq!(choice_at(227, 20), None);
        assert_eq!(choice_at(227, 400), None);
        assert_eq!(choice_at(10, row_rect(0).center().y), None);
        let gap = row_rect(0).top_left.y + row_rect(0).size.height as i32 + 2;
        assert_eq!(choice_at(227, gap), None);
    }

    /// The rows lie inside the round screen and do not overlap.
    #[test]
    fn rows_fit() {
        for n in 0..CHOICES.len() {
            let r = row_rect(n);
            for corner in [r.top_left, r.top_left + Point::new(r.size.width as i32 - 1, 0), r.top_left + Point::new(0, r.size.height as i32 - 1), r.top_left + Point::new(r.size.width as i32 - 1, r.size.height as i32 - 1)] {
                let (dx, dy) = (corner.x - 227, corner.y - 227);
                assert!(dx * dx + dy * dy <= 227 * 227, "row {n} corner {corner:?}");
            }
            if n > 0 {
                assert!(row_rect(n - 1).intersection(&r).is_zero_sized());
            }
        }
    }

    #[test]
    fn every_choice_has_a_row() {
        for (n, &choice) in CHOICES.iter().enumerate() {
            let centre = row_rect(n).center();
            assert_eq!(choice_at(centre.x, centre.y), Some(choice));
        }
        assert!(CHOICES.contains(&Choice::PowerOff));
    }
}

#[cfg(test)]
mod face {
    use crate::app::face::{draw, key, rect, whole, State, Time, ELEMENTS, SIZE};
    use crate::services::font::Paint;
    use embedded_graphics::pixelcolor::Rgb565;
    use embedded_graphics::prelude::*;
    use embedded_graphics::primitives::Rectangle;

    /// A picture that only remembers which pixels were drawn to.
    struct Touched(Vec<bool>);

    impl OriginDimensions for Touched {
        fn size(&self) -> Size {
            Size::new(SIZE as u32, SIZE as u32)
        }
    }

    impl DrawTarget for Touched {
        type Color = Rgb565;
        type Error = core::convert::Infallible;
        fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
            for Pixel(p, _) in pixels {
                assert!((0..SIZE).contains(&p.x) && (0..SIZE).contains(&p.y), "drawn outside the picture at {p:?}");
                self.0[(p.y * SIZE + p.x) as usize] = true;
            }
            Ok(())
        }
    }

    impl Paint for Touched {}

    fn states() -> Vec<State> {
        let mut all = Vec::new();
        for (hour, minute, second, weekday, day, month) in [(23, 42, 37, 2, 24, 11), (0, 0, 0, 1, 1, 1), (8, 8, 59, 3, 31, 5), (19, 51, 15, 7, 9, 9)] {
            for (battery, heart) in [(Some((62, false)), Some(62)), (Some((100, true)), Some(188)), (Some((0, false)), None), (None, None)] {
                all.push(State {
                    time: Some(Time { year: 2026, month, day, weekday, hour, minute, second }),
                    millis: 400,
                    ticks: 12_345,
                    battery,
                    heart,
                    measuring: heart.is_some(),
                    bluetooth: true,
                    worn: Some(true),
                    temperature: Some(268),
                    steps: battery.map(|(percent, _)| percent as u32 * 123),
                    step_goal: 10_000,
                });
            }
        }
        // A clock that was never set.
        all.push(State { time: None, ..all[0] });
        all
    }

    /// The damage tracking repaints an element's rectangle when its key
    /// changes, and nothing else: whatever an element draws outside its
    /// rectangle would be left behind.
    #[test]
    fn every_element_draws_inside_its_rectangle() {
        for state in states() {
            for n in 0..ELEMENTS {
                let mut picture = Touched(vec![false; (SIZE * SIZE) as usize]);
                draw(&mut picture, &whole(), n, &state);
                let r = rect(n, &state);
                for y in 0..SIZE {
                    for x in 0..SIZE {
                        if picture.0[(y * SIZE + x) as usize] {
                            assert!(r.contains(Point::new(x, y)), "element {n} drew at ({x}, {y}), outside {r:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_rectangle_is_inside_the_picture() {
        for state in states() {
            for n in 0..ELEMENTS {
                let r = rect(n, &state);
                assert_eq!(r.intersection(&whole()), r, "element {n}: {r:?}");
            }
        }
    }

    /// Drawing with a clip touches nothing outside the clip.
    #[test]
    fn a_clip_is_kept() {
        let state = states()[0];
        let clip = Rectangle::new(Point::new(150, 120), Size::new(160, 130));
        let mut picture = Touched(vec![false; (SIZE * SIZE) as usize]);
        for n in 0..ELEMENTS {
            draw(&mut picture, &clip, n, &state);
        }
        for y in 0..SIZE {
            for x in 0..SIZE {
                if picture.0[(y * SIZE + x) as usize] {
                    assert!(clip.contains(Point::new(x, y)), "drawn at ({x}, {y}), outside the clip");
                }
            }
        }
    }

    /// What an element shows depends on nothing but what its key covers:
    /// two states with the same key must give the same picture. Checked
    /// for the fields the face does not show at all.
    #[test]
    fn keys_ignore_what_the_face_does_not_show() {
        let a = states()[0];
        let b = State { millis: 900, ticks: 99, bluetooth: false, worn: None, temperature: None, measuring: false, ..a };
        for n in 0..ELEMENTS {
            assert_eq!(key(n, &a), key(n, &b), "element {n}");
            assert_eq!(rect(n, &a), rect(n, &b), "element {n}");
        }
    }
}

/// The screen is round: what a page puts into the corners of the square
/// picture is not seen.
#[cfg(test)]
mod round {
    use crate::app::theme::{C, SIZE};
    use crate::app::{power, status};
    use embedded_graphics::primitives::Rectangle;

    fn on_the_glass(r: &Rectangle) -> bool {
        let (x0, y0) = (r.top_left.x, r.top_left.y);
        let (x1, y1) = (x0 + r.size.width as i32 - 1, y0 + r.size.height as i32 - 1);
        [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].iter().all(|&(x, y)| (x - C).pow(2) + (y - C).pow(2) <= (SIZE / 2).pow(2))
    }

    #[test]
    fn status_rows_are_on_the_glass() {
        for row in 0..status::LINES {
            assert!(on_the_glass(&status::line_rect(row)), "row {row}: {:?}", status::line_rect(row));
        }
    }

    #[test]
    fn power_rows_are_on_the_glass() {
        for n in 0..power::CHOICES.len() {
            assert!(on_the_glass(&power::row_rect(n)), "row {n}");
        }
    }
}

#[cfg(test)]
mod settings {
    use crate::app::settings::{brightness_rect, display_off_rect, switch_rect, tap_at, within, HeartMonitor, Settings, Tap, FILE_SIZE, LEVELS, SWITCHES, TIMEOUTS};
    use crate::app::theme::{C, SIZE};

    fn r_right(r: embedded_graphics::primitives::Rectangle) -> i32 {
        r.top_left.x + r.size.width as i32
    }

    fn every() -> Vec<Settings> {
        let mut all = Vec::new();
        for brightness in std::iter::once(None).chain((0..LEVELS).map(Some)) {
            for &display_off_s in &TIMEOUTS {
                for switches in 0..8 {
                    all.push(Settings {
                        brightness,
                        display_off_s,
                        raise_to_wake: switches & 1 != 0,
                        quiet: switches & 2 != 0,
                        quiet_time: (23 * 60 + 59, 5),
                        move_reminder: switches & 4 != 0,
                        move_time: (8 * 60 + 30, 18 * 60),
                        step_goal: 12_000,
                        heart_monitor: HeartMonitor::ALL[switches as usize % 3],
                    });
                }
            }
        }
        all
    }

    #[test]
    fn the_file_gives_back_what_was_written() {
        for settings in every() {
            let mut text = [0u8; FILE_SIZE];
            let n = settings.text(&mut text);
            assert!(n < text.len(), "the text of {settings:?} fills the buffer");
            assert_eq!(Settings::parse(&text[..n]), settings);
        }
    }

    #[test]
    fn a_file_that_makes_no_sense_gives_the_start_values() {
        assert_eq!(Settings::parse(b""), Settings::START);
        assert_eq!(Settings::parse(&[0xFF; 64]), Settings::START);
        let text = b"brightness=9\ndisplay_off=7\nraise_to_wake=maybe\nnonsense\n=\nquiet_time=25:00-07:00\nmove_time=9-21\nquiet=1\n";
        assert_eq!(Settings::parse(text), Settings::START);
    }

    #[test]
    fn a_file_written_by_hand_is_understood() {
        let text = b"  display_off = 60 \r\nraise_to_wake=off\r\nsomething_new=1\r\nquiet = on\r\nquiet_time = 21:30 - 06:15\r\nbrightness=3";
        let expected = Settings { brightness: Some(2), display_off_s: 60, raise_to_wake: false, quiet: true, quiet_time: (21 * 60 + 30, 6 * 60 + 15), ..Settings::START };
        assert_eq!(Settings::parse(text), expected);
    }

    /// The file written before the quiet hours existed (three lines).
    #[test]
    fn an_older_file_keeps_the_new_settings_at_their_start_values() {
        let text = b"brightness=1\ndisplay_off=15\nraise_to_wake=on\n";
        assert_eq!(Settings::parse(text), Settings { brightness: Some(0), ..Settings::START });
    }

    /// The Apollo's rule for a daily window, which the firmware keeps for
    /// its own quiet hours.
    #[test]
    fn a_window_is_the_apollos() {
        // From before to: both minutes belong to it.
        assert!(within((540, 1260), 540) && within((540, 1260), 1260) && !within((540, 1260), 539) && !within((540, 1260), 1261));
        // Over midnight: from belongs to it, to does not.
        assert!(within((1320, 420), 1320) && within((1320, 420), 0) && within((1320, 420), 419) && !within((1320, 420), 420) && !within((1320, 420), 1319));
        // Equal: the whole day.
        assert!(within((600, 600), 0) && within((600, 600), 600) && within((600, 600), 1439));
        let quiet = Settings { quiet: true, ..Settings::START };
        assert!(quiet.quiet_at(23 * 60) && !quiet.quiet_at(12 * 60) && !Settings::START.quiet_at(23 * 60));
    }

    #[test]
    fn a_tap_in_the_middle_of_a_button_sets_what_it_says() {
        let middle = |r: embedded_graphics::primitives::Rectangle| r.center();
        let at = middle(brightness_rect(0));
        assert_eq!(tap_at(at.x, at.y), Some(Tap::Brightness(None)));
        for step in 0..LEVELS {
            let at = middle(brightness_rect(step as usize + 1));
            assert_eq!(tap_at(at.x, at.y), Some(Tap::Brightness(Some(step))));
        }
        for (n, &seconds) in TIMEOUTS.iter().enumerate() {
            let at = middle(display_off_rect(n));
            assert_eq!(tap_at(at.x, at.y), Some(Tap::DisplayOff(seconds)));
        }
        for (n, &tap) in SWITCHES.iter().enumerate() {
            let at = middle(switch_rect(n));
            // On the switch; the rest of a row with a time leads to setting it.
            let on_switch = r_right(switch_rect(n)) - 20;
            assert_eq!(tap_at(on_switch, at.y), Some(tap));
            let beside = switch_rect(n).top_left.x + 10;
            let expected = match tap {
                Tap::Quiet => Some(Tap::PickQuietTime),
                Tap::MoveReminder => Some(Tap::PickMoveTime),
                _ => None,
            };
            assert_eq!(tap_at(beside, at.y), expected);
        }
        // The title and the handle set nothing.
        for y in [40, 425] {
            assert_eq!(tap_at(C, y), None, "y = {y}");
        }
    }

    #[test]
    fn a_tap_changes_one_thing() {
        let start = Settings::START;
        assert_eq!(start.after(Tap::Brightness(Some(1))), Settings { brightness: Some(1), ..start });
        assert_eq!(start.after(Tap::DisplayOff(60)), Settings { display_off_s: 60, ..start });
        assert_eq!(start.after(Tap::Quiet), Settings { quiet: true, ..start });
        assert_eq!(start.after(Tap::MoveReminder), Settings { move_reminder: true, ..start });
        assert_eq!(start.after(Tap::RaiseToWake).after(Tap::RaiseToWake), start);
        assert_eq!(start.after(Tap::PickQuietTime), start);
        // The choices of a row go round and round.
        let mut s = start;
        for _ in 0..=LEVELS {
            s = s.after(Tap::NextBrightness);
        }
        assert_eq!(s, start);
        assert_eq!(start.after(Tap::NextDisplayOff).display_off_s, 30);
        assert_eq!(Settings { display_off_s: 60, ..start }.after(Tap::NextDisplayOff).display_off_s, 10);
        assert_eq!(start.after(Tap::NextStepGoal).step_goal, 12_000);
        assert_eq!(Settings { step_goal: 15_000, ..start }.after(Tap::NextStepGoal).step_goal, 6_000);
        let monitors: Vec<_> = (0..3).scan(start, |s, _| { *s = s.after(Tap::NextHeartMonitor); Some(s.heart_monitor) }).collect();
        assert_eq!(monitors, [HeartMonitor::Periodic, HeartMonitor::Always, HeartMonitor::Off]);
        assert_eq!(start.after(Tap::QuietTime((60, 120))), Settings { quiet_time: (60, 120), ..start });
        assert_eq!(start.after(Tap::MoveTime((600, 660))), Settings { move_time: (600, 660), ..start });
    }

    #[test]
    fn everything_to_tap_is_on_the_glass() {
        let mut rects: Vec<_> = (0..SWITCHES.len()).map(switch_rect).collect();
        rects.extend((0..=LEVELS as usize).map(brightness_rect));
        rects.extend((0..TIMEOUTS.len()).map(display_off_rect));
        for r in rects {
            let (x0, y0) = (r.top_left.x, r.top_left.y);
            let (x1, y1) = (x0 + r.size.width as i32 - 1, y0 + r.size.height as i32 - 1);
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                assert!((x - C).pow(2) + (y - C).pow(2) <= (SIZE / 2).pow(2), "{r:?}");
            }
        }
    }
}

#[cfg(test)]
mod alerts {
    use crate::app::alerts::{card_rect, Alert, Alerts, Kind, SHOWN};
    use crate::app::theme::{C, SIZE};

    #[test]
    fn the_newest_alert_is_first_and_the_oldest_drops_out() {
        let mut alerts = Alerts::new();
        assert_eq!(alerts.count(), 0);
        for minute in 0..SHOWN as u8 + 2 {
            alerts.add(Alert { kind: Kind::Move, at: Some((10, minute)) });
        }
        assert_eq!(alerts.count(), SHOWN);
    }

    #[test]
    fn the_cards_are_on_the_glass() {
        for n in 0..SHOWN {
            let r = card_rect(n);
            let (x0, y0) = (r.top_left.x, r.top_left.y);
            let (x1, y1) = (x0 + r.size.width as i32 - 1, y0 + r.size.height as i32 - 1);
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                assert!((x - C).pow(2) + (y - C).pow(2) <= (SIZE / 2).pow(2), "card {n}: {r:?}");
            }
        }
    }
}

#[cfg(test)]
mod picker {
    use crate::app::picker::{button_rect, wheels_rect, Outcome, Picker, PITCH};
    use crate::app::theme::{C, SIZE};

    /// Turn a wheel `dy` pixels down in steps of 20 ms, the last
    /// `fast` pixels of the way within the final 40 ms, and let go.
    /// Returns when the finger lifted, in milliseconds.
    fn turn(picker: &mut Picker, wheel: usize, dy: i32, slow_ms: u64) -> u64 {
        let at = picker.wheel_rect(wheel).center();
        assert!(picker.touch(Some((at.x, at.y)), 0));
        let steps = 10;
        for i in 1..=steps {
            picker.touch(Some((at.x, at.y + dy * i / steps)), i as u64 * slow_ms);
        }
        let lifted = steps as u64 * slow_ms + 1;
        assert!(picker.touch(None, lifted));
        lifted
    }

    /// Let the wheel run out, however long it takes.
    fn rest(picker: &mut Picker, lifted: u64) {
        let mut moved = 0;
        for ms in (lifted..lifted + 2000).step_by(16) {
            moved += picker.tick(ms) as u32;
        }
        assert!(!picker.tick(lifted + 3000), "still running after 3 s");
        let _ = moved;
    }

    #[test]
    fn a_wheel_moved_slowly_rests_on_the_nearest_value() {
        let mut picker = Picker::new("T", 10 * 60 + 30);
        // Down by one value and a little, slowly (100 ms a step, so the
        // speed adds next to nothing): the value before comes to the middle.
        let lifted = turn(&mut picker, 1, PITCH + 4, 100);
        assert_eq!(picker.seconds() / 60, 10 * 60 + 29);
        rest(&mut picker, lifted);
        // Up by three values.
        let lifted = turn(&mut picker, 0, -3 * PITCH, 100);
        assert_eq!(picker.seconds() / 60, 13 * 60 + 29);
        rest(&mut picker, lifted);
        assert_eq!(picker.seconds() / 60, 13 * 60 + 29);
    }

    #[test]
    fn a_flick_carries_further() {
        let mut picker = Picker::new("T", 12 * 60);
        // 100 pixels up in 50 ms: 2000 pixels a second, taken to last
        // 150 ms, is another 300 pixels; 400 in all, seven values.
        turn(&mut picker, 1, -100, 5);
        assert_eq!(picker.seconds() / 60, 12 * 60 + 7);
    }

    #[test]
    fn a_wheel_let_go_runs_out_and_stops() {
        let mut picker = Picker::new("T", 12 * 60);
        let lifted = turn(&mut picker, 1, -100, 5);
        // It moves for a while, by less and less, and then no more.
        let mut moves = Vec::new();
        for ms in (lifted..lifted + 1000).step_by(16) {
            moves.push(picker.tick(ms));
        }
        assert!(moves[1] && moves[5], "the wheel does not run");
        assert!(!moves[moves.len() - 1], "the wheel does not stop");
        assert_eq!(picker.seconds() / 60, 12 * 60 + 7);
    }

    #[test]
    fn a_touch_that_hardly_moves_turns_nothing() {
        let mut picker = Picker::new("T", 8 * 60);
        let lifted = turn(&mut picker, 0, 10, 2);
        assert_eq!(picker.seconds() / 60, 8 * 60);
        rest(&mut picker, lifted);
        assert_eq!(picker.seconds() / 60, 8 * 60);
    }

    #[test]
    fn the_wheels_go_round() {
        let mut picker = Picker::new("T", 0);
        turn(&mut picker, 0, PITCH, 100);
        turn(&mut picker, 1, PITCH, 100);
        assert_eq!(picker.seconds() / 60, 23 * 60 + 59);
        turn(&mut picker, 0, -PITCH, 100);
        turn(&mut picker, 1, -PITCH, 100);
        assert_eq!(picker.seconds() / 60, 0);
    }

    #[test]
    fn a_tap_above_or_below_the_middle_steps_by_one() {
        let mut picker = Picker::new("T", 6 * 60 + 15);
        let hours = picker.wheel_rect(0);
        let (x, top) = (hours.center().x, hours.top_left.y);
        assert_eq!(picker.tap(x, top + 5), Outcome::Stepped);
        assert_eq!(picker.seconds() / 60, 5 * 60 + 15);
        assert_eq!(picker.tap(x, top + 3 * PITCH - 5), Outcome::Stepped);
        assert_eq!(picker.tap(x, top + 3 * PITCH - 5), Outcome::Stepped);
        assert_eq!(picker.seconds() / 60, 7 * 60 + 15);
        // The middle row itself does nothing.
        assert_eq!(picker.tap(x, top + PITCH + PITCH / 2), Outcome::Nothing);
    }

    #[test]
    fn the_buttons_give_the_time_back_or_not() {
        let mut picker = Picker::new("T", 22 * 60);
        let (cancel, save) = (button_rect(0).center(), button_rect(1).center());
        assert_eq!(picker.tap(cancel.x, cancel.y), Outcome::Cancel);
        assert_eq!(picker.tap(save.x, save.y), Outcome::Save(22 * 3600));
        // A finger that comes down beside the wheels takes hold of nothing.
        assert!(!picker.touch(Some((save.x, save.y)), 0));
        assert!(!picker.touch(Some((save.x, save.y + 60)), 20) && !picker.touch(None, 40));
        assert_eq!(picker.seconds() / 60, 22 * 60);
    }

    #[test]
    fn everything_is_on_the_glass() {
        let (two, three) = (Picker::new("T", 0), Picker::with_seconds("T", 0));
        let wheels = [two.wheel_rect(0), two.wheel_rect(1), three.wheel_rect(0), three.wheel_rect(1), three.wheel_rect(2), wheels_rect()];
        for r in wheels.into_iter().chain([button_rect(0), button_rect(1)]) {
            let (x0, y0) = (r.top_left.x, r.top_left.y);
            let (x1, y1) = (x0 + r.size.width as i32 - 1, y0 + r.size.height as i32 - 1);
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                assert!((x - C).pow(2) + (y - C).pow(2) <= (SIZE / 2).pow(2), "{r:?}");
            }
        }
    }

    #[test]
    fn three_wheels_set_seconds_too() {
        let mut picker = Picker::with_seconds("T", 25 * 60 + 30);
        assert_eq!(picker.seconds(), 25 * 60 + 30);
        turn(&mut picker, 2, -2 * PITCH, 100);
        turn(&mut picker, 0, -PITCH, 100);
        assert_eq!(picker.seconds(), 3600 + 25 * 60 + 32);
        // With two wheels there are no seconds, whatever was asked for.
        let picker = Picker::new("T", 61);
        assert_eq!(picker.seconds(), 3660);
    }

    /// What is drawn again when a wheel turns covers every wheel.
    #[test]
    fn a_turn_repaints_every_wheel() {
        let all = wheels_rect();
        for (picker, wheels) in [(Picker::new("T", 0), 2), (Picker::with_seconds("T", 0), 3)] {
            for n in 0..wheels {
                assert_eq!(picker.wheel_rect(n).intersection(&all), picker.wheel_rect(n));
            }
        }
    }
}

#[cfg(test)]
mod apollo_messages {
    use crate::drivers::apollo_messages::{heart_monitor, heart_rate, Readings, DAILY, HEART};

    /// The summary of the day's activity as it arrived on the watch
    /// (2026-11-24, 12 s after a start): time, 42, and 180 bytes that
    /// begin with the step count.
    #[test]
    fn the_step_count_is_the_first_four_bytes_of_the_summary() {
        let mut payload = vec![0x08, 0xd9, 0xcd, 0x94, 0xd8, 0x06, 0x10, 0x2a, 0x1a, 0xb4, 0x01];
        let mut bytes = [0u8; 180];
        let mut readings = Readings::default();
        readings.update(DAILY, &[payload.clone(), bytes.to_vec()].concat());
        assert_eq!(readings.steps, Some(0));
        bytes[..4].copy_from_slice(&8_412u32.to_le_bytes());
        payload.extend_from_slice(&bytes);
        readings.update(DAILY, &payload);
        assert_eq!(readings.steps, Some(8_412));
        // A summary cut short changes nothing.
        readings.update(DAILY, &[0x1a, 0x02, 0x01, 0x02]);
        assert_eq!(readings.steps, Some(8_412));
    }

    /// The messages byte for byte: a field is its number times eight
    /// and then its value; fields that are zero are left out.
    #[test]
    fn heart_monitoring_is_sent_as_read() {
        // None: what the stock app sends at its start.
        assert_eq!(heart_monitor(0, 10).as_bytes(), [0x18, 40, 0x20, 100, 0x38, 1]);
        // Periodic, results every 10 s.
        assert_eq!(heart_monitor(1, 10).as_bytes(), [0x08, 1, 0x18, 40, 0x20, 100, 0x38, 10, 0x40, 1]);
        assert_eq!(heart_monitor(2, 10).as_bytes(), [0x08, 1, 0x18, 40, 0x20, 100, 0x38, 10, 0x40, 2]);
        // A measurement by hand, as it has been sent all along.
        assert_eq!(heart_rate(true).as_bytes(), [0x08, 1, 0x18, 40, 0x38, 1, 0x40, 3]);
        assert_eq!(heart_rate(false).as_bytes(), [0x18, 40, 0x38, 1, 0x40, 3]);
    }

    #[test]
    fn only_a_valid_heart_rate_is_kept_as_the_latest() {
        let mut readings = Readings::default();
        // Field 2 beats, 4 quality, 8 what it is about, 25 (two bytes
        // of tag) status.
        let report = |bpm: u8, about: u8, status: u8| vec![0x10, bpm, 0x20, 6, 0x40, about, 0xc8, 0x01, status];
        readings.update(HEART, &report(0, 3, 0));
        assert_eq!((readings.heart_bpm, readings.heart_last), (Some(0), None));
        readings.update(HEART, &report(64, 3, 1));
        assert_eq!((readings.heart_bpm, readings.heart_last), (Some(64), Some(64)));
        // Not worn: the live value goes, the latest valid one stays.
        readings.update(HEART, &report(0, 3, 3));
        assert_eq!((readings.heart_bpm, readings.heart_last), (Some(0), Some(64)));
        // A stress or SpO2 report is not a heart rate.
        readings.update(HEART, &report(99, 1, 1));
        readings.update(HEART, &report(99, 2, 1));
        assert_eq!((readings.heart_bpm, readings.heart_last), (Some(0), Some(64)));
        // Nor is a value of low quality: as the watch sent it, the first
        // of a periodic measurement, 100 beats with quality 1.
        readings.update(HEART, &[0x10, 100, 0x20, 1, 0x40, 3, 0xc8, 0x01, 1]);
        assert_eq!((readings.heart_bpm, readings.heart_last), (Some(100), Some(64)));
        // Out of range is not kept either.
        readings.update(HEART, &report(30, 3, 1));
        assert_eq!(readings.heart_last, Some(64));
    }
}

#[cfg(test)]
mod launcher {
    use crate::app::launcher::{Launcher, Outcome, APPS};
    use crate::app::theme::{C, SIZE};

    #[test]
    fn a_tap_chooses_and_a_second_one_opens() {
        let mut launcher = Launcher::new();
        // The first app is chosen from the start: a tap on it opens it.
        let first = launcher.tile_rect(0).unwrap().center();
        assert_eq!(launcher.tap(first.x, first.y), Outcome::Open(APPS[0]));
        for n in 1..APPS.len() {
            let tile = launcher.tile_rect(n).unwrap().center();
            assert_eq!(launcher.tap(tile.x, tile.y), Outcome::Chosen, "app {n}");
            assert_eq!(launcher.tap(tile.x, tile.y), Outcome::Open(APPS[n]), "app {n}");
            // The middle opens the chosen one too.
            assert_eq!(launcher.tap(C, C), Outcome::Open(APPS[n]), "app {n}");
        }
        // Between the tiles and the middle nothing happens.
        assert_eq!(launcher.tap(C, C + 100), Outcome::Nothing);
    }

    #[test]
    fn the_tiles_stand_apart_and_on_the_glass() {
        let launcher = Launcher::new();
        let tiles: Vec<_> = (0..APPS.len()).map(|n| launcher.tile_rect(n).unwrap()).collect();
        for (n, r) in tiles.iter().enumerate() {
            let (x0, y0) = (r.top_left.x, r.top_left.y);
            let (x1, y1) = (x0 + r.size.width as i32 - 1, y0 + r.size.height as i32 - 1);
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                assert!((x - C).pow(2) + (y - C).pow(2) <= (SIZE / 2).pow(2), "tile {n}: {r:?}");
            }
            for other in &tiles[n + 1..] {
                assert!(r.intersection(other).is_zero_sized(), "tiles overlap: {r:?} {other:?}");
            }
        }
        // The first one stands at the top.
        assert_eq!(tiles[0].center().x, C);
        assert!(tiles[0].center().y < C);
    }
}

#[cfg(test)]
mod menu {
    use crate::app::menu::{longest, tap_at, window, Hit, Row, Scroll, ROW, ROWS};
    use crate::app::settings::Tap;
    use crate::app::theme::{C, SIZE};

    /// Drag the list by `dy` (up is negative) in ten steps of `step_ms`
    /// and let go. Returns when the finger lifted.
    fn drag(scroll: &mut Scroll, dy: i32, step_ms: u64) -> u64 {
        let at = window().center();
        scroll.touch(Some((at.x, at.y)), 0);
        for i in 1..=10 {
            scroll.touch(Some((at.x, at.y + dy * i / 10)), i as u64 * step_ms);
        }
        scroll.touch(None, 10 * step_ms + 1);
        10 * step_ms + 1
    }

    #[test]
    fn the_list_is_longer_than_its_window_and_on_the_glass() {
        assert!(longest() > 0, "nothing to scroll: the scrolling is not exercised");
        let r = window();
        let (x0, y0) = (r.top_left.x, r.top_left.y);
        let (x1, y1) = (x0 + r.size.width as i32 - 1, y0 + r.size.height as i32 - 1);
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            assert!((x - C).pow(2) + (y - C).pow(2) <= (SIZE / 2).pow(2), "{r:?}");
        }
    }

    #[test]
    fn the_list_follows_the_finger_and_stops_at_its_ends() {
        let mut scroll = Scroll::new();
        // Slowly up by 40 pixels: the list is 40 further on.
        drag(&mut scroll, -40, 100);
        assert_eq!(scroll.moved(), 40);
        // Down by far more than there is: back at the top, not beyond.
        drag(&mut scroll, 300, 100);
        assert_eq!(scroll.moved(), 0);
        drag(&mut scroll, -1000, 100);
        assert_eq!(scroll.moved(), longest());
    }

    #[test]
    fn a_tap_does_not_nudge_the_list() {
        let mut scroll = Scroll::new();
        drag(&mut scroll, -40, 100);
        drag(&mut scroll, -8, 5);
        assert_eq!(scroll.moved(), 40);
        assert!(!scroll.tick(2000));
    }

    #[test]
    fn a_flick_runs_on_and_comes_to_rest() {
        let mut scroll = Scroll::new();
        // 30 pixels up in 50 ms.
        let lifted = drag(&mut scroll, -30, 5);
        let at_lift = scroll.moved();
        let mut moves = 0;
        for ms in (lifted..lifted + 3000).step_by(16) {
            moves += scroll.tick(ms) as u32;
        }
        assert!(scroll.moved() > at_lift, "the list does not run on");
        assert!(moves > 3);
        assert!(!scroll.tick(lifted + 4000), "the list does not come to rest");
        assert!(scroll.moved() <= longest());
    }

    #[test]
    fn a_tap_lands_on_the_row_under_it_wherever_the_list_stands() {
        let r = window();
        let (left, right) = (r.top_left.x + 20, r.top_left.x + r.size.width as i32 - 20);
        for moved in [0, 30, longest()] {
            for (n, &row) in ROWS.iter().enumerate() {
                let y = r.top_left.y + n as i32 * ROW - moved + ROW / 2;
                let hit = tap_at(left, y, moved);
                if y < r.top_left.y || y >= r.top_left.y + r.size.height as i32 {
                    assert_eq!(hit, None, "row {n} is out of the window");
                    continue;
                }
                let expected = match row {
                    Row::Quick => Some(Hit::Quick),
                    Row::Brightness => Some(Hit::Setting(Tap::NextBrightness)),
                    Row::DisplayOff => Some(Hit::Setting(Tap::NextDisplayOff)),
                    Row::StepGoal => Some(Hit::Setting(Tap::NextStepGoal)),
                    Row::HeartMonitor => Some(Hit::Setting(Tap::NextHeartMonitor)),
                    Row::Quiet => Some(Hit::Setting(Tap::PickQuietTime)),
                    Row::MoveAlert => Some(Hit::Setting(Tap::PickMoveTime)),
                    Row::RaiseToWake => None,
                };
                assert_eq!(hit, expected, "row {n}, list moved {moved}");
                // On the switch, the rows that have one switch.
                let switched = match row {
                    Row::RaiseToWake => Some(Hit::Setting(Tap::RaiseToWake)),
                    Row::Quiet => Some(Hit::Setting(Tap::Quiet)),
                    Row::MoveAlert => Some(Hit::Setting(Tap::MoveReminder)),
                    _ => expected,
                };
                assert_eq!(tap_at(right, y, moved), switched, "row {n}, on the switch");
            }
        }
        // Above the window (the title) nothing is hit.
        assert_eq!(tap_at(C, 40, 0), None);
    }
}
