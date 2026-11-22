//! Ambient light sensor on the sensor bus (I2C2).
//!
//! The stock app drives an STK31800 at address 0x45 (driver at 0x080777E0):
//! product ID in register 0x3E (it accepts 0x80 and 0x82), software reset
//! by writing register 0x80, a table of 14 register settings, then "ALS on"
//! by writing 2 to the state register 0. It reads two 16-bit values, high
//! byte first: register 0x13 (the light channel) and 0x15 (a second channel
//! used to tell the kind of light).
//!
//! The settings table is in the stock app's initialised data (RAM
//! 0x20034CF8). It was recovered by running the app's own unpacking routine
//! (0x081A8638) on the image in an emulator; `SETTINGS` is that table.
//!
//! Lux (stock: 0x08077948, then 0x080779E8): the light channel, scaled by a
//! factor that depends on the ratio of the two channels, times a
//! calibration factor measured for each watch at the factory (see
//! `services::factory::light_calibration`; the stock default is 0.174).
//!
//! Not done: the stock app also subtracts two offsets that it works out
//! from what the display is showing (0x08077B58), because the sensor sits
//! under the display. Here they are zero; measured on this watch, the
//! display's light made no clear difference to the reading.
//!
//! The stock app also carries a driver for a part at 0x53 with a different
//! register map (part ID in register 6); only one of the two is fitted.

use core::fmt::Write;

use embassy_stm32::i2c::{I2c, Master};
use embassy_stm32::mode::Blocking;
use embassy_time::Timer;

use crate::services::log::Log;

pub type Bus = I2c<'static, Blocking, Master>;

const STK_ADDRESS: u8 = 0x45;
const ALTERNATIVE_ADDRESS: u8 = 0x53;

const REG_STATE: u8 = 0x00;
const REG_DATA_ALS: u8 = 0x13;
const REG_PRODUCT_ID: u8 = 0x3E;
const REG_SOFT_RESET: u8 = 0x80;
const STATE_ALS_ON: u8 = 0x02;
/// The stock app's register settings, in its order. Register 2 is the light
/// channel's gain (bits 5:4: x1, x4, x16, x64, measured) and measurement
/// time (bits 3:0); 0x4E the second channel's gain. What the others do is
/// not known.
const SETTINGS: [(u8, u8); 14] = [
    (0x02, 0x25),
    (0x04, 0x00),
    (0x05, 0x40),
    (0x10, 0x00),
    (0x4E, 0x00),
    (0x6F, 0x00),
    (0xA1, 0x03),
    (0xA5, 0x00),
    (0xDB, 0x00),
    (0x60, 0x02),
    (0x61, 0x00),
    (0x62, 0x00),
    (0x63, 0x00),
    (0xF6, 0x09),
];

/// The calibration factor the stock app uses when a watch has none.
pub const DEFAULT_FACTOR: f32 = 0.174;

pub struct LightSensor {
    /// Turns the scaled light channel into lux.
    factor: f32,
}

/// Raw readings: the light channel and the second channel.
#[derive(Clone, Copy)]
pub struct Raw {
    pub light: u16,
    pub second: u16,
}

/// Log every address on the bus that answers. Returns how many did.
pub fn scan(bus: &mut Bus, log: &mut Log) -> usize {
    let _ = write!(log.stamp(), "sensor bus: devices at");
    let mut found = 0;
    for address in 0x08..0x78u8 {
        if bus.blocking_write(address, &[]).is_ok() {
            let _ = write!(log, " {:#04x}", address);
            found += 1;
        }
    }
    let _ = writeln!(log, "{}", if found == 0 { " (none)" } else { "" });
    found
}

impl LightSensor {
    /// Look for the sensor, reset it, give it the stock settings and switch
    /// light measurement on. `factor` is the watch's calibration factor.
    pub async fn start(bus: &mut Bus, factor: f32, log: &mut Log) -> Option<Self> {
        let mut id = [0u8];
        match bus.blocking_write_read(STK_ADDRESS, &[REG_PRODUCT_ID], &mut id) {
            Ok(()) => {
                let _ = writeln!(log.stamp(), "light sensor: product ID {:#04x} at {:#04x}", id[0], STK_ADDRESS);
            }
            Err(e) => {
                let _ = writeln!(log.stamp(), "light sensor: nothing at {:#04x} ({:?})", STK_ADDRESS, e);
                // Which part is it then? Log the alternative's ID.
                match bus.blocking_write_read(ALTERNATIVE_ADDRESS, &[0x06], &mut id) {
                    Ok(()) => {
                        let _ = writeln!(log.stamp(), "light sensor: part ID {:#04x} at {:#04x}; no driver for it yet", id[0], ALTERNATIVE_ADDRESS);
                    }
                    Err(e) => {
                        let _ = writeln!(log.stamp(), "light sensor: nothing at {:#04x} either ({:?})", ALTERNATIVE_ADDRESS, e);
                    }
                }
                return None;
            }
        }
        if bus.blocking_write(STK_ADDRESS, &[REG_SOFT_RESET, 0xFF]).is_err() {
            let _ = writeln!(log.stamp(), "light sensor: reset not accepted");
            return None;
        }
        Timer::after_millis(30).await;
        for (register, value) in SETTINGS.into_iter().chain([(REG_STATE, STATE_ALS_ON)]) {
            if bus.blocking_write(STK_ADDRESS, &[register, value]).is_err() {
                let _ = writeln!(log.stamp(), "light sensor: register {:#04x} could not be set", register);
                return None;
            }
        }
        let _ = writeln!(log.stamp(), "light sensor: on");
        Some(Self { factor })
    }

    /// Brightness in lux, as the stock app works it out: the light channel,
    /// scaled by a factor that depends on the ratio of the second channel
    /// to it (which tells the kind of light source), times the watch's
    /// calibration factor.
    pub fn lux(&self, raw: Raw) -> u32 {
        let light = raw.light as u32;
        if light == 0 {
            return 0;
        }
        let ratio = raw.second as u32 * 1000 / light;
        let percent = match ratio {
            0..=179 => 100,
            180..=229 => 136,
            230..=309 => 78,
            _ => 63,
        };
        ((light * percent / 100) as f32 * self.factor) as u32
    }

    /// The latest raw values, or `None` if the sensor did not answer.
    pub fn read(&mut self, bus: &mut Bus) -> Option<Raw> {
        let mut data = [0u8; 4];
        bus.blocking_write_read(STK_ADDRESS, &[REG_DATA_ALS], &mut data).ok()?;
        Some(Raw { light: u16::from_be_bytes([data[0], data[1]]), second: u16::from_be_bytes([data[2], data[3]]) })
    }
}
