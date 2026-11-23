//! Draw the watch face with the firmware's own drawing code into a picture
//! file (PPM), to look at a design without the watch.
//!
//!   cargo run --bin preview -- OUT.ppm [MILLISECONDS] [boxes]
//!   cargo run --bin preview -- OUT.ppm above|below|faces|cards
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

impl services::font::Paint for Frame {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut frame = Frame(vec![0; N * N]);
    let ms: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(37_400);
    let state = face::State {
        time: Some(face::Time { year: 2026, month: 11, day: 22, weekday: 7, hour: 20, minute: 9, second: (ms / 1000 % 60) as u8 }),
        millis: (ms % 1000) as u16,
        ticks: ms,
        battery: Some((84, false)),
        heart: Some(72),
        measuring: true,
        bluetooth: true,
        worn: Some(true),
        temperature: Some(268),
    };
    let whole = face::whole();
    match args.get(2).map(String::as_str) {
        Some("above") => panels::draw(&mut frame, &whole, Page::Above),
        Some("below") => panels::draw(&mut frame, &whole, Page::Below),
        Some("faces") => panels::draw(&mut frame, &whole, Page::Faces),
        Some("cards") => panels::draw(&mut frame, &whole, Page::Cards),
        _ => {
            for n in 0..face::ELEMENTS {
                face::draw(&mut frame, &whole, n, &state);
            }
        }
    }
    if args.get(3).is_some() {
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
