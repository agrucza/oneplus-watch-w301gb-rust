# W301 hardware bring-up plan

Written 2026-11-21, updated as chips come up. Addresses in the stock app are from the factory copy B.66 unless marked B.70; the watch runs B.70 (schema and data tables are identical in both, checked). The aim is every feature of every chip; a chip is not finished while its section lists open items, from the firmware files in the watch dump
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
| 10 | Ambient light sensor STK31800 | I2C2 (PF1, PF0), address 0x45 | none | works: calibrated lux, automatic brightness as in the stock app |
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
| PB0 | analog input, ADC1 channel 15 | board revision (**from stock code**) |
| PB1 | output | audio codec supply enable on board revisions 1 and 3 (**from stock code**, running build B.70): the stock app drives it high at start (`0x0805CA2C`), low when it puts the codec to sleep and high when it wakes it (`0x080A9B88` and the function before it, which also stop and start SPI2 and SAI1). We hold it low until there is a codec driver |
| PG6, PB6, PG10 | outputs | switched in the same codec sleep and wake functions; roles not read yet |
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

### 3.4 Ambient light sensor (item 10) - done

**Measured:** STK31800 on I2C2 at address 0x45, product ID 0x82. Register 2
holds the light channel's gain (bits 5:4: x1, x4, x16, x64) and measurement
time (bits 3:0); register 0x4E the second channel's gain.

**Settings:** the stock driver's table of 14 register settings, recovered
from the app's packed start-up data by running its own unpacking routine
(`0x081A8638`) in an emulator. It sets the light channel to x16 and the
second channel to x1; what the other twelve registers do is not known. The
sensor does not keep its settings across the switch from the stock
firmware to ours (a register dump showed reset defaults).

**Lux:** the stock formula (`0x08077948`) scales the light channel by 1.36,
1.00, 0.78 or 0.63 depending on the ratio of the two channels, then
multiplies by a calibration factor measured for each watch at the factory
(`0x080779E8`; 1000 divided by the reading under a reference light). The
factor is in the device block in internal flash at `0x08019000`, the block
that also carries the bootloader's update record, in a checksummed record
at offset 0x50; `services::factory` reads it. With it a desk in daylight
reads about 20 to 30 lux, a phone torch at the glass about 3000.

The formula jumps where the ratio crosses a limit: the same light read 18
and 32 lux alternately with the ratio at 0.23. That is the stock
behaviour.

**Not taken over:** the stock app subtracts two offsets that it derives
from what the display shows (`0x08077B58`), because the sensor sits under
the display. Measured here, the display's light made no clear difference.

**Automatic brightness** (`services::brightness`): the stock app's seven
steps (`0x0804149A`): below 11 lux 57, 11-40 80, 41-360 110, 361-1200 133,
1201-5000 172, 5001-8000 200, above 8000 215. Ours in addition: four
readings a second averaged over about a second, a 10 % margin at each
limit, and a fade between values. The lower button steps through automatic
and four fixed levels.

Remaining:
1. The display-dependent offsets, if the reading ever turns out to follow
   the picture.

### 3.5 Apollo (item 3 and items 15-24) - L, in progress

Works: transport, heartbeat, acknowledgements, sending messages, the
vibration motor's "custom pulses" command. Decoded reports: barometer,
battery, temperatures, lower button, sensor chip names, reset info, wear.

**The Apollo's firmware file** (`0:packet/apollo/apollo_app.bin`) is stored
in 98 chunks, each with a 0x60-byte header (load address at +0x58, length
at +0x5C). Reassembled it is one image at 0xC000. Its functions carry their
names as text, which makes it readable as the other half of every message.

**Start-up exchange (from both firmwares; ours is incomplete):**

| Step | Stock behaviour | Ours |
|---|---|---|
| Handshake | Apollo sends its heartbeat every second until answered (`lcMainDualComHandshake`) | done |
| Announcement | Apollo sends its sensor chip list (kind 52); stock acknowledges and starts its own messages (`0x0806CFE8`, `0x080AB4BC`) | acknowledged; used as trigger |
| Time | stock sends kind 5: GMT seconds and time zone in minutes | written, switched off: the Apollo's side is not read yet, and we would send local time where it expects GMT |
| Version | stock sends kind 6: its version text | written, switched off: the Apollo's side is not read yet |
| Setup status | stock sends kind 140 = 3 when first-time setup is over; the Apollo then switches its capacitive wear detection on (`lcAlgoOOBEStatusSet`) | sent on the announcement, repeated until acknowledged; **measured: acknowledged, capacitive sensor on, wrist detected about 4 s after putting the watch on** |
| Settings | stock then sends its settings in this order (`0x08033D38`): do-not-disturb (kind 122), sedentary reminder (125), a daily time window for the Apollo's algorithm 13, probably raise-to-wake (121), heart-rate monitoring (123) with stress monitoring (135), kind 142, SpO2 monitoring (124), kind 130 | do-not-disturb and sedentary are read through on both sides, written and switched off; **the others are identified but not read through**; none can be changed yet because the firmware has no settings of its own |
| Board revision | Apollo asks "get pin 11" (kind 7) once after its start; stock answers with the revision it measured at start on PB0 (ADC1 channel 15: five samples, highest and lowest dropped, `0x0805C7A4`) | **measured: reading 3411 of 4095 (1.50 V), revision 3, which the Apollo calls B281_DVT.** Answered with the stock app's bytes. The Apollo's handler (`0x3F020`) takes an answer with field 4 = 2 and field 2 = 0, stores field 3 and sends nothing back, so the watch cannot show that it was taken. For revision 3 it only prints its banner; revision 1 would reconfigure two of its pins, revision 0 changes a pull on its link pin 9 |
| Battery identification | kept inside the Apollo (its stored item 0x25); together with its revision question it sends the value to the STM32 (kind 7, field 2 = 0x25, field 4 = 2; measured: value 0) and it can be set or read with kind 24 | the value is logged; nothing else done with it |
| Sensor switches | stock switches sensors with kind 120 (field 1: 0 compass, 1 barometer, 2 battery, 3 motion, 4 GPS, 5 capacitive, 6 temperature, 7 power control; field 2 on/off; field 3 interval) | **not done** |

**Wear detection** (read through in the Apollo's `lcAlgoWearWristCheckExe`
and around it; the Apollo's own log texts name the inputs):

- Inputs: the capacitive sensor ("cap near" / "cap far") and the optical
  sensor's infrared check ("ir near" / "ir far").
- Put on: both near, each for at least one second. Report: on the wrist,
  reason 4.
- Taken off: infrared far (reason 1), or capacitive far confirmed by a
  second check (reason 2; the report then carries three status bytes of
  the capacitive sensor in field 4).
- Inside the Apollo the state is 8 (worn) or 3 (not worn); these are the
  only two values its code sets. The report (kind 45) carries 2 or 1 in
  field 2, the reason in field 3. It is sent on a change that lasted half
  a second, and once after start (reason 0).
- Enabled by: the setup status (kind 140 = 3), which switches the
  capacitive sensor on. The Apollo's status message (kind 54: compass,
  barometer, motion, gyroscope, GPS, capacitive, optical) shows the switch.
- Off-wrist timeout: kind 11 from the STM32 (field 1 = 0, field 2 =
  minutes, 0 = off) sets it; the Apollo answers with kind 11 (field 1 = 1,
  field 2 = reached or not) once the watch has been off the wrist that
  long. It only counts while it has been told that the screen is off.
- What depends on the wear state in the Apollo: the heart-rate sensor
  (`lcAlgoHeartRateEnable`), the sedentary reminder ("offbody, sedentary
  algo off"), heart-rate result sending and reminders.

Done and measured: setup status, detection of putting on (about 4 s) and
taking off, full decoding of the report (`messages::Wear`), screen status.

**The infrared check** (the Apollo's "OPPO_Wrist_algo", 0x3B23C in its
image): the infrared reflection is averaged over a number of samples and
compared with a fixed limit, 20001; at or above it is "near". Measured:
while the wearer moves the arm, the report flips to "not on the wrist:
infrared sees no skin" and back within a few seconds; while sitting still
it is stable. More exactly (measured over some 25 cycles): 1 to 5 s after
the arm is let hang the report goes to "not on the wrist", and about 1.5 s
(once 4.5 s) after the arm is raised it returns. This happens with the
display on as well as off, so it is the arm's posture and not the screen
status; nothing in the code ties the check to the screen status either.
The Apollo's own log shows the numbers (2026-11-22): its wear check
samples the infrared reflection about six times a second at LED level 10,
which puts the limit at 200,000. Arm raised: 350,000 to 550,000. Arm
hanging: the reading slides down over 5 to 8 s to 120,000 to 165,000 and
crosses the limit ("ir far"). The capacitive sensor says "near" throughout;
infrared alone decides.

**The stock firmware behaves the same.** Measured the same day with the
stock firmware (B.70) running and the Apollo log recorded by stock
(`AT^LOGSETPARA=APOLLO=0x1801CF04`, file `3:log/apollo/log_ap*.log`): same
LED level, same slide, six "ir far" in 160 s of raising and lowering the
arm. So this is not caused by anything this firmware sends or leaves out.
The stock log's old counters (14 wear changes for 7,000 raises in 2022)
were written under other wearing conditions.

**What the stock app does with a wear report** (handler 0x0806CC70,
B.66 addresses; read, not all of it followed to the end):

| Step | Stock | Here |
|---|---|---|
| Count "put on" and "taken off" (0x08033566) | `wear_cts`, `unwear_cts` in its log | done (`link.rs`, minute summary) |
| Add up the worn time (0x08034816) | yes | done |
| Not worn and screen off: 5 s one-shot timer, then event 0x10 to a service task (0x08033552) | yes; the task's reaction is not read (the nearby text is `service_screen_change_lock_create`, so probably the screen lock) | open, needs the lock first |
| Tell other tasks (message 0x58, 0x08052C74; sport module 0x080A71E0) | yes | open, no such features yet |
| Automatic do-not-disturb: not worn and a setting set starts a 10-minute timer (30 with an argument), worn cancels it (0x08034644, 0x08034724) | yes | open, no do-not-disturb yet |
| Hide short drops from the user | it never shows the wear state | `app::WearView`, 3 s hold |

No place was found where the stock app (B.70) sends the off-wrist timeout
(kind 11). Checked: a search of the whole image for code that loads the
constant 11 and stores it as a 16-bit value (eight hits, none of them a
message to the Apollo), and the kinds set at all 52 direct calls of the
two send routines. Not ruled out: a sender that gets its kind from a table
or a parameter. The stock app does receive the Apollo's off-wrist
notification (text `offWrist=%d`). The encoder here exists and stays
unused.

In the Apollo (read in its image): an arriving kind 11 with field 1 = 0 is
logged as `argOffWrist set <minutes>` (0x10CF4), then stored by its main
event loop (0x1554E; the minutes end up at RAM 0x10030370) and
acknowledged. That store is the only write to the value found; its start
value is 0. `lcAlgoOnOffControl` uses it (0x372B8): with 0 minutes the
"off the wrist for long" check never fires; otherwise it fires once the
time since the watch left the wrist reaches minutes x 60 s, and the
notification (kind 11, field 1 = 1) is only sent while the minutes are not
0 (0x37310). So with the start value the Apollo never sends the
notification. Not ruled out: the value being restored from the Apollo's
own flash by a block copy.

The stock app's table of handlers for incoming messages is at 0x081B67D8
(B.66), one pointer per kind. Its entry for kind 11 is 0x0806D466, and
that routine only writes `offWrist=%d` to the log and returns. So the
stock app does nothing with the notification; the feature is unused there.

Both stock send routines (B.66: 0x0806C35C, 0x0806C40A) start with a gate:
while the byte at RAM 0x2009152C is not 0, only the kinds 3, 6 to 8, 20,
47, 50 to 52, 101, 102 and 130 are sent and every other message is
dropped. The byte is an "update running" flag (writers read in B.66): set
to 1 when the update task starts unpacking an update package (0x08045356)
and when it starts updating the touch controller's firmware (0x08046722);
cleared when either ends, in success or failure (0x08045494, 0x0804676C,
0x080467C0, 0x080467E8), and at start (0x0804380E). In normal operation it
is 0 and every message goes out.

The Apollo log can be had under this firmware too: message kind 100
(`messages::log_setting`), text arrives in blocks starting with 0x7F
(`link.rs`, `APOLLO_LOG_WINDOW`). `app::WearView` can hold a "not worn"
back on the status screen for 3 s. The log window is off (`None`) in the
normal image. Whether
the stock firmware sees the same cannot be checked (it does not log wear
changes).

**Screen status and wrist gestures** (measured): the Apollo starts with
"screen off". Told "off", it sends almost nothing but wear changes; told
"on", it releases its held-back records and the live reports return.
Its wrist-raise algorithm runs on every motion sample with the screen
state: "lowered" (kind 41, value 2) is always reported, "raised" (value 1)
only while its algorithm 13 is on, which the raise-to-wake setting (kind
121: on, daily window, start state off) switches. The firmware now sends
that setting and shows the display on "raised"; on "lowered" it blanks
the display only if a raise had shown it (the stock app's own condition
is a setting not read yet).

Open:
1. Screen status (kind 130: field 2 = 1 on, 0 off; the stock app sends
   it when the backlight is set, when the screen goes off, and once after
   the Apollo's start). The Apollo starts with "off". It holds its cached
   health data back while the screen is off, makes its sport module
   report on "on", feeds the state to its motion algorithm and daily
   module, and only counts the off-wrist timeout while off. Written,
   switched off; next single test. The off-wrist timeout comes after it.
2. The other two ways the stock app can switch the capacitive sensor:
   kind 120 with sensor 5, and "power control" (sensor 7). The stock app
   uses them only in a timed test mode (`0x080A7290`); not read further.
3. What the stock app does on a wear change besides logging it (its
   handler at `0x0806CC70` calls six functions): not read.
4. Reason 2 has not been seen on the watch yet.

**Heart rate:** command kind 123 (field 8 = mode: 3 single measurement, 2
with the stress measurement, 0xFF test), results kind 44 (field 2 = beats
per minute, 5 SpO2, 7 stress, 8 type, 25 status). The Apollo's
`lcAlgoHeartRateEnable` does nothing unless its wear state is "on the
wrist", so this depends on wear detection.

Measured: a long press of the lower button starts a measurement, the
green light comes on, and one report a second arrives. No value for the
first 21 s, then early values (106 to 122 with field 4 = 1), settling at 61
to 69 with field 4 = 4 to 6; field 4 looks like the confidence of the
value. Not done: SpO2, stress, automatic monitoring, the limits and
warnings, the other fields of the report.

**Vibration motor:** command kind 84. Works: type 2, pulses with length,
pause and count. Not done: type 1 and type 3 (the Apollo's built-in effects
by number), stop, the stock app's table of about 30 effects
(`0x081C54C0`), motor calibration.

Steps:
1. Finish the start-up exchange above, in the stock order.
2. Battery identification with the ADC.
3. Name the rest of the schema (about 80 kinds; see
   `src/drivers/apollo/messages.rs` for those known).
4. Motion data (LSM6DSO), compass (MMC5603): switches, rates, reports.
5. Heart rate, SpO2, stress: all modes, automatic monitoring, limits.
6. Vibration: all types and the effect table.
7. GPS position data: see 3.9.
8. Battery and charger: all fields of kind 47, charge control.
9. Records (kinds 80-96): daily activity, sleep, sport.
10. Sleep: the stock app tells the Apollo when the STM32 sleeps.
11. Apollo firmware update over I2C4 (PF14, PF15, address 0x20), from the
    stock task at `0x08079A64`.

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
1. Address from factory data: done. `0:flash_device.txt` (1024 bytes,
   checksummed) holds a record at 0x28 with the address as twelve hex
   digits; `services::factory` reads and checks it as the stock app does
   (`0x08031ED0`). The address is no longer built into the image.
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
sensor and automatic brightness (3.4), touch (3.1), real-time clock (3.12), Bluetooth address from factory data (3.8).

1. **Apollo: sending, haptics, heart rate** (3.5 steps 2-5).
2. **Low power** (section 4): after that the watch can be worn.
3. **Audio** (3.6).
4. **GPS, pairing, BLE, NFC** as wanted.

## 6. Open questions

- What PF4 feeds (switched together with the touch reset).
- Why the lower button's first press after a display command once came
  without a "clicked" message from the Apollo.
- What SPI2 and the outputs PA4, PB3, PB1 are used for.
- Whether the device at 0x36 on I2C2 is the speaker amplifier, and which
  pins power or enable it and the NFC controller (neither answers on the
  bus as things are).
- The panel's driver IC (the commands fit several 454 x 454 AMOLED drivers).
- Whether OCTOSPI2 is really used: one reference exists in the stock app,
  but no clock selection for it and no flash chip strings.
- Format of the 279 MB resource file.
