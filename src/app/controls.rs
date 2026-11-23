//! What the two buttons do: which kind of press of which button leads to
//! which action. The table can be changed while the firmware runs; what
//! it holds at start is `Bindings::new`.
//!
//! The upper button is wired to this processor and is read here, press
//! and release (`Hold`). The lower one is wired to the Apollo, which
//! reports its presses; short ones are told apart as click and double
//! click here (`Taps`), long ones by the Apollo.

use super::pages::Page;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Upper,
    Lower,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Press {
    Click,
    DoubleClick,
    Long,
    VeryLong,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Nothing,
    /// A dark display is shown. A lit one goes back to the watch face
    /// from any other page, and dark from the watch face.
    HomeOrSleep,
    /// Open the page with restart, power off and the way back to the
    /// stock firmware.
    PowerMenu,
    /// Open a page; if it is the one shown, go back to the watch face.
    Open(Page),
    /// Brightness: automatic, then the fixed steps in turn.
    StepBrightness,
    /// Start or stop a heart-rate measurement.
    ToggleHeartRate,
    /// Restart the watch at once, without asking. Only the upper
    /// button's very long press does this, and that cannot be changed:
    /// it is the way out if the screen no longer answers.
    Restart,
}

/// The table: an action for every kind of press of each button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bindings {
    actions: [[Action; 4]; 2],
}

fn row(button: Button) -> usize {
    match button {
        Button::Upper => 0,
        Button::Lower => 1,
    }
}

fn column(press: Press) -> usize {
    match press {
        Press::Click => 0,
        Press::DoubleClick => 1,
        Press::Long => 2,
        Press::VeryLong => 3,
    }
}

impl Bindings {
    /// The table as it is at start.
    pub const fn new() -> Self {
        Self {
            actions: [
                // Upper: click, double click (not told apart from two
                // clicks, see `Hold`), long, very long.
                [Action::HomeOrSleep, Action::Nothing, Action::PowerMenu, Action::Restart],
                // Lower.
                [Action::Open(Page::Status), Action::StepBrightness, Action::ToggleHeartRate, Action::Nothing],
            ],
        }
    }

    pub fn action(&self, button: Button, press: Press) -> Action {
        self.actions[row(button)][column(press)]
    }

    /// Change what a press does. False, and nothing changed, for what is
    /// fixed: the upper button's very long press, and `Action::Restart`
    /// anywhere else.
    #[allow(dead_code)] // for the settings to come; tested on the PC
    pub fn set(&mut self, button: Button, press: Press, action: Action) -> bool {
        if (button, press) == (Button::Upper, Press::VeryLong) || action == Action::Restart {
            return false;
        }
        self.actions[row(button)][column(press)] = action;
        true
    }

    /// Whether short presses of `button` have to be watched for a second
    /// one: only if a double click does something.
    pub fn waits_for_double(&self, button: Button) -> bool {
        self.action(button, Press::DoubleClick) != Action::Nothing
    }
}

/// Held this long, a press of the upper button is a long press.
pub const LONG_MS: u64 = 1000;
/// And this long, a very long one.
pub const VERY_LONG_MS: u64 = 10_000;

/// Tells the upper button's presses apart from how long it is held. A
/// long and a very long press are reported while the button is still
/// down, when their time is reached; a click when the button comes up
/// before that. There is no double click: waiting for a second press
/// would make every click late.
pub struct Hold {
    down_since: Option<u64>,
    long_reported: bool,
    very_long_reported: bool,
}

impl Hold {
    pub const fn new() -> Self {
        Self { down_since: None, long_reported: false, very_long_reported: false }
    }

    /// Take the button's state (`down`) at `now_ms`. Call often.
    pub fn feed(&mut self, down: bool, now_ms: u64) -> Option<Press> {
        match (self.down_since, down) {
            (None, true) => {
                self.down_since = Some(now_ms);
                self.long_reported = false;
                self.very_long_reported = false;
                None
            }
            (Some(since), true) => {
                let held = now_ms.saturating_sub(since);
                if held >= VERY_LONG_MS && !self.very_long_reported {
                    self.very_long_reported = true;
                    self.long_reported = true;
                    Some(Press::VeryLong)
                } else if held >= LONG_MS && !self.long_reported {
                    self.long_reported = true;
                    Some(Press::Long)
                } else {
                    None
                }
            }
            (Some(_), false) => {
                self.down_since = None;
                (!self.long_reported).then_some(Press::Click)
            }
            (None, false) => None,
        }
    }
}

/// A second short press within this long of the first makes a double
/// click. The Apollo's own number for the lower button.
pub const DOUBLE_MS: u64 = 300;

/// Tells clicks and double clicks apart from short presses as they
/// end. With `wait` false every short press is a click at once; with it
/// true a click is held back for `DOUBLE_MS` to see whether a second
/// press follows.
pub struct Taps {
    first_at: Option<u64>,
}

impl Taps {
    pub const fn new() -> Self {
        Self { first_at: None }
    }

    /// A short press ended at `now_ms`.
    pub fn pressed(&mut self, now_ms: u64, wait: bool) -> Option<Press> {
        if !wait {
            self.first_at = None;
            return Some(Press::Click);
        }
        match self.first_at.take() {
            Some(first) if now_ms.saturating_sub(first) <= DOUBLE_MS => Some(Press::DoubleClick),
            // A first press, or one too late to belong to the one before;
            // that one's click was given out by `tick` already.
            _ => {
                self.first_at = Some(now_ms);
                None
            }
        }
    }

    /// Call often: gives out the click of a press no second one followed.
    /// A press is waiting to be told apart from a double click (`tick`).
    pub fn waiting(&self) -> bool {
        self.first_at.is_some()
    }

    pub fn tick(&mut self, now_ms: u64) -> Option<Press> {
        match self.first_at {
            Some(first) if now_ms.saturating_sub(first) > DOUBLE_MS => {
                self.first_at = None;
                Some(Press::Click)
            }
            _ => None,
        }
    }
}
