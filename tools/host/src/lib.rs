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
    use crate::services::gestures::{Gesture, Gestures};
    use embassy_time::{Duration, Instant};

    fn at(ms: u64) -> Instant {
        Instant::from_ticks(0) + Duration::from_millis(1_000_000 + ms)
    }

    #[test]
    fn tap() {
        let mut g = Gestures::new();
        assert_eq!(g.feed(Some(Down { x: 100, y: 200 }), at(0)), None);
        assert_eq!(g.feed(None, at(50)), None);
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
        assert_eq!(g.feed(None, at(900)), None);
        assert_eq!(g.feed(Some(Down { x: 11, y: 10 }), at(950)), None);
        assert_eq!(g.feed(Some(Up), at(1000)), None);
        // The next touch starts clean.
        g.feed(Some(Down { x: 10, y: 10 }), at(2000));
        assert_eq!(g.feed(Some(Up), at(2050)), Some(Gesture::Tap { x: 10, y: 10 }));
    }

    #[test]
    fn no_drag_after_long_press() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 200, y: 200 }), at(0));
        assert!(matches!(g.feed(None, at(700)), Some(Gesture::LongPress { .. })));
        assert_eq!(g.feed(Some(Down { x: 100, y: 200 }), at(800)), None);
        assert_eq!(g.feed(Some(Up), at(900)), None);
    }

    #[test]
    fn drag_and_flick() {
        let mut g = Gestures::new();
        g.feed(Some(Down { x: 300, y: 200 }), at(0));
        assert_eq!(g.feed(Some(Down { x: 290, y: 200 }), at(20)), None);
        assert_eq!(g.feed(Some(Down { x: 280, y: 202 }), at(40)), Some(Gesture::DragStart { x: 300, y: 200, dx: -20, dy: 2 }));
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
