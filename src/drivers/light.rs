//! Ambient light sensor on the sensor bus (I2C2).
//!
//! The stock app drives an STK31800 at address 0x45 (driver at 0x080777E0):
//! product ID in register 0x3E (it accepts 0x80 and 0x82), software reset
//! by writing register 0x80, a table of 14 register settings (kept in the
//! app's initialised data, not recovered yet), then "ALS on" by writing 2
//! to the state register 0. It reads two 16-bit values, high byte first:
//! register 0x13 (the light channel) and 0x15 (a second channel used to
//! tell the kind of light), and derives lux from them and two calibration
//! offsets.
//!
//! The stock app also carries a driver for a part at 0x53 with a different
//! register map (part ID in register 6); only one of the two is fitted.
//!
//! Settings: the stock table is not known, so the gains were found on the
//! watch by changing one register at a time under steady light (see the
//! register constants). On reset defaults desk light reads about 5 counts;
//! with the light channel at x64 it reads about 340. The second channel is
//! set to x4, because that puts the ratio of the two channels (about 0.27
//! under indoor light) into the range the stock lux formula distinguishes
//! (0.18 to 0.31). That choice is an inference, and the stock calibration
//! offsets are not applied, so treat the lux value as approximate.

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
/// Light channel: gain in bits 5:4 (x1, x4, x16, x64), measurement time in
/// bits 3:0 (each two steps quadruple the reading; 5 after reset).
const REG_ALS_CONTROL: u8 = 0x02;
const ALS_GAIN_64: u8 = 0x30;
const ALS_TIME_DEFAULT: u8 = 0x05;
/// Second channel: gain in bits 5:4 (x1, x4, x16, x64); bit 2 is a further
/// x128 or so.
const REG_GAIN_CONTROL: u8 = 0x4E;
const SECOND_GAIN_4: u8 = 0x10;

pub struct LightSensor {
    _private: (),
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
    /// Look for the sensor, reset it and switch light measurement on.
    pub async fn start(bus: &mut Bus, log: &mut Log) -> Option<Self> {
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
        let settings = [(REG_ALS_CONTROL, ALS_GAIN_64 | ALS_TIME_DEFAULT), (REG_GAIN_CONTROL, SECOND_GAIN_4), (REG_STATE, STATE_ALS_ON)];
        for (register, value) in settings {
            if bus.blocking_write(STK_ADDRESS, &[register, value]).is_err() {
                let _ = writeln!(log.stamp(), "light sensor: register {:#04x} could not be set", register);
                return None;
            }
        }
        let _ = writeln!(log.stamp(), "light sensor: on");
        Some(Self { _private: () })
    }

    /// Brightness in lux, by the stock app's formula (0x08077948): the
    /// light channel, scaled by a factor that depends on the ratio of the
    /// second channel to it (which tells the kind of light source).
    pub fn lux(raw: Raw) -> u32 {
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
        light * percent / 100
    }

    /// The latest raw values, or `None` if the sensor did not answer.
    pub fn read(&mut self, bus: &mut Bus) -> Option<Raw> {
        let mut data = [0u8; 4];
        bus.blocking_write_read(STK_ADDRESS, &[REG_DATA_ALS], &mut data).ok()?;
        Some(Raw { light: u16::from_be_bytes([data[0], data[1]]), second: u16::from_be_bytes([data[2], data[3]]) })
    }
}
