//! The firmware's pure logic, built for the PC so it can be tested there.
//!
//! Each module below is the firmware's own source file. What such a file
//! needs from the rest of the firmware is stood in for here by the least
//! that will do.

pub mod drivers {
    /// Stand-in for the firmware's touch driver: only the event type.
    pub mod touch {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum TouchEvent {
            Down { x: u16, y: u16 },
            Up,
        }
    }
}
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

/// `app::pages`: which gesture leads from which page to which.
#[cfg(test)]
mod pages {
    use crate::app::motion::Motion;
    use crate::app::pages::{drag, long_press, tap, Page};
    use crate::services::gestures::Direction::{self, Down, Left, Right, Up};

    const PAGES: [Page; 6] = [Page::Face, Page::Status, Page::Above, Page::Below, Page::Faces, Page::Cards];
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

    #[test]
    fn long_press_and_back() {
        assert_eq!(long_press(Page::Face), Some(Page::Faces));
        assert_eq!(long_press(Page::Status), Some(Page::Cards));
        for page in [Page::Above, Page::Below, Page::Faces, Page::Cards] {
            assert_eq!(long_press(page), None);
        }
        // A tap leaves what a long press opened, back to where it came from.
        for page in PAGES {
            match long_press(page) {
                Some(opened) => assert_eq!(tap(opened), Some(page)),
                None => {}
            }
        }
        for page in [Page::Face, Page::Status, Page::Above, Page::Below] {
            assert_eq!(tap(page), None);
        }
    }
}
