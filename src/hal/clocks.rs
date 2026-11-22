//! Clock tree.
//!
//! The stock firmware (bootloader 0x0800C932, app 0x0804DE98) runs from the
//! external crystal through the main PLL: divide by 3, multiply by 30,
//! divide by 2 = 120 MHz, with flash latency 5 in voltage range 1 "boost".
//! SDMMC1 is fed separately, from the internal MSI oscillator at 48 MHz.
//!
//! Measured on the watch: the crystal is 24 MHz (24.01 MHz against the
//! crystal-trimmed MSI, using TIM17's internal HSE/32 input), and boost mode
//! is still active when the bootloader hands over (PWR_CR5 = 0).
//!
//! This app uses the same crystal and PLL, but multiplies by 24 for 96 MHz:
//! the HAL (embassy-stm32 0.6) only sets flash latencies up to 4, which is
//! valid up to 100 MHz, so 120 MHz needs clock code of our own. 96 MHz also
//! divides exactly to the Bluetooth UART's 3,000,000 baud.

use core::fmt::Write;
use core::ptr::{read_volatile, write_volatile};

use embassy_stm32::rcc::mux::{Adcsel, Clk48sel};
use embassy_stm32::rcc::{LsConfig, LseConfig, LseDrive, LseMode, RtcClockSource};
use embassy_stm32::rcc::{Hse, HseMode, MSIRange, Pll, PllMul, PllPreDiv, PllRDiv, PllSource, Sysclk};
use embassy_stm32::time::Hertz;
use embassy_stm32::Config;

use crate::hal::reg::{RCC_APB1ENR1, RCC_BDCR, RCC_CFGR, RCC_CR, RCC_PLLCFGR};
use crate::services::log::Log;

pub const CRYSTAL_HZ: u32 = 24_000_000;
/// SDMMC1's clock, independent of the system clock.
pub const SDMMC_KERNEL_HZ: u32 = 48_000_000;

const PWR_CR5: *mut u32 = 0x4000_7080 as *mut u32;
const FLASH_ACR: *const u32 = 0x4002_2000 as *const u32;

/// Fill in the clock part of the HAL configuration. Call before
/// `embassy_stm32::init`.
pub fn configure(config: &mut Config) {
    // Voltage range 1 boost (PWR_CR5.R1MODE = 0) is required above 80 MHz.
    // The bootloader leaves it on; make sure, since the HAL does not set it.
    unsafe {
        write_volatile(RCC_APB1ENR1, read_volatile(RCC_APB1ENR1) | 1 << 28); // PWR clock
        let _ = read_volatile(RCC_APB1ENR1);
        write_volatile(PWR_CR5, read_volatile(PWR_CR5) & !(1 << 8));
    }

    config.rcc.hse = Some(Hse { freq: Hertz(CRYSTAL_HZ), mode: HseMode::Oscillator });
    config.rcc.pll = Some(Pll {
        source: PllSource::HSE,
        prediv: PllPreDiv::DIV3, // 8 MHz into the PLL
        mul: PllMul::MUL24,      // 192 MHz
        divp: None,
        divq: None,
        divr: Some(PllRDiv::DIV2), // 96 MHz system clock
    });
    config.rcc.sys = Sysclk::PLL1_R;
    // Display pixel clock, as in the stock app: PLLSAI2 = HSE / 3 * 10 / 2 =
    // 40 MHz, which the LTDC divides by 2.
    config.rcc.pllsai2 = Some(Pll {
        source: PllSource::HSE,
        prediv: PllPreDiv::DIV3,
        mul: PllMul::MUL10,
        divp: None,
        divq: None,
        divr: Some(PllRDiv::DIV2),
    });
    // MSI stays on at 48 MHz as the 48 MHz peripheral clock (SDMMC1), as in
    // the stock firmware (CLK48SEL = MSI).
    config.rcc.msi = Some(MSIRange::RANGE48M);
    config.rcc.mux.clk48sel = Clk48sel::MSI;
    // The converter's clock is the system clock, as in the stock firmware
    // (its peripheral clock set-up at 0x0804DF40 selects SYSCLK for the
    // ADC); it is divided at the converter, see `board::RevisionPin`.
    config.rcc.mux.adcsel = Adcsel::SYS;
}

/// Log the clock registers as they are now.
pub fn log_state(log: &mut Log) {
    let (cr, cfgr, pllcfgr, acr, cr5) = unsafe {
        (
            read_volatile(RCC_CR),
            read_volatile(RCC_CFGR),
            read_volatile(RCC_PLLCFGR),
            read_volatile(FLASH_ACR),
            read_volatile(PWR_CR5),
        )
    };
    let source = ["MSI", "HSI16", "HSE", "PLL"][((cfgr >> 2) & 0b11) as usize];
    let _ = writeln!(
        log.stamp(),
        "clocks: system clock from {}, flash latency {}, boost {}; RCC_CR={:#010x} CFGR={:#010x} PLLCFGR={:#010x}",
        source,
        acr & 0b111,
        if cr5 & 1 << 8 == 0 { "on" } else { "off" },
        cr,
        cfgr,
        pllcfgr
    );
}

/// Describe the RTC/LSE setup that is already running, instead of choosing one.
///
/// The HAL resets the whole backup domain (RTC time and backup registers) when
/// RCC_BDCR differs from its configuration. The first round trip did exactly
/// that: the watch came back with its clock at 2000-01-01. Reading BDCR and
/// handing the HAL the same settings leaves the stock firmware's RTC alone.
pub fn adopt_low_speed_clocks() -> LsConfig {
    let bdcr = unsafe { read_volatile(RCC_BDCR) };
    let lse_on = bdcr & 1 != 0;
    let lse_bypass = bdcr & (1 << 2) != 0;
    let lse_drive = match (bdcr >> 3) & 0b11 {
        0 => LseDrive::Low,
        1 => LseDrive::MediumLow,
        2 => LseDrive::MediumHigh,
        _ => LseDrive::High,
    };
    let rtc_enabled = bdcr & (1 << 15) != 0;
    let rtc = match (bdcr >> 8) & 0b11 {
        1 if rtc_enabled && lse_on => RtcClockSource::LSE,
        2 if rtc_enabled => RtcClockSource::LSI,
        _ => RtcClockSource::DISABLE,
    };
    LsConfig {
        rtc,
        lsi: true, // the watchdog runs from LSI anyway
        lse: lse_on.then_some(LseConfig {
            frequency: Hertz(32_768),
            mode: if lse_bypass { LseMode::Bypass } else { LseMode::Oscillator(lse_drive) },
        }),
    }
}
