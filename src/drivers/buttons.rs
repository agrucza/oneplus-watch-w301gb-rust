//! The upper side button, on PC13 (pull-up; pressed reads low). Measured on
//! the watch; the stock app sets the pin up at 0x0805DBB8 with an interrupt
//! on both edges. PC13 is also a wake-up pin of the STM32.
//!
//! The lower button is not on the STM32 at all: it is wired to the Apollo,
//! which reports it in messages (see `apollo_messages::KeyEvent`).
//!
//! The button is sampled from the main loop (every 20 ms or so); a level has
//! to be seen twice in a row before it counts.

use core::fmt::Write;

use embassy_stm32::gpio::{Input, Pull};
use embassy_stm32::{peripherals, Peri};
use embassy_time::Instant;

use crate::services::log::Log;

pub struct Buttons {
    upper: Input<'static>,
    pressed: bool,
    last_sample: bool,
    since: Instant,
}

impl Buttons {
    pub fn start(pc13: Peri<'static, peripherals::PC13>) -> Self {
        let upper = Input::new(pc13, Pull::Up);
        // Let the pull-up bring the line up before the first sample.
        cortex_m::asm::delay(96_000);
        let low = upper.is_low();
        Self { upper, pressed: low, last_sample: low, since: Instant::now() }
    }

    /// Is the upper button held down right now?
    #[allow(dead_code)]
    pub fn upper_pressed(&self) -> bool {
        self.pressed
    }

    /// Sample the button; changes go to the log.
    pub fn poll(&mut self, log: &mut Log) {
        let low = self.upper.is_low();
        if low == self.last_sample && low != self.pressed {
            self.pressed = low;
            if low {
                let _ = writeln!(log.stamp(), "button upper: pressed");
            } else {
                let _ = writeln!(log.stamp(), "button upper: released after {} ms", self.since.elapsed().as_millis());
            }
            self.since = Instant::now();
        }
        self.last_sample = low;
    }
}
