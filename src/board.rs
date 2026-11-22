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
//! | Sensor bus | I2C2: PF1 clock, PF0 data (external pull-ups); light sensor 0x45, NFC 0x28, a device at 0x36 | `SensorBus`, `drivers::light` |
//! | Touch controller | I2C3: PC0 clock, PB4 data (external pull-ups), address 0x24; PF13 reset, PF4 enable, PA0 interrupt | `drivers::touch` |
//! | Upper button | PC13 | `drivers::buttons` |
//! | Board revision | ADC1 channel 15 on PB0 (a resistor divider) | `RevisionPin` |
//! | Audio codec supply | PB1, on board revisions 1 and 3 only | `CodecSupply` |
//! | Crystals | PH0, PH1 (24 MHz); PC14, PC15 (32.768 kHz) | `hal::clocks` |
//!
//! The lower button is on the Apollo. See `docs/hardware-plan.md` for the
//! chips that are not driven yet and their pins.

use embassy_stm32::adc::{Adc, SampleTime};
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::{peripherals, Peri, Peripherals};
use embassy_stm32::i2c::{self, I2c};
use embassy_stm32::time::Hertz;
use embassy_time::{Duration, Timer};

use crate::drivers::{apollo, cypress, light, panel, touch};

pub struct Board {
    pub power: PowerPins,
    pub bluetooth: cypress::Resources,
    pub apollo: apollo::Resources,
    pub display: panel::Resources,
    pub upper_button: Peri<'static, peripherals::PC13>,
    pub sensor_bus: SensorBus,
    pub touch: touch::Resources,
    pub revision: RevisionPin,
    pub codec_supply: CodecSupply,
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
                blitter: p.DMA2D,
            },
            upper_button: p.PC13,
            sensor_bus: SensorBus { i2c: p.I2C2, scl: p.PF1, sda: p.PF0 },
            revision: RevisionPin { adc: p.ADC1, pin: p.PB0 },
            codec_supply: CodecSupply { pb1: p.PB1 },
            touch: touch::Resources { i2c: p.I2C3, scl: p.PC0, sda: p.PB4, reset: p.PF13, enable: p.PF4, irq: p.PA0 },
        }
    }
}

/// The audio codec's supply enable.
///
/// On board revisions 1 and 3 the stock app switches PB1 together with the
/// codec: high when it wakes the codec, low when it puts it to sleep (its
/// `BSP_GPIO_Audio_Codec_Exit_Slp` and the matching entry function, in the
/// running build B.70 at 0x080A9B88 and before it; they also start and
/// stop SPI2 and SAI1). At start it drives PB1 high for every revision
/// but 0 and 2 (0x0805CA2C in B.70). On revisions 0 and 2 it never
/// touches the pin.
pub struct CodecSupply {
    pb1: Peri<'static, peripherals::PB1>,
}

impl CodecSupply {
    /// Leave the codec without supply, as the stock app does while the
    /// codec sleeps. There is no codec driver yet; until there is, the
    /// pin is held low instead of floating. Does nothing on revisions
    /// where the stock app leaves the pin alone.
    pub fn off(self, revision: u8) -> bool {
        if revision == 1 || revision == 3 {
            core::mem::forget(Output::new(self.pb1, Level::Low, Speed::Low));
            true
        } else {
            false
        }
    }
}

/// The board revision, coded as a voltage on PB0.
pub struct RevisionPin {
    adc: Peri<'static, peripherals::ADC1>,
    pin: Peri<'static, peripherals::PB0>,
}

impl RevisionPin {
    /// Measure the voltage and return the revision number the stock app
    /// derives from it, with the average reading for the log.
    ///
    /// The stock app does this once at start (`0x0805C7A4`): five
    /// conversions, the highest and lowest dropped, the rest averaged and
    /// taken as a fraction of 1.8 V. Its classification, found by running
    /// that routine in an emulator for every possible reading: below 1.55 V
    /// revision 3, from 1.55 V revision 0, from 1.60 V revision 2; a
    /// reading of zero leaves its start value 0. The Apollo asks for this
    /// number after it has started and names the revisions B280_DVT (0),
    /// B280_PVT (1) and B281_DVT (2).
    ///
    /// The converter is set up as in the stock app (0x0808F224): clocked
    /// from the system clock divided by 128, 12 bits, channel 15, the
    /// shortest sampling time.
    pub fn measure(mut self) -> (u8, u16) {
        let mut adc = Adc::new(self.adc);
        // The HAL calibrates at the full system clock, which is above the
        // converter's limit. Divide the clock by 128 as the stock app does
        // and calibrate again.
        embassy_stm32::pac::ADC1_COMMON.ccr().modify(|w| w.0 = (w.0 & !(0xF << 18)) | 0b1010 << 18);
        embassy_stm32::pac::ADC1.cr().modify(|w| w.0 = (w.0 & 0x3000_0000) | 1 << 31); // start calibration
        for _ in 0..2_000_000 {
            if embassy_stm32::pac::ADC1.cr().read().0 & 1 << 31 == 0 {
                break;
            }
        }
        let mut samples = [0u16; 5];
        for sample in samples.iter_mut() {
            *sample = adc.blocking_read(&mut self.pin, SampleTime::Cycles25);
        }
        let sum: u32 = samples.iter().map(|&s| s as u32).sum();
        let lowest = *samples.iter().min().unwrap_or(&0) as u32;
        let highest = *samples.iter().max().unwrap_or(&0) as u32;
        let average = ((sum - lowest - highest) / 3) as u16;
        let revision = match average {
            0 => 0,
            1..=3527 => 3,
            3528..=3640 => 0,
            _ => 2,
        };
        (revision, average)
    }
}

/// I2C2, shared by the light sensor, the NFC controller and one more device.
pub struct SensorBus {
    i2c: Peri<'static, peripherals::I2C2>,
    scl: Peri<'static, peripherals::PF1>,
    sda: Peri<'static, peripherals::PF0>,
}

impl SensorBus {
    /// Open the bus at 100 kHz. A transfer gives up after 10 ms, so a
    /// stuck bus cannot hold the main loop for long.
    pub fn open(self) -> light::Bus {
        let mut config = i2c::Config::default();
        config.frequency = Hertz(100_000);
        config.timeout = Duration::from_millis(10);
        I2c::new_blocking(self.i2c, self.scl, self.sda, config)
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
