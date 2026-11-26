//! First contact with the three chips on the STM32 that have never
//! answered: the audio codec, the speaker amplifier and the NFC
//! controller. Each is powered the way the stock app does it, asked for
//! the one thing the stock app asks first, and left as it was found.
//! What comes back goes into the log; nothing else is done with it.
//!
//! All of it is read in the stock app (B.70), nothing tried out:
//!
//! - Codec, Cirrus CS47L15: its control bus is SPI2 (PB10 clock, PC1
//!   out, PC2 in, PB12 chip select; mode 0, `BSP_SPI2_Init` 0x0808FCC0).
//!   PB1 high takes it out of sleep on this board revision, PB6 is its
//!   reset (low for 5 ms, then 15 ms of waiting, `cs47Lxx_hard_init`
//!   0x080BDF5E), and PA3 carries a 3 MHz clock for it from timer 2
//!   (`MX_TIM2_Init` 0x08031016). A read is four address bytes with the
//!   top bit set, two bytes of padding, then the value (0x080BD81E).
//!   Register 0 is expected to say 0x6370.
//! - Amplifier at 0x36 on the sensor bus: PG6 high and PG10 low are the
//!   stock app's state from its start (0x0805D562). It may be one of two
//!   parts, which the stock app tells apart at start (0x080A9A28):
//!   register 3 reading 0x1B96 is an NXP TFA, otherwise register 0
//!   reading 0x0310 is an Awinic AW8896. Registers are one byte, values
//!   two, high byte first.
//! - NFC controller, NXP SN100T at 0x28 on the sensor bus: PF2 its
//!   supply, PB3 its enable, PA4 its download request (kept low), PG2
//!   its interrupt. The stock app's factory check (0x08026C4C): PF2
//!   high, 100 ms, PB3 low, 100 ms, PB3 high, 100 ms, then the four
//!   bytes of a "core reset" written; that the chip takes them is the
//!   check. An answer is read as three bytes and then as many as the
//!   third says (0x08060820).
//!
//! Afterwards: PB1 low again and every other pin given up, which is how
//! this firmware has left them so far.

use core::fmt::Write;

use embassy_stm32::gpio::{Input, Level, Output, OutputType, Pull, Speed};
use embassy_stm32::spi::{self, Spi};
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::simple_pwm::{PwmPin, SimplePwm};
use embassy_stm32::{peripherals, Peri};
use embassy_time::Timer;

use crate::boot::watchdog;
use crate::drivers::light::Bus;
use crate::services::log::Log;

pub struct Resources {
    pub amplifier_supply: Peri<'static, peripherals::PG6>,
    pub amplifier_reset: Peri<'static, peripherals::PG10>,
    pub codec_reset: Peri<'static, peripherals::PB6>,
    pub codec_select: Peri<'static, peripherals::PB12>,
    pub codec_spi: Peri<'static, peripherals::SPI2>,
    pub codec_clock: Peri<'static, peripherals::PB10>,
    pub codec_out: Peri<'static, peripherals::PC1>,
    pub codec_in: Peri<'static, peripherals::PC2>,
    pub master_clock_timer: Peri<'static, peripherals::TIM2>,
    pub master_clock: Peri<'static, peripherals::PA3>,
    pub nfc_supply: Peri<'static, peripherals::PF2>,
    pub nfc_enable: Peri<'static, peripherals::PB3>,
    pub nfc_download: Peri<'static, peripherals::PA4>,
    pub nfc_interrupt: Peri<'static, peripherals::PG2>,
}

const AMPLIFIER: u8 = 0x36;
const NFC: u8 = 0x28;

/// Ask each of the three chips once and log what it says. `codec_awake`
/// is PB1, held low by the caller before and after.
pub async fn run(r: Resources, codec_awake: &mut Output<'static>, bus: &mut Bus, log: &mut Log) {
    let _ = writeln!(log.stamp(), "probe: first contact with codec, amplifier and NFC, each as the stock app makes it");

    // --- Codec and amplifier: the stock app's state from its start --------
    let _amplifier_supply = Output::new(r.amplifier_supply, Level::High, Speed::Low);
    let _amplifier_reset = Output::new(r.amplifier_reset, Level::Low, Speed::Low);
    let mut codec_reset = Output::new(r.codec_reset, Level::High, Speed::Low);
    let mut select = Output::new(r.codec_select, Level::High, Speed::VeryHigh);
    codec_awake.set_high();
    let mut config = spi::Config::default();
    config.mode = spi::MODE_0;
    config.frequency = Hertz(7_500_000);
    let mut bus_spi = Spi::new_blocking(r.codec_spi, r.codec_clock, r.codec_out, r.codec_in, config);
    let clock_pin = PwmPin::new(r.master_clock, OutputType::PushPull);
    let mut clock = SimplePwm::new(r.master_clock_timer, None, None, None, Some(clock_pin), Hertz(3_000_000), Default::default());
    {
        let mut channel = clock.ch4();
        channel.set_duty_cycle(channel.max_duty_cycle() / 2);
        channel.enable();
    }
    let _ = writeln!(log.stamp(), "probe: codec clock on PA3 at {} Hz", clock.ch4().get_frequency().0);
    Timer::after_millis(20).await;
    watchdog::feed();

    codec_reset.set_low();
    Timer::after_millis(5).await;
    codec_reset.set_high();
    Timer::after_millis(15).await;
    // Up to three reads 2 ms apart, as the stock app's factory check has it.
    for attempt in 1..=3 {
        let mut frame = [0x80u8, 0, 0, 0, 0, 0, 0, 0];
        select.set_low();
        let done = bus_spi.blocking_transfer_in_place(&mut frame);
        select.set_high();
        let id = u16::from_be_bytes([frame[6], frame[7]]);
        let _ = writeln!(
            log.stamp(),
            "probe: codec register 0, read {}: {:#06x}{}{}",
            attempt,
            id,
            if id == 0x6370 { ", the CS47L15's" } else { ", NOT the 0x6370 expected" },
            if done.is_ok() { "" } else { " (the SPI transfer failed)" }
        );
        if id == 0x6370 {
            break;
        }
        Timer::after_millis(2).await;
    }
    watchdog::feed();

    for (register, expected, part) in [(0x03u8, 0x1B96u16, "an NXP TFA"), (0x00, 0x0310, "an Awinic AW8896")] {
        let mut value = [0u8; 2];
        match bus.blocking_write_read(AMPLIFIER, &[register], &mut value) {
            Ok(()) => {
                let value = u16::from_be_bytes(value);
                let _ = writeln!(
                    log.stamp(),
                    "probe: amplifier register {:#04x}: {:#06x}{}",
                    register,
                    value,
                    if value == expected { " -> " } else { "" },
                );
                if value == expected {
                    let _ = writeln!(log.stamp(), "probe: the amplifier is {}", part);
                }
            }
            Err(_) => {
                let _ = writeln!(log.stamp(), "probe: amplifier register {:#04x}: no answer at {:#04x}", register, AMPLIFIER);
            }
        }
    }

    // Codec and amplifier back as they were.
    drop(clock);
    drop(bus_spi);
    codec_awake.set_low();
    drop(codec_reset);
    drop(select);
    drop(_amplifier_reset);
    drop(_amplifier_supply);
    watchdog::feed();

    // --- NFC ------------------------------------------------------------------
    let mut supply = Output::new(r.nfc_supply, Level::High, Speed::Low);
    let mut download = Output::new(r.nfc_download, Level::Low, Speed::Low);
    let mut enable = Output::new(r.nfc_enable, Level::Low, Speed::Low);
    let interrupt = Input::new(r.nfc_interrupt, Pull::Down);
    Timer::after_millis(100).await;
    watchdog::feed();
    Timer::after_millis(100).await;
    enable.set_high();
    Timer::after_millis(100).await;
    watchdog::feed();
    // NCI "core reset", keeping the configuration.
    let mut taken = false;
    for _ in 0..5 {
        if bus.blocking_write(NFC, &[0x20, 0x00, 0x01, 0x01]).is_ok() {
            taken = true;
            break;
        }
        Timer::after_millis(10).await;
    }
    let _ = writeln!(log.stamp(), "probe: NFC controller {} the core reset at {:#04x}", if taken { "took" } else { "did NOT take" }, NFC);
    if taken {
        // Its answer and what it sends by itself after a reset: while its
        // interrupt line asks, for a quarter of a second at most.
        for _ in 0..25 {
            if interrupt.is_high() {
                let mut head = [0u8; 3];
                if bus.blocking_read(NFC, &mut head).is_err() {
                    let _ = writeln!(log.stamp(), "probe: NFC asked to be read and did not answer the read");
                    break;
                }
                let mut rest = [0u8; 32];
                let length = (head[2] as usize).min(rest.len());
                let read = length == 0 || bus.blocking_read(NFC, &mut rest[..length]).is_ok();
                let _ = writeln!(log.stamp(), "probe: NFC says {:02x} {:02x} {:02x}, {} more{}", head[0], head[1], head[2], head[2], if read { ":" } else { ", which could not be read" });
                if read && length > 0 {
                    log.hex(&rest[..length]);
                }
            }
            Timer::after_millis(10).await;
        }
    }
    // Off, in the stock app's order.
    supply.set_low();
    download.set_low();
    enable.set_low();
    Timer::after_millis(2).await;
    drop((supply, download, enable, interrupt));
    watchdog::feed();
    let _ = writeln!(log.stamp(), "probe: done; all three chips are off again");
}
