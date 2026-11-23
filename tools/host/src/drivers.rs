//! Stand-ins for what the firmware's drivers offer, and the driver files
//! that are pure logic, used as they are.

/// Stand-in for the firmware's touch driver: only the event type.
pub mod touch {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum TouchEvent {
        Down { x: u16, y: u16 },
        Up,
    }
}

#[path = "../../../src/drivers/touch_report.rs"]
pub mod touch_report;
