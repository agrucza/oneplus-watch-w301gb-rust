# W301 hardware bring-up plan

Written 2026-11-21, updated as chips come up, from the firmware files in the watch dump
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
| 4 | AMOLED panel 454 x 454 | DSI + LTDC, TE on PF11, PG5, PF12, PF5, PA11 | none | works: status screen, one frame per second, brightness, blanking |
| 5 | PSRAM 16 MB | FMC bank 1, ~30 pins, CRE on PE2 | none | works |
| 6 | Touch controller | I2C3 (PB4, PC0), address 0x24 | `tp/tp_firmware.bin` | works: finger position and events |
| 7 | Audio codec Cirrus CS47Lxx with DSP | SPI (control), SAI1 (audio) | `codec/adsp_for_*.bin` (13 files) | nothing |
| 8 | Speaker and microphone | through the codec | tones in `audio/*.wav` | nothing |
| 9 | NFC controller NXP SN100T with secure element | I2C2, address 0x28 | `nfc/nfc_firmware.bin`, two `.conf` files | nothing; does not answer on the bus yet |
| 10 | Ambient light sensor STK31800 | I2C2 (PF1, PF0), address 0x45 | none | works: approximate lux, automatic brightness |
| 11 | Buttons: upper on PC13, lower through the Apollo (message kind 51) | GPIO; Apollo link | none | both work; they control the display |
| 12 | GPS Airoha AG3335, firmware side | shared with the Apollo | `gps/*.bin` (5 files) | nothing |
| 13 | Real-time clock, backup domain | internal, LSE crystal | none | works: read, set over Bluetooth, shown on screen |
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
| I2C2 | PF1 clock, PF0 data | light sensor STK31800 (**measured**) | 0x45 |
| I2C2 | | second light-sensor driver, for a part with a Lite-On style register map (alternative fitting, **guess**) | 0x53 |
| I2C2 | | NFC controller SN100T; silent in a bus scan (**measured**), so unpowered or in reset until enabled | 0x28 |
| I2C2 | | device with 16-bit registers, most likely the speaker amplifier (**guess**, see 3.6); also silent in a bus scan (**measured**) | 0x36 |
| I2C3 | PC0 clock, PB4 data | touch controller (**measured**) | 0x24 |
| I2C4 | PF14, PF15 (pull-ups) | the Apollo, for its firmware update only | 0x20 |
| SPI1 | PA5, PA6, PA7 | Apollo message link (STM32 is the slave) | |
| SPI2 | PB10 clock, PC1, PC2 data, PB12 chip select | not traced to a device; the audio codec is the likely one (**guess**) | |
| SAI1 | PA8, PA9, PA10, PB5 | audio samples | |
| LPUART1 | PG7, PG8 | log output | |
| ADC1 | PB0 (analog) | not traced | |

Other pins the stock app sets up:

| Pin | Setup | Likely role |
|---|---|---|
| PA0 | interrupt on falling edge, pull-up | touch interrupt (**measured**) |
| PF13 | output | touch reset, active low (**measured**) |
| PF4 | output | switched with the touch reset; high in use |
| PC13 | interrupt on both edges, pull-up | upper button (**measured**) |
| PF2 | output | unknown (not the touch reset) |
| PG2 | interrupt on rising edge, pull-down | unknown (not the touch interrupt) |
| PA4, PB3 | outputs, driven low at start | unknown |
| PB1 | output | unknown, set up with the ADC pin PB0 |
| PB6, PB12 and two pins in a group of 6 and 10 | outputs | unknown |

Cross-check: the stock app's clock selections name exactly USART3, LPUART1,
I2C2, I2C3, I2C4, SAI1, SDMMC1, LTDC, DSI, RTC and ADC. There is no hidden
bus beyond the table.

## 3. Plan per chip

Effort is relative: S = an evening like the PSRAM, M = like the Apollo
link, L = several sessions.

### 3.1 Touch controller (item 6) - done, refinements left

**Measured:** a Cypress/Parade TrueTouch controller on I2C3 (PC0 clock, PB4
data, 400 kHz) at address 0x24. Reset is PF13 (active low), the interrupt
line PA0 (low while a report is waiting). PF4 is switched with the reset as
in the stock app and is high while the chip is in use; what it feeds is not
known. After the reset the chip runs its touch firmware straight away (mode
byte 0xF7) and sends touch reports without further set-up.

Coordinates are display pixels, same orientation as the picture: X 0 (left)
to 453, Y 0 (top) to 453; the system information also gives 454 x 454. A
report carries the event per finger (down, move, lifted); reports come about
every 15 ms while a finger is on the glass.

`drivers::touch` resets the chip, logs its description and system
information and returns finger events; the app marks each touch with a dot.
The messages sent to the chip are byte for byte the stock app's (driver at
`0x08077C74` to `0x08079270`).

Not exercised: the path for a chip that starts in its bootloader (the
command is the stock one, but the chip never was in that state).

A finger on the glass also shades the light sensor, so the brightness
drops a little while touching.

Remaining:
1. Gestures: tap, swipe, long press.
2. Sleep and wake: the stock app has a deep-sleep command pair and a
   wake-up report (kind 4).
3. The stock app's settings after start (`0x0807857A`) and its "underwater
   mode".
4. Firmware update of the chip from `tp_firmware.bin` (only if ever needed).

### 3.2 Buttons (item 11) - done

**Measured:** the upper button is PC13 (pull-up, pressed reads low). The
lower button is wired to the Apollo, which reports it in message kind 51:
pressed, released, clicked, and long press after 1.5 s. PA0, once
suspected to be a button, is the touch controller's interrupt line (3.1).

**In use:** a click of the upper button (released within 800 ms) blanks
and shows the display; a click of the lower one steps the brightness mode.

Remaining:
1. Long-press detection for the upper button, matching what the Apollo
   does for the lower one.
2. A long hold as a "return to stock" gesture that does not depend on
   Bluetooth.

### 3.3 Display, remaining work (item 4) - M

**Works (measured):** panel on with the stock app's values, framebuffer in
the PSRAM, drawing with `embedded-graphics`, a status screen redrawn and
sent once a second, brightness (command `0x51`) and blanking (display off /
on commands) from the buttons.

Remaining:
1. Full power-down and power-up with the stock sequences (`0x08039464`,
   `0x080397E2`); blanking keeps the panel powered. Belongs with low power.
2. Partial refresh: the stock app updates regions; needed for smooth
   drawing.
3. Double buffering in the PSRAM (the stock app reserves 2 MB at
   `0x60C00000`; one frame is 412 KB).
4. Larger fonts and a real watch face.
5. Always-on mode (`lcd_alwayson` in the stock strings): later.

### 3.4 Ambient light sensor (item 10) - done, refinements left

**Measured:** STK31800 on I2C2 at address 0x45, product ID 0x82. Register 2
holds the light channel's gain (bits 5:4: x1, x4, x16, x64) and measurement
time (bits 3:0); register 0x4E the second channel's gain (bits 5:4, plus a
large step in bit 2). With the light channel at x64 and the second at x4,
and the stock lux formula (`0x08077948`), an indoor desk reads about 270
lux in the evening and 400 to 550 lux by day, steady within a few lux.

The stock driver's table of 14 register settings is in the app's compressed
initial data and was not recovered; the gains above were found by
experiment. The second channel's gain is inferred from the ratio bands of
the stock formula, and the stock calibration offsets are not applied.

The stock app also carries a driver for a second part at 0x53; this watch
has the STK.

**Range (measured with a phone torch):** the light channel rises smoothly
from about 400 counts (room) to 59,000 with the torch at the glass, without
saturating; its 16-bit limit is 65,535, so direct sun will exceed it at this
gain. The sensor reads 0 until its first measurement is finished (within
270 ms of switching on). The display's own light does not reach the sensor:
the reading is the same with the display dark and at full brightness.

**The lux formula jumps:** it multiplies by 1.36, 1.00, 0.78 or 0.63
depending on the ratio of the two channels. Where the ratio crosses a
limit the lux value steps (seen with the torch: 8301 lux at 6104 counts,
then 8136 lux at 8136 counts). On the other hand, two room readings with
quite different raw values (403 / 83 and 878 / 329) both came out at about
550 lux, so the formula does compensate for the kind of light.

**Automatic brightness** (`services::brightness`): four readings a second,
averaged over about a second, mapped by our own curve (0 lux: 16, 250 lux:
133, 5000 lux and above: 255), with hysteresis; the panel fades towards the
result. The lower button steps through automatic and four fixed levels.

Remaining:
1. Switch the gain down in very bright light, if lux above about 60,000
   is ever needed (brightness is at its maximum long before).
2. Recover the stock settings table (the app's data initialisation would
   have to be unpacked) if exact lux ever matters.

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

The controller does not answer in a scan of the bus (**measured**), so it
needs power or an enable first.

Steps:
1. Find the power, enable and interrupt pins.
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

### 3.12 Real-time clock (item 13) - done

**Measured:** the STM32's clock runs from the 32.768 kHz crystal with the
stock app's set-up (24-hour format, prescalers 127 / 255) and keeps the time
through resets and firmware changes. It holds local time: the stock app
adds the phone's time zone before setting it (`0x0802DEC0`), and keeps the
last time and time zone (in minutes) in `0:rtcData.txt`.

`hal::rtc` reads and sets the clock at register level without touching its
set-up; the HAL's driver would stop the clock briefly on every boot.
`services::clock` shows the time on the status screen, stamps files with it
and sets it from `AT^W301TIME=YYYY-MM-DD HH:MM:SS`, which `tools/ota.sh
time` sends with the PC's local time. Set and read back across a restart
on the watch.

Remaining:
1. Time zone and daylight saving changes: the clock simply holds whatever
   local time was sent.
2. Alarm and wake-up timer, when low power is done.
3. `0:rtcData.txt` is not written; the stock app restores a stale time
   from it if the backup domain is ever lost.

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

Done so far: buttons (3.2), drawing layer and brightness (3.3), light
sensor and automatic brightness (3.4), touch (3.1), real-time clock (3.12).

1. **Bluetooth address from factory data** (3.8 step 1): removes the last
   hard-coded value.
2. **Apollo: sending, haptics, heart rate** (3.5 steps 2-5).
3. **Low power** (section 4): after that the watch can be worn.
4. **Audio** (3.6).
5. **GPS, pairing, BLE, NFC** as wanted.

## 6. Open questions

- What PF4 feeds (switched together with the touch reset).
- Why the lower button's first press after a display command once came
  without a "clicked" message from the Apollo.
- What SPI2, ADC1 (PB0) and the outputs PA4, PB3, PB1 are used for.
- Whether the device at 0x36 on I2C2 is the speaker amplifier, and which
  pins power or enable it and the NFC controller (neither answers on the
  bus as things are).
- The panel's driver IC (the commands fit several 454 x 454 AMOLED drivers).
- Whether OCTOSPI2 is really used: one reference exists in the stock app,
  but no clock selection for it and no flash chip strings.
- Format of the 279 MB resource file.
