//! Automatic display brightness: from the ambient light in lux to the
//! panel's brightness value.
//!
//! The curve is our own choice, not the stock firmware's (its table has not
//! been looked for). It is anchored on the one known point: the stock app
//! starts the panel at 0x85, and a desk under room light reads about 270 lux.
//!
//! Two things keep the display from flickering:
//!   - the light readings are averaged (about one second at four readings a
//!     second), so a hand passing over the watch does little;
//!   - the wanted brightness only moves once the curve is `HYSTERESIS` away
//!     from it, so light sitting at the edge between two values does not
//!     make it step back and forth.
//!
//! Moving the panel to the wanted value in small steps is the caller's job.

/// (lux, panel brightness), ascending; straight lines in between.
const CURVE: [(u32, u8); 6] = [(0, 0x10), (10, 0x28), (50, 0x50), (250, 0x85), (1000, 0xC0), (5000, 0xFF)];
/// How far the curve must be from the wanted value before that follows.
const HYSTERESIS: u8 = 10;
/// Weight of a new reading in the average: 1 / this.
const SMOOTHING: u32 = 4;

/// The panel brightness the curve gives for `lux`.
pub fn curve(lux: u32) -> u8 {
    let mut below = CURVE[0];
    for above in CURVE {
        if lux < above.0 {
            let span = above.0 - below.0;
            let rise = (above.1 - below.1) as u32;
            return below.1 + ((lux - below.0) * rise / span) as u8;
        }
        below = above;
    }
    below.1
}

pub struct AutoBrightness {
    /// Averaged light in 1/16 lux; `None` before the first reading.
    average: Option<u32>,
    wanted: u8,
}

impl AutoBrightness {
    /// `current` is what the panel is set to now; it stays the wanted value
    /// until the light says otherwise.
    pub fn new(current: u8) -> Self {
        Self { average: None, wanted: current }
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
        let now = curve(average / 16);
        // The ends of the curve are always reached, hysteresis or not.
        let at_end = now == CURVE[0].1 || now == CURVE[CURVE.len() - 1].1;
        if now.abs_diff(self.wanted) >= HYSTERESIS || (at_end && now != self.wanted) {
            self.wanted = now;
            return true;
        }
        false
    }
}
