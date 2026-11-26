//! Draw the watch face with the firmware's own drawing code into a picture
//! file (PPM), to look at a design without the watch.
//!
//!   cargo run --bin preview -- OUT.ppm [MILLISECONDS] [boxes]
//!   cargo run --bin preview -- OUT.ppm above|above-set|below|below-set|sounds|compass|compass-waiting|menu|menu-moved|launcher|launcher-chosen|picker|picker-turning|picker-seconds|faces|cards|power|power-chosen|status
//!
//! MILLISECONDS moves the animation; a third argument outlines every
//! element's rectangle. Text is blended in software here, on black; on the
//! watch the blitter does it.

use w301_host::app::face;
use w301_host::app::pages::Page;
use w301_host::app::panels;
use w301_host::services;

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

const N: usize = 454;
struct Frame(Vec<u16>);
impl OriginDimensions for Frame {
    fn size(&self) -> Size { Size::new(N as u32, N as u32) }
}
impl DrawTarget for Frame {
    type Color = Rgb565;
    type Error = core::convert::Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if (0..N as i32).contains(&p.x) && (0..N as i32).contains(&p.y) {
                self.0[p.y as usize * N + p.x as usize] = RawU16::from(c).into_inner();
            }
        }
        Ok(())
    }
}

impl services::font::Paint for Frame {
    /// Blended over what is in the picture, as the watch's blitter does.
    fn coverage(&mut self, clip: &Rectangle, x: i32, y: i32, width: u16, height: u16, coverage: &[u8], color: Rgb565) {
        let area = Rectangle::new(Point::new(x, y), Size::new(width as u32, height as u32)).intersection(clip).intersection(&Rectangle::new(Point::zero(), Size::new(N as u32, N as u32)));
        for row in area.rows() {
            for column in area.columns() {
                let c = coverage[(row - y) as usize * width as usize + (column - x) as usize] as u32;
                let at = row as usize * N + column as usize;
                let old = Rgb565::from(RawU16::new(self.0[at]));
                let mix = |new: u8, old: u8| ((new as u32 * c + old as u32 * (255 - c) + 127) / 255) as u8;
                let shade = Rgb565::new(mix(color.r(), old.r()), mix(color.g(), old.g()), mix(color.b(), old.b()));
                self.0[at] = RawU16::from(shade).into_inner();
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut frame = Frame(vec![0; N * N]);
    let ms: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(37_400);
    let state = face::State {
        time: Some(face::Time { year: 2026, month: 11, day: 24, weekday: 2, hour: 23, minute: 42, second: (ms / 1000 % 60) as u8 }),
        millis: (ms % 1000) as u16,
        ticks: ms,
        battery: Some((62, false)),
        heart: Some(62),
        measuring: true,
        bluetooth: true,
        worn: Some(true),
        temperature: Some(268),
        steps: Some(6_412),
        step_goal: 10_000,
    };
    let whole = face::whole();
    match args.get(2).map(String::as_str) {
        Some("above") => w301_host::app::settings::draw(&mut frame, &whole, &w301_host::app::settings::Settings::START),
        Some("above-set") => {
            let set = w301_host::app::settings::Settings { brightness: Some(2), display_off_s: 30, raise_to_wake: false, quiet: true, move_reminder: true, ..w301_host::app::settings::Settings::START };
            w301_host::app::settings::draw(&mut frame, &whole, &set)
        }
        Some("sounds") => w301_host::app::sounds::draw(&mut frame, &whole, Some(2)),
        Some("compass") => {
            let view = w301_host::app::compass::View { degrees: 291, calibrated: true };
            w301_host::app::compass::draw(&mut frame, &whole, Some(view))
        }
        Some("compass-waiting") => w301_host::app::compass::draw(&mut frame, &whole, None),
        Some("menu") => w301_host::app::menu::draw(&mut frame, &whole, &w301_host::app::settings::Settings::START, 0),
        Some("menu-moved") => {
            let set = w301_host::app::settings::Settings { brightness: Some(2), quiet: true, ..w301_host::app::settings::Settings::START };
            w301_host::app::menu::draw(&mut frame, &whole, &set, w301_host::app::menu::longest())
        }
        Some("launcher") => w301_host::app::launcher::draw(&mut frame, &whole, &w301_host::app::launcher::Launcher::new()),
        Some("launcher-chosen") => {
            let mut launcher = w301_host::app::launcher::Launcher::new();
            let tile = launcher.tile_rect(2).unwrap().center();
            launcher.tap(tile.x, tile.y);
            w301_host::app::launcher::draw(&mut frame, &whole, &launcher)
        }
        Some("picker") => w301_host::app::picker::draw(&mut frame, &whole, &w301_host::app::picker::Picker::new("QUIET FROM", 22 * 60)),
        Some("picker-seconds") => w301_host::app::picker::draw(&mut frame, &whole, &w301_host::app::picker::Picker::with_seconds("TIMER", 25 * 60 + 30)),
        Some("picker-turning") => {
            // The minutes wheel under the finger, 70 pixels down.
            let mut picker = w301_host::app::picker::Picker::new("QUIET UNTIL", 7 * 60);
            let at = picker.wheel_rect(1).center();
            picker.touch(Some((at.x, at.y)), 0);
            picker.touch(Some((at.x, at.y + 70)), 100);
            w301_host::app::picker::draw(&mut frame, &whole, &picker)
        }
        Some("below") => w301_host::app::alerts::draw(&mut frame, &whole, &w301_host::app::alerts::Alerts::new()),
        Some("below-set") => {
            use w301_host::app::alerts::{Alert, Alerts, Kind};
            let mut alerts = Alerts::new();
            alerts.add(Alert { kind: Kind::Move, at: Some((11, 2)) });
            alerts.add(Alert { kind: Kind::Move, at: Some((15, 40)) });
            alerts.add(Alert { kind: Kind::LowBattery, at: Some((17, 5)) });
            w301_host::app::alerts::draw(&mut frame, &whole, &alerts)
        }
        Some("faces") => panels::draw(&mut frame, &whole, Page::Faces),
        Some("cards") => panels::draw(&mut frame, &whole, Page::Cards),
        Some("status") => {
            use w301_host::app::status;
            let lines = status::compose(&status::Status {
                time: state.time,
                uptime_s: 4_327,
                battery: Some((62, 3_912.0, false)),
                temperatures: Some([26.8, 27.1, 30.2]),
                barometer: Some((101_320.0, 118.0)),
                heart_bpm: None,
                lux: Some(143),
                bluetooth_connected: true,
                worn: Some(false),
                measuring_heart: false,
                brightness_step: None,
                brightness: 128,
                last_button: "lower: short press",
                last_touch: None,
            });
            status::background(&mut frame, &whole);
            for (row, line) in lines.iter().enumerate() {
                status::draw_line(&mut frame, &whole, row, line);
            }
        }
        Some("power") => w301_host::app::power::draw(&mut frame, &whole, None, true),
        Some("power-chosen") => w301_host::app::power::draw(&mut frame, &whole, Some(w301_host::app::power::Choice::Stock), false),
        _ => {
            for n in 0..face::ELEMENTS {
                face::draw(&mut frame, &whole, n, &state);
            }
        }
    }
    if args.get(3).is_some() && args[2].parse::<u32>().is_ok() {
        for n in 0..face::ELEMENTS {
            let r: Rectangle = face::rect(n, &state);
            let _ = r.into_styled(embedded_graphics::primitives::PrimitiveStyle::with_stroke(Rgb565::new(12, 12, 12), 1)).draw(&mut frame);
        }
    }
    let mut out = format!("P6\n{} {}\n255\n", N, N).into_bytes();
    for y in 0..N {
        for x in 0..N {
            let v = frame.0[y * N + x];
            let (r, g, b) = ((v >> 11) as u8 * 8, ((v >> 5) & 63) as u8 * 4, (v & 31) as u8 * 8);
            let (dx, dy) = (x as i32 - 227, y as i32 - 227);
            if dx * dx + dy * dy > 227 * 227 { out.extend([40, 40, 40]); } else { out.extend([r, g, b]); }
        }
    }
    std::fs::write(&args[1], out).unwrap();
}
