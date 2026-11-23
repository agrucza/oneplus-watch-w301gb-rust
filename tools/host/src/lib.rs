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

    const PAGES: [Page; 7] = [Page::Face, Page::Status, Page::Above, Page::Below, Page::Faces, Page::Cards, Page::Power];
    const DIRECTIONS: [Direction; 4] = [Left, Right, Up, Down];

    /// Every page with every direction: the whole table.
    #[test]
    fn drags() {
        for page in PAGES {
            for direction in DIRECTIONS {
                let expected = match (page, direction) {
                    (Page::Face, Left) => Some((Page::Status, Motion::Pages { leftwards: true })),
                    (Page::Face, Right) => Some((Page::Face, Motion::Give { leftwards: false })),
                    (Page::Face, Down) => Some((Page::Above, Motion::PanelIn { from_top: true })),
                    (Page::Face, Up) => Some((Page::Below, Motion::PanelIn { from_top: false })),
                    (Page::Status, Right) => Some((Page::Face, Motion::Pages { leftwards: false })),
                    (Page::Status, Left) => Some((Page::Status, Motion::Give { leftwards: true })),
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

    /// Every page has frames of its own, and together they are exactly
    /// the frames there are.
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
                assert_eq!(owner[frame], None, "{page:?}");
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
        for page in [Page::Above, Page::Below, Page::Faces, Page::Cards, Page::Power] {
            assert_eq!(long_press(page), None);
        }
        // A tap leaves what a long press opened, back to where it came from.
        for page in PAGES {
            match long_press(page) {
                Some(opened) => assert_eq!(tap(opened), Some(page)),
                None => {}
            }
        }
        for page in [Page::Face, Page::Status, Page::Above, Page::Below, Page::Power] {
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
