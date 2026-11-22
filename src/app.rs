//! The application: bring the hardware up in a safe order, then run.

mod status;

use core::fmt::Write;

use embassy_time::{Duration, Instant};

use crate::board::Board;
use crate::boot::{faults, ticket, watchdog};
use crate::drivers::apollo::messages::{KeyEvent, Readings, WristGesture};
use crate::drivers::buttons::ButtonEvent;
use crate::drivers::cypress::Link;
use crate::drivers::touch::TouchEvent;
use crate::drivers::{apollo, buttons, light, panel, psram, touch};
use crate::hal::{clocks, sdmmc};
use crate::services::log::Log;
use crate::services::update::server::{Activity, Server};
use crate::services::brightness::AutoBrightness;
use crate::services::{bluetooth, clock, factory, storage};

/// Volume the update package and the installed image file live on.
const IMAGE_VOLUME: usize = 1;
/// Volume with the log folder.
const LOG_VOLUME: usize = 3;

/// Returns only if the watch cannot be reached or served; the caller then
/// resets, which takes the watch back to the stock firmware.
pub async fn run(board: Board) {
    board.power.switch_on().await;
    let _ = ticket::report(ticket::STAGE_POWER, 0);

    // --- Storage and log ----------------------------------------------------
    watchdog::feed();
    if let Err(e) = sdmmc::init() {
        let _ = ticket::report(ticket::STAGE_INIT_FAILED, e as u8);
        return;
    }
    let Ok(log_partition) = storage::inspect(LOG_VOLUME) else {
        let _ = ticket::report(ticket::STAGE_LOG, 1);
        return;
    };
    let mut log = Log::start(log_partition);
    let _ = writeln!(log.stamp(), "--- w301 rust app: ota server ---");
    faults::write_reset_cause(log.stamp());
    clocks::log_state(&mut log);
    clock::start(&mut log);
    if let Err(step) = log.flush() {
        let _ = ticket::report(ticket::STAGE_LOG, step);
        return;
    }
    let _ = ticket::report(ticket::STAGE_LOG, 0); // log works; the rest follows

    let Ok(image_partition) = storage::inspect(IMAGE_VOLUME) else {
        let _ = writeln!(log.stamp(), "volume 1 not readable");
        let _ = log.flush();
        return;
    };

    // --- Bluetooth: become the watch the PC is paired with -----------------
    // If Bluetooth cannot be started there is no way to reach us, so fall
    // through and reset (back to the stock firmware).
    if let Some(link) = bluetooth::start(board.bluetooth, &mut log).await {
        let _ = log.flush();
        // Bluetooth first, then the Apollo: once released it may reset us
        // again, and by then the PC must be able to reach the watch.
        // The board revision, measured once as in the stock app; the Apollo
        // asks for it.
        // The board revision, measured as the stock app does; the Apollo
        // asks for it after its start.
        let _ = log.flush();
        let (revision, reading) = board.revision.measure();
        let _ = writeln!(log.stamp(), "board revision {} (reading {} of 4095, {} mV)", revision, reading, reading as u32 * 1800 / 4096);
        if board.codec_supply.off(revision) {
            let _ = writeln!(log.stamp(), "audio codec supply (PB1) held off");
        }
        let _ = log.flush();
        let mut apollo = apollo::Apollo::start(board.apollo).await;
        if let Some(apollo) = apollo.as_mut() {
            apollo.set_revision(revision);
        }
        let _ = writeln!(log.stamp(), "apollo: {}", if apollo.is_some() { "released from reset" } else { "not started" });

        // External memory, then the display with its framebuffer in it.
        // Last, so that a problem here cannot keep Bluetooth from coming up.
        // Should the memory bus hang, the watchdog resets us - and since no
        // PC has connected to this boot yet, that reset returns to the stock
        // firmware instead of looping.
        let _ = log.flush();
        let mut screen = None;
        psram::init().await;
        if psram::quick_check(&mut log) {
            let _ = writeln!(log.stamp(), "psram: ready");
            // The 2 MB at FRAMEBUFFER belong to the display alone.
            let frame = unsafe { &mut *(psram::FRAMEBUFFER as *mut panel::Frame) };
            screen = panel::Panel::start(board.display, frame, &mut log).await.map(Screen::new);
        } else {
            let _ = writeln!(log.stamp(), "display: not started, no memory for the picture");
        }
        let _ = log.flush();
        // Sensor bus and light sensor. Nothing depends on them.
        let mut bus = board.sensor_bus.open();
        light::scan(&mut bus, &mut log);
        let factor = factory::light_calibration(&mut log).unwrap_or_else(|| {
            let _ = writeln!(log.stamp(), "light sensor: using the stock default factor {}", light::DEFAULT_FACTOR);
            light::DEFAULT_FACTOR
        });
        let light = light::LightSensor::start(&mut bus, factor, &mut log).await;
        let _ = log.flush();
        let sensors = Sensors {
            bus,
            light,
            last_raw: None,
            last_lux: None,
            ready_at: Instant::now() + LIGHT_FIRST_READING,
            read_at: None,
            logged_for: None,
            logged: 0,
            brightest: 0,
        };
        let touch = touch::Touch::start(board.touch, &mut log).await;
        let _ = log.flush();
        let buttons = buttons::Buttons::start(board.upper_button);
        if let Some(server) = Server::new(image_partition) {
            main_loop(link, server, apollo, buttons, screen, sensors, touch).await;
        }
    }

    match log.flush() {
        Ok(()) => {
            let _ = ticket::report(ticket::STAGE_LOG, 15);
        }
        Err(step) => {
            let _ = ticket::report(ticket::STAGE_LOG, step);
        }
    }
}

/// Fixed brightness steps (panel values). The lower button goes from
/// automatic through these and back to automatic.
const BRIGHTNESS: [u8; 4] = [0x20, 0x60, 0x85, 0xFF];
/// What the stock app's panel set-up leaves the brightness at.
const START_BRIGHTNESS: u8 = 0x85;
/// A press of the upper button shorter than this is a click.
const CLICK_MS: u64 = 800;
/// Automatic brightness moves the panel by at most `FADE_STEP` every
/// `FADE_INTERVAL`: from darkest to brightest in about two seconds.
const FADE_STEP: u8 = 4;
const FADE_INTERVAL: Duration = Duration::from_millis(30);

/// How long a "not on the wrist" report has to stand, with the display on,
/// before the status screen shows it.
const NOT_WORN_HOLD: Duration = Duration::from_secs(3);

/// The wear state as the status screen shows it.
///
/// The Apollo's report is taken as it is everywhere else. On the screen a
/// "not on the wrist" is held back for a moment, because of how the
/// Apollo's check behaves (measured): while the arm hangs down, its
/// infrared check loses the skin and it reports "not on the wrist"; about
/// 1.5 s after the arm is raised it finds the skin again. Without the hold
/// the display, switched on by that same raise, would always come up
/// reading "not worn".
struct WearView {
    /// The Apollo's latest report and when the display side first saw it.
    reported: Option<bool>,
    since: Instant,
    shown: Option<bool>,
}

impl WearView {
    fn new() -> Self {
        Self { reported: None, since: Instant::now(), shown: None }
    }

    /// The state to show, given the Apollo's report and since when the
    /// display has been on.
    fn update(&mut self, reported: Option<bool>, display_on_since: Instant) -> Option<bool> {
        if reported != self.reported {
            self.reported = reported;
            self.since = Instant::now();
        }
        match reported {
            Some(false) => {
                // Counted from the report or from the display coming on,
                // whichever is later.
                let standing = self.since.max(display_on_since).elapsed();
                if self.shown.is_none() || standing >= NOT_WORN_HOLD {
                    self.shown = Some(false);
                }
            }
            other => self.shown = other,
        }
        self.shown
    }
}

/// The panel and what is currently shown on it.
struct Screen {
    panel: panel::Panel,
    visible: bool,
    /// `None`: brightness follows the light. `Some(n)`: fixed at `BRIGHTNESS[n]`.
    fixed: Option<usize>,
    auto: AutoBrightness,
    /// What the panel is set to now.
    level: u8,
    /// The last level written to the log.
    level_logged: u8,
    faded_at: Instant,
    /// The display was shown by a wrist raise, not by the button.
    shown_by_raise: bool,
    /// Since when the display has been on.
    visible_since: Instant,
    wear: WearView,
    last_button: &'static str,
    /// Where the glass was last touched, if that is newer than the button.
    last_touch: Option<(u16, u16)>,
    /// The frame has changed since it was last sent to the panel.
    changed: bool,
    /// Uptime second the screen was last drawn for.
    drawn_for: Option<u64>,
    refresh_logged: bool,
}

impl Screen {
    fn new(panel: panel::Panel) -> Self {
        status::clear(panel.frame);
        Self {
            panel,
            visible: true,
            fixed: None,
            auto: AutoBrightness::new(START_BRIGHTNESS),
            level: START_BRIGHTNESS,
            level_logged: START_BRIGHTNESS,
            faded_at: Instant::now(),
            shown_by_raise: false,
            visible_since: Instant::now(),
            wear: WearView::new(),
            last_button: "no button yet",
            last_touch: None,
            changed: false,
            drawn_for: None,
            refresh_logged: false,
        }
    }

    /// React to a click: the upper button blanks and shows the picture,
    /// the lower one steps the brightness (automatic, then the fixed steps).
    async fn click(&mut self, upper: bool, log: &mut Log) {
        let taken = if upper {
            self.visible = !self.visible;
            self.visible_since = Instant::now();
            self.shown_by_raise = false;
            self.last_button = "upper: click";
            let _ = writeln!(log.stamp(), "display: {}", if self.visible { "shown" } else { "blanked" });
            self.panel.set_visible(self.visible).await
        } else {
            self.last_button = "lower: click";
            self.fixed = match self.fixed {
                None => Some(0),
                Some(n) if n + 1 < BRIGHTNESS.len() => Some(n + 1),
                Some(_) => None,
            };
            match self.fixed {
                Some(n) => {
                    let _ = writeln!(log.stamp(), "display: brightness fixed, step {} of {}", n + 1, BRIGHTNESS.len());
                    self.level = BRIGHTNESS[n];
                    self.level_logged = self.level;
                    self.panel.set_brightness(self.level).await
                }
                None => {
                    let _ = writeln!(log.stamp(), "display: brightness automatic");
                    true // `fade` takes it from here
                }
            }
        };
        if !taken {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.last_touch = None;
        self.drawn_for = None; // show the change at once
    }

    /// React to a wrist gesture from the Apollo. Raising the wrist shows a
    /// blanked display, as in the stock app. Lowering it blanks the
    /// display again only if it was a raise that showed it: the stock
    /// app's own condition for that is a setting not read yet, and a
    /// display switched on by hand should not go dark on every movement.
    /// Returns true if the display changed.
    async fn gesture(&mut self, gesture: WristGesture, log: &mut Log) -> bool {
        let show = match gesture {
            WristGesture::Raised if !self.visible => true,
            WristGesture::Lowered if self.visible && self.shown_by_raise => false,
            _ => return false,
        };
        self.visible = show;
        self.visible_since = Instant::now();
        self.shown_by_raise = show;
        let _ = writeln!(log.stamp(), "display: {} by wrist gesture", if show { "shown" } else { "blanked" });
        if !self.panel.set_visible(show).await {
            let _ = writeln!(log.stamp(), "display: the panel did not take the command");
        }
        self.drawn_for = None;
        true
    }

    /// A finger is on the glass: mark the place with a dot.
    fn touched(&mut self, x: u16, y: u16) {
        status::dot(self.panel.frame, x, y);
        self.last_touch = Some((x, y));
        self.changed = true;
    }

    /// A new light reading, in lux.
    fn ambient(&mut self, lux: u32) {
        self.auto.feed(lux);
    }

    /// In automatic mode, move the panel one step towards the brightness
    /// the light asks for. Call on every pass; does nothing most of the time.
    async fn fade(&mut self, log: &mut Log) {
        let wanted = self.auto.wanted();
        if self.fixed.is_some() || !self.visible || self.level == wanted || self.faded_at.elapsed() < FADE_INTERVAL {
            return;
        }
        self.faded_at = Instant::now();
        let step = self.level.abs_diff(wanted).min(FADE_STEP);
        self.level = if wanted > self.level { self.level + step } else { self.level - step };
        if !self.panel.set_brightness(self.level).await {
            let _ = writeln!(log.stamp(), "display: the panel did not take brightness {}", self.level);
        }
    }

    /// Send the picture if it changed; once a second, redraw the status
    /// text first. Takes about 40 ms when it sends.
    async fn update(
        &mut self,
        readings: Option<&Readings>,
        lux: Option<u32>,
        bluetooth_connected: bool,
        measuring_heart: bool,
        log: &mut Log,
    ) {
        let uptime_s = Instant::now().as_secs();
        let new_second = self.drawn_for != Some(uptime_s);
        if !self.visible || !(new_second || self.changed) {
            return;
        }
        self.changed = false;
        if new_second {
            self.redraw(uptime_s, readings, lux, bluetooth_connected, measuring_heart, log);
        }
        let started = Instant::now();
        let r = self.panel.refresh().await;
        // The first refresh and every failed one go to the log.
        if !r.done || !self.refresh_logged {
            self.refresh_logged = true;
            let _ = writeln!(
                log.stamp(),
                "display: refresh {} after {} ms; WISR={:#010x} ISR0={:#010x} ISR1={:#010x} PSR={:#010x}",
                if r.done { "done" } else { "NOT finished" },
                started.elapsed().as_millis(),
                r.wisr,
                r.isr0,
                r.isr1,
                r.psr
            );
        }
    }

    /// Draw the status text for this second into the frame.
    fn redraw(
        &mut self,
        uptime_s: u64,
        readings: Option<&Readings>,
        lux: Option<u32>,
        bluetooth_connected: bool,
        measuring_heart: bool,
        log: &mut Log,
    ) {
        self.drawn_for = Some(uptime_s);
        // Where automatic brightness has arrived, at most once a second.
        if self.fixed.is_none() && self.level == self.auto.wanted() && self.level != self.level_logged {
            let _ = writeln!(
                log.stamp(),
                "display: automatic brightness {} -> {} at {} lux",
                self.level_logged,
                self.level,
                self.auto.lux()
            );
            self.level_logged = self.level;
        }
        let worn = self.wear.update(readings.and_then(|r| r.wear).map(|w| w.on_wrist), self.visible_since);
        status::draw(
            self.panel.frame,
            &status::Status {
                uptime_s,
                readings,
                lux,
                bluetooth_connected,
                worn,
                measuring_heart,
                brightness_step: self.fixed.map(|n| (n + 1, BRIGHTNESS.len())),
                brightness: self.level,
                last_button: self.last_button,
                last_touch: self.last_touch,
            },
        );
    }
}

/// How often the light sensor is read.
const LIGHT_INTERVAL: Duration = Duration::from_millis(250);
/// The sensor reads 0 until its first measurement is finished (measured:
/// 0 after 22 ms, a real value after 270 ms), so wait this long after
/// switching it on.
const LIGHT_FIRST_READING: Duration = Duration::from_millis(400);

/// The sensor bus and what is on it.
struct Sensors {
    bus: light::Bus,
    light: Option<light::LightSensor>,
    last_raw: Option<light::Raw>,
    /// The last reading in lux.
    last_lux: Option<u32>,
    /// From when the light sensor's values can be used.
    ready_at: Instant,
    /// When the light sensor was last read.
    read_at: Option<Instant>,
    /// Uptime second of the last reading written to the log.
    logged_for: Option<u64>,
    logged: u32,
    /// Highest light channel value logged so far.
    brightest: u16,
}

impl Sensors {
    /// Read the light sensor four times a second; returns the brightness in
    /// lux when there is a new reading. The first values and then one
    /// reading every ten seconds go to the log.
    fn poll(&mut self, log: &mut Log) -> Option<u32> {
        let light = self.light.as_mut()?;
        if Instant::now() < self.ready_at || self.read_at.is_some_and(|at| at.elapsed() < LIGHT_INTERVAL) {
            return None;
        }
        self.read_at = Some(Instant::now());
        self.last_raw = light.read(&mut self.bus);
        self.last_lux = self.last_raw.map(|raw| light.lux(raw));
        let now = Instant::now().as_secs();
        if (self.logged < 5 || now % 10 == 0) && self.logged_for != Some(now) {
            self.logged_for = Some(now);
            self.logged += 1;
            match self.last_raw {
                Some(raw) => {
                    let _ = writeln!(
                        log.stamp(),
                        "light sensor: {} lux (raw {} / {})",
                        light.lux(raw),
                        raw.light,
                        raw.second
                    );
                }
                None => {
                    let _ = writeln!(log.stamp(), "light sensor: no answer");
                }
            }
        }
        // Every new maximum (by a quarter or more) goes to the log, to see
        // how the sensor behaves in bright light.
        if let Some(raw) = self.last_raw {
            if raw.light >= 1000 && raw.light as u32 * 4 >= self.brightest as u32 * 5 {
                self.brightest = raw.light;
                let _ = writeln!(
                    log.stamp(),
                    "light sensor: new maximum, {} lux (raw {} / {})",
                    light.lux(raw),
                    raw.light,
                    raw.second
                );
            }
        }
        self.last_lux
    }
}

/// Everything that happens once the hardware is up. Each pass takes at most
/// about 20 ms unless a packet is being dealt with or the screen is sent.
/// Ends only in a reset: an installed update, a return to the stock
/// firmware, or the watchdog.
async fn main_loop(
    mut link: Link,
    mut server: Server,
    mut apollo: Option<apollo::Apollo>,
    mut buttons: buttons::Buttons,
    mut screen: Option<Screen>,
    mut sensors: Sensors,
    mut touch: Option<touch::Touch>,
) -> ! {
    let mut log = Log::get();
    let mut last_save = Instant::now();
    let mut measuring_heart = false;
    loop {
        // The Apollo's handshake cannot wait long, so look after it on every
        // pass - but not while a package is streaming in.
        if !server.transferring() {
            if let Some(apollo) = apollo.as_mut() {
                apollo.service(&mut log).await;
            }
        }

        let lux = if server.transferring() { None } else { sensors.poll(&mut log) };

        // Buttons: the upper one is ours, the lower one comes through the Apollo.
        let upper_click = matches!(buttons.poll(&mut log), Some(ButtonEvent::Released { held_ms }) if held_ms < CLICK_MS);
        let lower = apollo.as_mut().and_then(|a| a.take_key());
        let lower_click = lower == Some(KeyEvent::Clicked);
        // Feedback through the vibration motor: one short pulse for a
        // click, three for a long press.
        if let Some(apollo) = apollo.as_mut() {
            match lower {
                Some(KeyEvent::Clicked) => {
                    apollo.vibrate(200, 0, 1, &mut log).await;
                }
                Some(KeyEvent::LongPressed) => {
                    // A long press starts and stops a heart-rate
                    // measurement: three pulses for start, two for stop.
                    measuring_heart = !measuring_heart;
                    apollo.measure_heart_rate(measuring_heart, &mut log).await;
                    apollo.vibrate(200, 100, if measuring_heart { 3 } else { 2 }, &mut log).await;
                }
                _ => {}
            }
        }
        let touched = match touch.as_mut() {
            Some(touch) if !server.transferring() => touch.poll(&mut log),
            _ => None,
        };
        if let Some(screen) = screen.as_mut() {
            if let Some(TouchEvent::Down { x, y }) = touched {
                screen.touched(x, y);
            }
            if let Some(lux) = lux {
                screen.ambient(lux);
            }
            if upper_click {
                screen.click(true, &mut log).await;
                if let Some(apollo) = apollo.as_mut() {
                    apollo.set_screen(screen.visible);
                }
            }
            if lower_click {
                screen.click(false, &mut log).await;
            }
            if let Some(gesture) = apollo.as_mut().and_then(|a| a.take_gesture()) {
                if screen.gesture(gesture, &mut log).await {
                    if let Some(apollo) = apollo.as_mut() {
                        apollo.set_screen(screen.visible);
                    }
                }
            }
            if !server.transferring() {
                screen.fade(&mut log).await;
                screen.update(apollo.as_ref().map(|a| &a.readings), sensors.last_lux, server.connected, measuring_heart, &mut log).await;
            }
        }

        let activity = server.step(&mut link, &mut log).await;

        // Nothing is arriving: a good moment to save what was logged, so
        // that it survives if the app dies. Not during a transfer, where the
        // sender is waiting for us.
        if activity == Activity::Idle
            && !server.transferring()
            && log.pending()
            && last_save.elapsed() > Duration::from_secs(2)
        {
            let _ = log.flush();
            last_save = Instant::now();
        }
    }
}
