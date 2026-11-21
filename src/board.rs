//! The board: which pin and which peripheral of the STM32 goes where.
//!
//! This is the one place that hands out pins. Some peripherals are driven
//! at register level and set up their own pins; they are listed here too so
//! that the whole assignment can be read in one place.
//!
//! | Function | Pins / peripheral | Set up by |
//! |---|---|---|
//! | Power rails and enables | PC3, PF3, PA1, PA12 high; PG4 pulsed low | `PowerPins` |
//! | eMMC | SDMMC1: PC8-PC11 data, PC12 clock, PD2 command | `hal::sdmmc` |
//! | Bluetooth chip | USART3: PC4 TX, PC5 RX, PB13 CTS, PB14 RTS; PB2 power, PC6 reset, PB9 wake, PC7 interrupt; DMA1 channels 1, 2 | `drivers::cypress` |
//! | Apollo | SPI1 slave: PA5 clock, PA6, PA7; PG1 request, PA15 ready, PF10 have-data, PG12 reset, PF7; DMA1 channels 3, 6 | `drivers::apollo` |
//! | Display | LTDC, DSI host; PF11 tearing effect; PG5, PF12 supplies, PF5 power, PA11 reset | `drivers::panel`, `hal::dsi` |
//! | PSRAM | FMC bank 1: port D except PD2, PB7, PE0, PE1, PE3-PE15, PG9; PE2 CRE | `drivers::psram`, `hal::fmc` |
//! | Upper button | PC13 | `drivers::buttons` |
//! | Crystals | PH0, PH1 (24 MHz); PC14, PC15 (32.768 kHz) | `hal::clocks` |
//!
//! The lower button is on the Apollo. See `docs/hardware-plan.md` for the
//! chips that are not driven yet and their pins.

use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::{peripherals, Peri, Peripherals};
use embassy_time::Timer;

use crate::drivers::{apollo, cypress, panel};

pub struct Board {
    pub power: PowerPins,
    pub bluetooth: cypress::Resources,
    pub apollo: apollo::Resources,
    pub display: panel::Resources,
    pub upper_button: Peri<'static, peripherals::PC13>,
}

/// The enables the bootloader's `bsp_gpio_power_init` sets. Which of them
/// feeds what is not known, except that the eMMC needs them.
pub struct PowerPins {
    pc3: Peri<'static, peripherals::PC3>,
    pf3: Peri<'static, peripherals::PF3>,
    pa1: Peri<'static, peripherals::PA1>,
    pg4: Peri<'static, peripherals::PG4>,
    pa12: Peri<'static, peripherals::PA12>,
}

impl Board {
    pub fn new(p: Peripherals) -> Self {
        Self {
            power: PowerPins { pc3: p.PC3, pf3: p.PF3, pa1: p.PA1, pg4: p.PG4, pa12: p.PA12 },
            bluetooth: cypress::Resources {
                usart: p.USART3,
                tx: p.PC4,
                rx: p.PC5,
                cts: p.PB13,
                rts: p.PB14,
                power: p.PB2,
                reset: p.PC6,
                wake: p.PB9,
                irq: p.PC7,
                rx_dma: p.DMA1_CH1,
                tx_dma: p.DMA1_CH2,
            },
            apollo: apollo::Resources {
                request: p.PG1,
                ready: p.PA15,
                have_data: p.PF10,
                reset: p.PG12,
                pf7: p.PF7,
                _sck: p.PA5,
                _miso: p.PA6,
                _mosi: p.PA7,
            },
            display: panel::Resources {
                supply_pg5: p.PG5,
                supply_pf12: p.PF12,
                power: p.PF5,
                reset: p.PA11,
                _te: p.PF11,
                _ltdc: p.LTDC,
                _dsi: p.DSIHOST,
            },
            upper_button: p.PC13,
        }
    }
}

impl PowerPins {
    /// Same pins, same levels, same order as the bootloader. It resets
    /// ports B, C and D before jumping to us, so at least PC3 is off again.
    /// The pins stay driven afterwards.
    pub async fn switch_on(self) {
        embassy_stm32::pac::PWR.cr2().modify(|w| w.set_iosv(true)); // port G supply valid
        let pc3 = Output::new(self.pc3, Level::High, Speed::VeryHigh);
        let pf3 = Output::new(self.pf3, Level::High, Speed::VeryHigh);
        let pa1 = Output::new(self.pa1, Level::High, Speed::VeryHigh);
        let mut pg4 = Output::new(self.pg4, Level::High, Speed::VeryHigh);
        pg4.set_low(); // 100 ms low pulse: looks like a reset line
        Timer::after_millis(100).await;
        pg4.set_high();
        let pa12 = Output::new(self.pa12, Level::High, Speed::VeryHigh);
        core::mem::forget((pc3, pf3, pa1, pg4, pa12));
        Timer::after_millis(50).await;
    }
}
