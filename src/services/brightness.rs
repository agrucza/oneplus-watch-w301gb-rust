//! Automatic display brightness: from the ambient light in lux to the
//! panel's brightness value.
//!
//! The steps are the stock app's (`0x0804149A`, run once a second there):
//! seven ranges of lux, each with a brightness value from the table in its
//! initialised data (RAM 0x2002C0D9).
//!
//! Our own additions keep the display from flickering:
//!   - the light readings are averaged (about one second at four readings a
//!     second), so a hand passing over the watch does little;
//!   - a step is only left once the light is 10 % past its limit, so light
//!     sitting right at a limit does not make the display step back and
//!     forth.
//!
//! Moving the panel to the wanted value in small steps is the caller's job.

/// (from this many lux, panel brightness), ascending. Stock: below 11 lux,
/// 11-40, 41-360, 361-1200, 1201-5000, 5001-8000, above 8000.
const STEPS: [(u32, u8); 7] = [(0, 0x39), (11, 0x50), (41, 0x6E), (361, 0x85), (1201, 0xAC), (5001, 0xC8), (8001, 0xD7)];
/// How far past a limit the light must be before the step changes, in percent.
const MARGIN_PERCENT: u32 = 10;
/// Weight of a new reading in the average: 1 / this.
const SMOOTHING: u32 = 4;

/// The step the stock app would choose for `lux`.
fn step_for(lux: u32) -> usize {
    STEPS.iter().rposition(|&(from, _)| lux >= from).unwrap_or(0)
}

pub struct AutoBrightness {
    /// Averaged light in 1/16 lux; `None` before the first reading.
    average: Option<u32>,
    /// The current step; `None` before the first reading.
    step: Option<usize>,
    wanted: u8,
}

impl AutoBrightness {
    /// `current` is what the panel is set to now; it stays the wanted value
    /// until the first light reading.
    pub fn new(current: u8) -> Self {
        Self { average: None, step: None, wanted: current }
    }

    /// The brightness the panel should have.
    pub fn wanted(&self) -> u8 {
        self.wanted
    }

    /// The averaged light in lux; 0 before the first reading.
    pub fn lux(&self) -> u32 {
        self.average.unwrap_or(0) / 16
    }

    /// Take a light reading. Returns true if the wanted brightness changed.
    pub fn feed(&mut self, lux: u32) -> bool {
        let reading = lux.min(100_000) * 16;
        let average = match self.average {
            // The first reading counts in full: no slow start from zero.
            None => reading,
            Some(a) => (a * (SMOOTHING - 1) + reading) / SMOOTHING,
        };
        self.average = Some(average);
        let lux = average / 16;
        let step = match self.step {
            None => step_for(lux),
            Some(mut step) => {
                // Up while the light is clearly past the next limit, down
                // while it is clearly below this step's own.
                while step + 1 < STEPS.len() && lux * 100 >= STEPS[step + 1].0 * (100 + MARGIN_PERCENT) {
                    step += 1;
                }
                while step > 0 && lux * 100 < STEPS[step].0 * (100 - MARGIN_PERCENT) {
                    step -= 1;
                }
                step
            }
        };
        let changed = self.step != Some(step);
        self.step = Some(step);
        self.wanted = STEPS[step].1;
        changed
    }
}
