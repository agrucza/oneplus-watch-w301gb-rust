//! What the user can set on the watch, the file it is kept in, and the
//! page it is set on: the panel pulled down over the watch face, the
//! design's "quick access".
//!
//! Each of them is something the firmware acts on: how bright the
//! display is (by the light, or one of four fixed steps), after how long
//! without a finger or a button it goes dark, whether raising the wrist
//! lights it, quiet hours (the Apollo's do-not-disturb: no display on a
//! raised wrist, and no vibration for a reminder), and the reminder to
//! move after sitting for long (the Apollo's sedentary reminder).
//!
//! The file is text, a line a setting ("display_off=15"), so that it can
//! be read and written by hand. A line that is not understood is left
//! out; whatever is missing keeps its start value. The two daily times
//! ("quiet_time=22:00-07:00") are set on a page of their own
//! (`super::picker`), opened by a tap on the time.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::theme::{self, BLACK, BONE, CHROME, CYAN, SIZE, STEEL, YELLOW};
use crate::services::font::Paint;
use crate::services::font_data::{NW_LABEL, NW_MONO};

/// The most the file's text takes.
pub const FILE_SIZE: usize = 256;
/// The fixed brightness steps there are.
pub const LEVELS: u8 = 4;
/// The times to choose from after which the display goes dark, seconds.
pub const TIMEOUTS: [u16; 4] = [10, 15, 30, 60];
/// The daily step goals to choose from.
pub const STEP_GOALS: [u32; 5] = [6_000, 8_000, 10_000, 12_000, 15_000];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// `None`: the brightness follows the light. `Some(n)`: fixed step
    /// `n`, counted from 0.
    pub brightness: Option<u8>,
    /// Dark after this many seconds without a finger or a button.
    pub display_off_s: u16,
    /// Raising the wrist lights the display.
    pub raise_to_wake: bool,
    /// Quiet hours are kept, and from when to when (`Window`).
    pub quiet: bool,
    pub quiet_time: Window,
    /// The reminder to move is on, and from when to when.
    pub move_reminder: bool,
    pub move_time: Window,
    /// The steps a day at which the watch face's arc is full.
    pub step_goal: u32,
    /// Whether the Apollo measures the heart rate by itself.
    pub heart_monitor: HeartMonitor,
    /// How loud sounds are, 0 (quiet) up to `VOLUMES - 1`.
    pub volume: u8,
    /// Whether things that happen (the charger put on, the battery low,
    /// the reminder to move) are told by a sound.
    pub event_sounds: bool,
}

/// How many steps of loudness there are.
pub const VOLUMES: u8 = 4;

/// The Apollo's own heart-rate monitoring, while the watch is worn. Its
/// periodic kind cannot be given another rhythm: 30 seconds in every
/// ten and a half minutes are fixed in the Apollo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeartMonitor {
    Off,
    Periodic,
    Always,
}

impl HeartMonitor {
    pub const ALL: [HeartMonitor; 3] = [HeartMonitor::Off, HeartMonitor::Periodic, HeartMonitor::Always];

    /// The mode for the Apollo (`messages::heart_monitor`).
    pub fn mode(self) -> u8 {
        match self {
            HeartMonitor::Off => 0,
            HeartMonitor::Periodic => 1,
            HeartMonitor::Always => 2,
        }
    }

    /// The word in the settings file.
    fn word(self) -> &'static str {
        match self {
            HeartMonitor::Off => "off",
            HeartMonitor::Periodic => "periodic",
            HeartMonitor::Always => "always",
        }
    }
}

/// A stretch of the day, from and to in minutes since midnight. The
/// Apollo's rule for its windows (`lcAlgoCheckDNDMode`, 0x371C0 in its
/// image), which this firmware keeps too: with from before to, both
/// minutes belong to it; otherwise it runs over midnight, and from equal
/// to to is the whole day.
pub type Window = (u16, u16);

/// Whether minute `now` of the day lies in `window`.
pub fn within(window: Window, now: u16) -> bool {
    let (from, to) = window;
    if from < to { from <= now && now <= to } else { now >= from || now < to }
}

/// What a tap on the page changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tap {
    Brightness(Option<u8>),
    DisplayOff(u16),
    RaiseToWake,
    Quiet,
    MoveReminder,
    /// The time beside a switch was tapped: it is to be set (on the
    /// page `super::picker`). Changes nothing by itself.
    PickQuietTime,
    PickMoveTime,
    /// A time that was set there.
    QuietTime(Window),
    MoveTime(Window),
    /// The next of the choices there are, round and round: for a row of
    /// the settings list, which has no room for a button each.
    NextBrightness,
    NextDisplayOff,
    NextStepGoal,
    NextHeartMonitor,
    NextVolume,
    EventSounds,
}

/// The entry after `now` in `choices`, the first after the last; the
/// first if `now` is none of them.
fn next<T: Copy + PartialEq>(choices: &[T], now: T) -> T {
    let at = choices.iter().position(|&choice| choice == now).map_or(0, |n| (n + 1) % choices.len());
    choices[at]
}

/// "22:00-07:00" as a window.
fn parse_window(text: &str) -> Option<Window> {
    let minutes = |text: &str| {
        let (hours, minutes) = text.trim().split_once(':')?;
        let (hours, minutes) = (hours.parse::<u16>().ok()?, minutes.parse::<u16>().ok()?);
        (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
    };
    let (from, to) = text.split_once('-')?;
    Some((minutes(from)?, minutes(to)?))
}

impl Settings {
    /// The settings of a watch that has no file yet.
    /// The two daily times are the stock firmware's start values.
    pub const START: Settings = Settings {
        brightness: None,
        display_off_s: 15,
        raise_to_wake: true,
        quiet: false,
        quiet_time: (22 * 60, 7 * 60),
        move_reminder: false,
        move_time: (9 * 60, 21 * 60),
        step_goal: 10_000,
        heart_monitor: HeartMonitor::Off,
        volume: 0,
        event_sounds: false,
    };

    /// Whether the quiet hours are on and minute `now` of the day lies
    /// in them.
    pub fn quiet_at(&self, now: u16) -> bool {
        self.quiet && within(self.quiet_time, now)
    }

    /// The settings a file's text holds.
    pub fn parse(text: &[u8]) -> Settings {
        let mut settings = Settings::START;
        for line in text.split(|&b| b == b'\n') {
            let Ok(line) = core::str::from_utf8(line) else {
                continue;
            };
            let Some((name, value)) = line.trim().split_once('=') else {
                continue;
            };
            match (name.trim(), value.trim()) {
                ("brightness", "auto") => settings.brightness = None,
                ("brightness", step) => {
                    if let Some(step) = step.parse::<u8>().ok().filter(|step| (1..=LEVELS).contains(step)) {
                        settings.brightness = Some(step - 1);
                    }
                }
                ("display_off", seconds) => {
                    if let Some(seconds) = seconds.parse().ok().filter(|seconds| TIMEOUTS.contains(seconds)) {
                        settings.display_off_s = seconds;
                    }
                }
                ("raise_to_wake", "on") => settings.raise_to_wake = true,
                ("raise_to_wake", "off") => settings.raise_to_wake = false,
                ("event_sounds", "on") => settings.event_sounds = true,
                ("event_sounds", "off") => settings.event_sounds = false,
                ("quiet", "on") => settings.quiet = true,
                ("quiet", "off") => settings.quiet = false,
                ("move_reminder", "on") => settings.move_reminder = true,
                ("move_reminder", "off") => settings.move_reminder = false,
                ("step_goal", steps) => {
                    if let Some(steps) = steps.parse().ok().filter(|steps| STEP_GOALS.contains(steps)) {
                        settings.step_goal = steps;
                    }
                }
                ("heart_monitor", word) => {
                    if let Some(&monitor) = HeartMonitor::ALL.iter().find(|monitor| monitor.word() == word) {
                        settings.heart_monitor = monitor;
                    }
                }
                ("volume", step) => {
                    if let Some(step) = step.parse::<u8>().ok().filter(|step| (1..=VOLUMES).contains(step)) {
                        settings.volume = step - 1;
                    }
                }
                ("quiet_time", window) => settings.quiet_time = parse_window(window).unwrap_or(settings.quiet_time),
                ("move_time", window) => settings.move_time = parse_window(window).unwrap_or(settings.move_time),
                _ => {}
            }
        }
        settings
    }

    /// The file's text for these settings, written into `out`. Returns
    /// its length.
    pub fn text(&self, out: &mut [u8; FILE_SIZE]) -> usize {
        use core::fmt::Write;
        struct Into<'a>(&'a mut [u8; FILE_SIZE], usize);
        impl Write for Into<'_> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let n = s.len().min(self.0.len() - self.1);
                self.0[self.1..self.1 + n].copy_from_slice(&s.as_bytes()[..n]);
                self.1 += n;
                Ok(())
            }
        }
        let mut into = Into(out, 0);
        match self.brightness {
            None => {
                let _ = writeln!(into, "brightness=auto");
            }
            Some(step) => {
                let _ = writeln!(into, "brightness={}", step + 1);
            }
        }
        let _ = writeln!(into, "display_off={}", self.display_off_s);
        let on = |on: bool| if on { "on" } else { "off" };
        let _ = writeln!(into, "raise_to_wake={}", on(self.raise_to_wake));
        let time = |(from, to): Window| (from / 60, from % 60, to / 60, to % 60);
        let (a, b, c, d) = time(self.quiet_time);
        let _ = writeln!(into, "quiet={}", on(self.quiet));
        let _ = writeln!(into, "quiet_time={:02}:{:02}-{:02}:{:02}", a, b, c, d);
        let (a, b, c, d) = time(self.move_time);
        let _ = writeln!(into, "move_reminder={}", on(self.move_reminder));
        let _ = writeln!(into, "move_time={:02}:{:02}-{:02}:{:02}", a, b, c, d);
        let _ = writeln!(into, "step_goal={}", self.step_goal);
        let _ = writeln!(into, "heart_monitor={}", self.heart_monitor.word());
        let _ = writeln!(into, "volume={}", self.volume + 1);
        let _ = writeln!(into, "event_sounds={}", on(self.event_sounds));
        into.1
    }

    /// The settings after `tap`.
    pub fn after(self, tap: Tap) -> Settings {
        match tap {
            Tap::Brightness(brightness) => Settings { brightness, ..self },
            Tap::DisplayOff(display_off_s) => Settings { display_off_s, ..self },
            Tap::RaiseToWake => Settings { raise_to_wake: !self.raise_to_wake, ..self },
            Tap::Quiet => Settings { quiet: !self.quiet, ..self },
            Tap::MoveReminder => Settings { move_reminder: !self.move_reminder, ..self },
            Tap::PickQuietTime | Tap::PickMoveTime => self,
            Tap::QuietTime(quiet_time) => Settings { quiet_time, ..self },
            Tap::MoveTime(move_time) => Settings { move_time, ..self },
            Tap::NextBrightness => {
                let brightness = match self.brightness {
                    None => Some(0),
                    Some(step) if step + 1 < LEVELS => Some(step + 1),
                    Some(_) => None,
                };
                Settings { brightness, ..self }
            }
            Tap::NextDisplayOff => Settings { display_off_s: next(&TIMEOUTS, self.display_off_s), ..self },
            Tap::NextStepGoal => Settings { step_goal: next(&STEP_GOALS, self.step_goal), ..self },
            Tap::NextHeartMonitor => Settings { heart_monitor: next(&HeartMonitor::ALL, self.heart_monitor), ..self },
            Tap::NextVolume => Settings { volume: (self.volume + 1) % VOLUMES, ..self },
            Tap::EventSounds => Settings { event_sounds: !self.event_sounds, ..self },
        }
    }
}

// --- The page -------------------------------------------------------------------

/// The left edge and the width of everything on the page.
const LEFT: i32 = 62;
const WIDTH: i32 = SIZE - 2 * LEFT;
/// Buttons in a row: their height and the gap between them.
const BUTTON: i32 = 40;
const GAP: i32 = 6;
/// The tops of the two labels with their rows of buttons under them, the
/// top of the first row with a switch and the height of such a row.
const BRIGHTNESS_Y: i32 = 76;
const DISPLAY_OFF_Y: i32 = 152;
const SWITCHES_Y: i32 = 232;
const ROW: i32 = 44;
const LABEL_TO_ROW: i32 = 26;
/// The switch: width and height.
const SWITCH: (i32, i32) = (64, 30);
/// The rows with a switch, from the top.
pub const SWITCHES: [Tap; 3] = [Tap::RaiseToWake, Tap::Quiet, Tap::MoveReminder];

/// The button for automatic brightness (0) or for fixed step `n - 1`.
pub fn brightness_rect(n: usize) -> Rectangle {
    // "AUTO" is wider than the digits.
    let narrow = (WIDTH - 90 - LEVELS as i32 * GAP) / LEVELS as i32;
    let (x, width) = if n == 0 { (LEFT, 90) } else { (LEFT + 90 + GAP + (n as i32 - 1) * (narrow + GAP), narrow) };
    Rectangle::new(Point::new(x, BRIGHTNESS_Y + LABEL_TO_ROW), Size::new(width as u32, BUTTON as u32))
}

/// The button for `TIMEOUTS[n]`.
pub fn display_off_rect(n: usize) -> Rectangle {
    let width = (WIDTH - (TIMEOUTS.len() as i32 - 1) * GAP) / TIMEOUTS.len() as i32;
    Rectangle::new(Point::new(LEFT + n as i32 * (width + GAP), DISPLAY_OFF_Y + LABEL_TO_ROW), Size::new(width as u32, BUTTON as u32))
}

/// Row `n` of those with a switch (`SWITCHES`).
pub fn switch_rect(n: usize) -> Rectangle {
    Rectangle::new(Point::new(LEFT, SWITCHES_Y + n as i32 * ROW), Size::new(WIDTH as u32, ROW as u32))
}

/// What a tap at (x, y) changes, if it lands on anything.
pub fn tap_at(x: i32, y: i32) -> Option<Tap> {
    let at = Point::new(x, y);
    // A finger is wider than a button's edge: half the gap counts too.
    let hit = |r: Rectangle| Rectangle::new(r.top_left - Point::new(GAP / 2, GAP / 2), r.size + Size::new(GAP as u32, GAP as u32)).contains(at);
    if let Some(n) = (0..=LEVELS as usize).find(|&n| hit(brightness_rect(n))) {
        return Some(Tap::Brightness(n.checked_sub(1).map(|step| step as u8)));
    }
    if let Some(n) = (0..TIMEOUTS.len()).find(|&n| hit(display_off_rect(n))) {
        return Some(Tap::DisplayOff(TIMEOUTS[n]));
    }
    // Only the switch switches: the rows lie close together, and a tap
    // meant for the row below has switched the one above (seen in the
    // log of 2026-11-24). In a row with a time the rest of the row leads
    // to setting the time; in the others it does nothing.
    let on_switch = x >= LEFT + WIDTH - SWITCH.0 - 12;
    let n = (0..SWITCHES.len()).find(|&n| switch_rect(n).contains(at))?;
    match (SWITCHES[n], on_switch) {
        (tap, true) => Some(tap),
        (Tap::Quiet, false) => Some(Tap::PickQuietTime),
        (Tap::MoveReminder, false) => Some(Tap::PickMoveTime),
        _ => None,
    }
}

/// A button of a row: a plate in yellow with black text if it is the
/// one chosen, a frame otherwise.
fn button<D: Paint>(target: &mut D, clip: &Rectangle, rect: &Rectangle, text: &str, chosen: bool) {
    let ink = if chosen {
        theme::plate(target, clip, rect, 6, YELLOW);
        BLACK
    } else {
        theme::frame(target, clip, rect, 6, STEEL);
        BONE
    };
    // The label font's letters carry their spacing on the right.
    let x = rect.top_left.x + (rect.size.width as i32 - NW_LABEL.measure(text) + 3) / 2;
    target.text(clip, x, rect.top_left.y + (BUTTON - NW_LABEL.height as i32) / 2, &NW_LABEL, text, ink);
}

/// Draw the part of the page that lies in `clip`.
pub fn draw<D: Paint>(target: &mut D, clip: &Rectangle, settings: &Settings) {
    theme::clear(target, clip);
    theme::title(target, clip, "QUICK.ACCESS", CYAN);

    target.text(clip, LEFT, BRIGHTNESS_Y, &NW_LABEL, "BRIGHTNESS", CYAN);
    const STEPS: [&str; 5] = ["AUTO", "1", "2", "3", "4"];
    for (n, text) in STEPS.iter().enumerate().take(LEVELS as usize + 1) {
        let chosen = settings.brightness.map_or(0, |step| step as usize + 1) == n;
        button(target, clip, &brightness_rect(n), text, chosen);
    }

    target.text(clip, LEFT, DISPLAY_OFF_Y, &NW_LABEL, "DISPLAY OFF AFTER", CYAN);
    const TIMES: [&str; 4] = ["10 S", "15 S", "30 S", "60 S"];
    for (n, text) in TIMES.iter().enumerate() {
        button(target, clip, &display_off_rect(n), text, settings.display_off_s == TIMEOUTS[n]);
    }

    theme::rule(target, clip, LEFT, SWITCHES_Y, WIDTH, STEEL);
    for (n, tap) in SWITCHES.iter().enumerate() {
        let row = switch_rect(n);
        let y = row.top_left.y;
        let (label, on, time) = match tap {
            Tap::Quiet => ("QUIET", settings.quiet, Some(settings.quiet_time)),
            Tap::MoveReminder => ("MOVE ALERT", settings.move_reminder, Some(settings.move_time)),
            _ => ("RAISE TO WAKE", settings.raise_to_wake, None),
        };
        target.text(clip, LEFT, y + 10, &NW_LABEL, label, BONE);
        let switch_x = LEFT + WIDTH - SWITCH.0;
        if let Some((from, to)) = time {
            // From when to when, e.g. "22:00-07:00".
            let mut text = [0u8; 11];
            for (i, value) in [from / 60, from % 60, to / 60, to % 60].into_iter().enumerate() {
                text[i * 3] = b'0' + (value / 10 % 10) as u8;
                text[i * 3 + 1] = b'0' + (value % 10) as u8;
            }
            (text[2], text[5], text[8]) = (b':', b'-', b':');
            let text = core::str::from_utf8(&text).unwrap_or("");
            target.text(clip, switch_x - 12 - NW_MONO.measure(text), y + 10, &NW_MONO, text, CHROME);
        }
        theme::switch(target, clip, switch_x, y + (ROW - SWITCH.1) / 2, SWITCH, on);
        theme::rule(target, clip, LEFT, y + ROW, WIDTH, STEEL);
    }

    // The bar the panel is held by, and a line along the edge it ends
    // at, which shows while it moves over the face.
    theme::handle(target, clip, SIZE - 34, CYAN);
    let edge = Rectangle::new(Point::new(0, SIZE - 2), Size::new(SIZE as u32, 2));
    let _ = target.fill_solid(&edge.intersection(clip), CYAN);
}
