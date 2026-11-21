# W301 hardware bring-up plan

Written 2026-11-21 from the firmware files in the watch dump
(`~/opwatch/work/dump/mirror`), the stock bootloader and app disassembly, and
what the Rust firmware has measured on the watch so far.

Every statement is marked by how it is known:

- **measured**: seen on the watch from the Rust firmware
- **stock code**: read from the stock bootloader or app
- **strings**: inferred from names and log texts in a firmware image
- **guess**: my inference, to be checked

## 1. Inventory

The watch has two processors that own hardware: the STM32L4R9 (this
firmware) and the Apollo co-processor. A third, the Cypress Bluetooth chip,
runs its own stack. Each chip below hangs off one of them.

### Attached to the STM32

| # | Chip / function | Interface | Firmware file | State in Rust |
|---|---|---|---|---|
| 1 | eMMC, ~3.6 GB, four FAT32 volumes | SDMMC1 (PC8-12, PD2) | none | works (1-bit, 12 MHz) |
| 2 | Cypress CYW20721 Bluetooth | USART3 3 Mbaud (PC4, PC5, PB13, PB14), PB2, PC6, PB9, PC7 | `cypress/appdriver.bin`, `minidriver.bin` | works for existing pairings |
| 3 | Apollo co-processor | SPI1 slave (PA5-7), PG1, PA15, PF10, PG12, PF7 | `apollo/apollo_app.bin` | link works, few messages understood |
| 4 | AMOLED panel 454 x 454 | DSI + LTDC, TE on PF11, PG5, PF12, PF5, PA11 | none | test picture works |
| 5 | PSRAM 16 MB | FMC bank 1, ~30 pins, CRE on PE2 | none | works |
| 6 | Touch controller | I2C3 (PB4, PC0), address 0x24 | `tp/tp_firmware.bin` | nothing |
| 7 | Audio codec Cirrus CS47Lxx with DSP | SPI (control), SAI1 (audio) | `codec/adsp_for_*.bin` (13 files) | nothing |
| 8 | Speaker and microphone | through the codec | tones in `audio/*.wav` | nothing |
| 9 | NFC controller NXP SN100T with secure element | I2C2, address 0x28 | `nfc/nfc_firmware.bin`, two `.conf` files | nothing |
| 10 | Ambient light sensor STK31800 | I2C2, address 0x45 | none | nothing |
| 11 | Buttons: upper on PC13, lower through the Apollo (message kind 51) | GPIO; Apollo link | none | both read and logged |
| 12 | GPS Airoha AG3335, firmware side | shared with the Apollo | `gps/*.bin` (5 files) | nothing |
| 13 | Real-time clock, backup domain | internal, LSE crystal | none | left running, not used |
| 14 | Log UART | LPUART1 (PG7, PG8) | none | not used |

### Attached to the Apollo

The Rust firmware does not drive these directly. It gets their data, and
would send their commands, through protobuf messages on the Apollo link.
The Apollo names them itself in message kind 52 (**measured**).

| # | Chip | Function | State in Rust |
|---|---|---|---|
| 15 | LSM6DSO (firmware also carries BMI270 and ICM-40608 drivers) | accelerometer and gyroscope | nothing decoded |
| 16 | MMC5603 | magnetometer | nothing decoded |
| 17 | ICP-101xx | barometer | decoded: pressure, altitude |
| 18 | MediaTek MT6670 | heart rate and SpO2 optical front end | nothing decoded |
| 19 | SX932x | capacitive wear detection | decoded: worn / not worn |
| 20 | BQ25618 | battery charger | partly: USB power flag |
| 21 | CW2015 | fuel gauge | decoded: percent, millivolts |
| 22 | AW8695 | haptic motor driver | nothing |
| 23 | AG3335 | GPS, position data side | nothing decoded |
| 24 | NTC thermistors | three temperatures | decoded |

## 2. The STM32's own buses and pins

All from **stock code** (bus setup at `0x08090380`, the transfer wrappers and
their callers, and every GPIO setup call in the app), unless marked.

| Bus | Pins | Device | Address (7-bit) |
|---|---|---|---|
| I2C2 | PF0, PF1 | light sensor STK31800 | 0x45 |
| I2C2 | | second light-sensor driver, for a part with a Lite-On style register map (alternative fitting, **guess**) | 0x53 |
| I2C2 | | NFC controller SN100T | 0x28 |
| I2C2 | | device with 16-bit registers, most likely the speaker amplifier (**guess**, see 3.6) | 0x36 |
| I2C3 | PB4, PC0 | touch controller | 0x24 |
| I2C4 | PF14, PF15 (pull-ups) | the Apollo, for its firmware update only | 0x20 |
| SPI1 | PA5, PA6, PA7 | Apollo message link (STM32 is the slave) | |
| SPI2 | PB10 clock, PC1, PC2 data, PB12 chip select | not traced to a device; the audio codec is the likely one (**guess**) | |
| SAI1 | PA8, PA9, PA10, PB5 | audio samples | |
| LPUART1 | PG7, PG8 | log output | |
| ADC1 | PB0 (analog) | not traced | |

Other pins the stock app sets up:

| Pin | Setup | Likely role |
|---|---|---|
| PA0 | interrupt on falling edge, pull-up | not a button (**measured**); role unknown |
| PC13 | interrupt on both edges, pull-up | upper button (**measured**) |
| PF2 | output | touch reset (**guess**: set up next to PG2) |
| PG2 | interrupt on rising edge, pull-down | touch interrupt (**guess**) |
| PA4, PB3 | outputs, driven low at start | unknown |
| PB1 | output | unknown, set up with the ADC pin PB0 |
| PB6, PB12 and two pins in a group of 6 and 10 | outputs | unknown |

Cross-check: the stock app's clock selections name exactly USART3, LPUART1,
I2C2, I2C3, I2C4, SAI1, SDMMC1, LTDC, DSI, RTC and ADC. There is no hidden
bus beyond the table.

## 3. Plan per chip

Effort is relative: S = an evening like the PSRAM, M = like the Apollo
link, L = several sessions.

### 3.1 Touch controller (item 6) - M

Known: on I2C3 at address 0x24 (**stock code**; the read and write
wrappers are at `0x080A0018` and `0x080A00CE`, with "TP I2C" log texts).
0x24 is the default address of Cypress/Parade TrueTouch controllers, and the
threshold file `0:cmcp_threshold_file.csv` is that family's format. The
stock app has a `TouchTask`, a firmware update path, a reset with timing and
an "underwater mode". Reset is probably PF2, the interrupt PG2 (**guess**).

Steps:
1. Confirm the reset and interrupt pins from the stock touch task.
2. First image: reset the chip, read its identification, log it.
3. Read touch reports on interrupt; log coordinates.
4. Draw a dot under the finger. This is the first interactive feature.

Risk: TrueTouch controllers speak a packet protocol with a bootloader mode.
If the chip comes up in bootloader mode it needs a start command first.

### 3.2 Buttons (item 11) - done

**Measured:** the upper button is PC13 (pull-up, pressed reads low). The
lower button is wired to the Apollo, which reports it in message kind 51:
pressed, released, clicked, and long press after 1.5 s. PA0, set up by the
stock app as a falling-edge interrupt, is not a button; it follows something
else (it went high about 3.8 s after boot in one run and stayed low in
another).

Remaining:
1. Click and long-press detection for the upper button, matching what the
   Apollo does for the lower one.
2. Use a button to switch the display off and on, and a long hold as a
   "return to stock" gesture that does not depend on Bluetooth.

### 3.3 Display, remaining work (item 4) - M

Works: panel on, one frame from the PSRAM.

Steps:
1. Drawing layer: `embedded-graphics` on the framebuffer, text and shapes.
2. Brightness (command `0x51`) and display off / on with the stock
   sequences (`0x08039464`, `0x080397E2`).
3. Partial refresh: the stock app updates regions; needed for smooth
   drawing without tearing.
4. Double buffering in the PSRAM (the stock app reserves 2 MB at
   `0x60C00000`; one frame is 412 KB).
5. Always-on mode (`lcd_alwayson` in the stock strings): later.

### 3.4 Ambient light sensor (item 10) - S

Known: STK31800 on I2C2 at address 0x45 (**stock code**, driver at
`0x080777E0`: it reads register 0x3E, the product ID, and writes 0x80, the
reset). A `TaskLightSensor` drives automatic brightness. The app also
carries a driver for a second part at 0x53; only one of the two is fitted.

Steps:
1. Bring up I2C2; read the product ID at both addresses, log which answers.
2. Read lux; log it.
3. Tie brightness to it.

A good first I2C device: simple chip, and it tells us which bus is which.

### 3.5 Apollo, remaining work (item 3 and items 15-24) - L

Works: transport, heartbeat, acknowledgements, 9 of 79 message kinds
identified.

Steps:
1. Name the rest of the schema. For each kind: the handler's log text in
   the stock app, then a capture on the watch. Kinds seen but not solved:
   4, 7, 51, 54, 80.
2. Sending: the stock app's outgoing messages (sensor enable, settings,
   time, user data). The stock function that builds them is at
   `0x0806C40A`; its callers give the list.
3. Motion data (LSM6DSO): find how the stock app requests raw or step
   data; decode kind 81 "daily activity".
4. Heart rate and SpO2 (MT6670): find the start/stop messages; the Apollo
   runs the algorithm and reports results.
5. Haptics (AW8695): find the vibrate message; fire it.
6. GPS position data: see 3.9.
7. Battery and charger: complete the fields of kind 47 (fault mask,
   charge state).
8. Sleep: the stock app tells the Apollo when the STM32 sleeps
   (`service_snd_req_to_sleep_apollo`); needed before we add low power.
9. Apollo firmware update: the stock app can reflash it
   (`start to update APOLLO`). It does so over I2C4 (PF14, PF15, address
   0x20) from a task at `0x08079A64` that also toggles the Apollo's reset
   pin, started from the update screen. Not needed unless we want to change
   the Apollo's firmware; do not drive I2C4 otherwise.

The Apollo's firmware is 811 KB with full driver names. If a message stays
unclear from the STM32 side, the Apollo image can be read as the other half.

### 3.6 Audio codec, speaker, microphone (items 7, 8) - L

Known: Cirrus CS47Lxx, controlled over SPI
(`cs47lxx_spi_read_single_reg`), with register and DSP firmware loaded at
start (`cs47lxx_load_reg_fw`, `cs47lxx_load_dsp_fw`), state switching, a
sleep pin (`BSP_GPIO_Audio_Codec_Exit_Slp`), and SAI1 for the samples. The
13 `adsp_for_*.bin` files are DSP images and tunings for playback, voice,
and a "tfa" voice path, which suggests a separate speaker amplifier
(**guess**).

SPI2 (PB10, PC1, PC2, chip select PB12) is the only free SPI and the likely
control bus (**guess**). A device at 0x36 on I2C2 with 16-bit registers,
whose revision register 3 is read at start, fits an NXP TFA98xx speaker
amplifier (**guess**).

Steps:
1. Confirm SPI2 as the codec's bus; find the reset and sleep pins.
2. Read the codec's ID register.
3. Port the register init and the firmware loader; load
   `adsp_for_playback_48.bin`.
4. SAI1 output; play one of the `audio/*.wav` tones.
5. Microphone path.

Risk: the largest single driver in the stock app. The codec needs its
register file in the right order before it makes a sound.

### 3.7 NFC (item 9) - L, low priority

Known: NXP SN100T with secure element, NXP's `SeApi` middleware in the
stock app, a firmware image and two RF configuration files.

On I2C2 at address 0x28 (**stock code**, transfers with a 5 s timeout at
`0x08060768`).

Steps:
1. Find the enable and interrupt pins.
2. Reset the controller and read its version (NCI core reset / init).
3. Stop there unless NFC is wanted: card emulation with the secure element
   is a large protocol stack.

### 3.8 Bluetooth, remaining work (item 2) - M

Works: existing pairings, serial port, update protocol.

Steps:
1. Address from factory data. Found: `0:flash_device.txt` holds the
   address as text ("5C17CF8F8CF0" at offset 0x21, after an `A5` marker)
   next to what looks like the serial number. Read it instead of the
   `local.env` constant.
2. Pairing: handle the chip's pairing events and store new keys in
   `2:bluethooth_pair.cfg` in the stock layout, so both firmwares keep
   sharing pairings.
3. Device name and visibility as the stock app sets them.
4. BLE side (the chip reports GATT and LE groups) for a phone companion.
5. Heartbeat towards the chip (`heartbeat timeout %d` in the stock app).
6. Chip firmware update (`minidriver.bin` / `appdriver.bin` at 115200
   baud): only if ever needed.

### 3.9 GPS (items 12, 23) - M

Known: the Apollo owns the AG3335 for positions (`AG3335_Init`,
`AG3335_nmea_decode` in its image). The STM32 side handles assistance data
("EPO" update services) and can reflash the chip (`ag3335_bootloader.bin`,
`DA.bin`, `gnss_demo.bin`).

Steps:
1. Find the Apollo message that switches GPS on; decode the position kind.
2. EPO assistance data: how the stock app feeds it. Without it a fix takes
   minutes.
3. Firmware update path: leave alone.

### 3.10 eMMC, remaining work (item 1) - S

1. 4-bit bus and 48 MHz, as the stock firmware runs it.
2. Multi-block transfers.
3. Card capacity from the card instead of a constant.

Only worth it when file speed starts to matter (resources, audio).

### 3.11 PSRAM, remaining work (item 5) - S

1. A simple allocator or fixed map for the 16 MB (stock map: resources,
   animation storage, framebuffer).
2. Raise the bus clock when the system clock goes to 120 MHz.

### 3.12 Real-time clock (item 13) - S

1. Read the time; use it for log timestamps and file times.
2. Set the time over Bluetooth. The watch's clock is currently about a day
   behind, because an early Rust image reset the backup domain.

### 3.13 Log UART (item 14) - S, optional

LPUART1 on PG7 / PG8. Only useful if those pins are reachable on the board.
Would give a live log without Bluetooth.

## 4. Things that are not chips but block real use

| Topic | Why it matters | Effort |
|---|---|---|
| Low power (sleep, stop) | The Rust app runs flat out; the battery lasts hours | L |
| 120 MHz system clock | Needs our own flash-latency handling; the HAL stops at 100 MHz | S |
| Resource files `stm32_res1.bin` (279 MB), `stm32_res2.bin` | Stock fonts and images; format unknown | M |
| Robust update path | Today a broken image falls back to stock; a second slot (`packetB`) could hold a known-good Rust image | M |
| Factory data file | Address and serial are in `0:flash_device.txt`; never write to volume 0 | S |

## 5. Suggested order

1. **Buttons** (3.2): small, and gives a physical control.
2. **Drawing layer and brightness** (3.3 steps 1-2): the watch can show its
   own state.
3. **Light sensor** (3.4): first I2C device; also proves I2C2, which NFC
   and the amplifier share.
4. **Touch** (3.1): interaction.
5. **Real-time clock and Bluetooth address from factory data** (3.12, 3.8
   step 1): removes the two hard-coded values.
6. **Apollo: sending, haptics, heart rate** (3.5 steps 2-5).
7. **Low power** (section 4): after that the watch can be worn.
8. **Audio** (3.6).
9. **GPS, pairing, BLE, NFC** as wanted.

## 6. Open questions

- The touch controller's reset and interrupt pins (PF2 / PG2 are candidates).
- What PA0 signals (not a button; the stock app takes a falling-edge
  interrupt from it).
- What SPI2, ADC1 (PB0) and the outputs PA4, PB3, PB1 are used for.
- Whether the device at 0x36 on I2C2 is the speaker amplifier.
- The NFC controller's enable and interrupt pins.
- The panel's driver IC (the commands fit several 454 x 454 AMOLED drivers).
- Whether OCTOSPI2 is really used: one reference exists in the stock app,
  but no clock selection for it and no flash chip strings.
- Format of the 279 MB resource file.
