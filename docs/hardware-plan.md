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
| 1 | eMMC, ~3.6 GB, four FAT32 volumes | SDMMC1 (PC8-12, PD2) | none | works at the stock bus settings (4-bit, 48 MHz), reads and writes up to 64 sectors per command |
| 2 | Cypress CYW20721 Bluetooth | USART3 3 Mbaud (PC4, PC5, PB13, PB14), PB2, PC6, PB9, PC7 | `cypress/appdriver.bin`, `minidriver.bin` | works for existing pairings |
| 3 | Apollo co-processor | SPI1 slave (PA5-7), PG1, PA15, PF10, PG12, PF7 | `apollo/apollo_app.bin` | link works; start-up exchange done through time, name, setup status, screen status and raise-to-wake; its own log can be read (see 3.5) |
| 4 | AMOLED panel 454 x 454 | DSI + LTDC, TE on PF11, PG5, PF12, PF5, PA11 | none | works: frames in step with the panel, two frame buffers, only what changed is drawn, hardware blitter, watch face and status page, brightness, blanking by button and by wrist gesture |
| 5 | PSRAM 16 MB | FMC bank 1, ~30 pins, CRE on PE2 | none | works |
| 6 | Touch controller | I2C3 (PB4, PC0), address 0x24 | `tp/tp_firmware.bin` | partly: position and events of one finger; tap and long press recognised; drags act in all 4 of the stock app's directions (pages left and right, two panels up and down, the panels with placeholder content). covering hand blanks the display. Open: what the panels and the long-press pages hold, sleep and wake (see 3.1) |
| 7 | Audio codec Cirrus CS47Lxx with DSP | SPI (control), SAI1 (audio) | `codec/adsp_for_*.bin` (13 files) | nothing |
| 8 | Speaker and microphone | through the codec | tones in `audio/*.wav` | nothing |
| 9 | NFC controller NXP SN100T with secure element | I2C2, address 0x28 | `nfc/nfc_firmware.bin`, two `.conf` files | nothing; does not answer on the bus yet |
| 10 | Ambient light sensor STK31800 | I2C2 (PF1, PF0), address 0x45 | none | works: calibrated lux, automatic brightness as in the stock app |
| 11 | Buttons: upper on PC13, lower through the Apollo (message kind 51) | GPIO; Apollo link | none | both work; they control the display. The Apollo's key logic is read; short presses of the lower one are counted on our side |
| 12 | GPS Airoha AG3335, firmware side | shared with the Apollo | `gps/*.bin` (5 files) | nothing |
| 13 | Real-time clock, backup domain | internal, LSE crystal | none | works: read, set over Bluetooth with time zone, shown on screen |
| 14 | Log UART | LPUART1 (PG7, PG8) | none | not used |

### Attached to the Apollo

The Rust firmware does not drive these directly. It gets their data, and
would send their commands, through protobuf messages on the Apollo link.
The Apollo names them itself in message kind 52 (**measured**).

| # | Chip | Function | State in Rust |
|---|---|---|---|
| 15 | LSM6DSO (firmware also carries BMI270 and ICM-40608 drivers) | accelerometer and gyroscope | raw data not decoded; the Apollo's wrist gestures from it are used (raise-to-wake) |
| 16 | MMC5603 | magnetometer | nothing decoded |
| 17 | ICP-101xx | barometer | decoded: pressure, altitude |
| 18 | MediaTek MT6670 | heart rate and SpO2 optical front end | single heart-rate measurement works; its infrared check is one input of wear detection |
| 19 | SX932x | capacitive wear detection | decoded: worn / not worn with reason |
| 20 | BQ25618 | battery charger | partly: USB power flag |
| 21 | CW2015 | fuel gauge | decoded: percent, millivolts |
| 22 | AW8695 | haptic motor driver | pulses work; effects not done |
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

### 3.1 Touch controller (item 6) - positions done, gestures partly

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
information and returns finger events. A lift never swallows the place
reported just before it: if both are waiting, the place is handed out first.
The messages sent to the chip are byte for byte the stock app's (driver at
`0x08077C74` to `0x08079270`).

Not exercised: the path for a chip that starts in its bootloader (the
command is the stock one, but the chip never was in that state).

A finger on the glass also shades the light sensor, so the brightness
drops a little while touching.

**What the stock app does with touch** (B.70, read 2026-11-22. "Checked"
means the instructions at the address were read twice, independently;
the rest was read once and is marked "one reading". Not read: about 90
of the other screens' own touch handlers.)

Below the user interface:

- **One finger.** The driver stores up to two touches of a report
  (`0x80790E2` to `0x8079162`, checked), the touch task drops reports
  with three or more (one reading), and the routine the interface
  samples the touch with takes only the first one's position and
  "pressed or not" (`0x819C79C`, checked).
- **A covering hand** is bit 5 of the report byte that also holds the
  touch count: "Large Object detected" (`0x8079082`, checked). One such
  report switches the display off through the "cover off" path, if
  always-on mode is off (one reading: `0x8034178`, `0x8022C6E`,
  `0x8033DB0`). No count or timing in the app.
- **A touch restarts the display's switch-off timer** (one reading:
  flag 0x4000, `0x803402C`).
- **The chip sleeps with the display**: deep sleep on every display
  suspend and during NFC card reads, woken when the display comes on
  (one reading: `0x8042648`, `0x8042670`). Entering sends `05 00 01 08`
  and waits up to 500 ms for an acknowledging report, three tries. So
  the chip's wake-up report (kind 4), which would switch the display
  on, normally cannot come on this build. No tap-to-wake or
  double-tap-to-wake setting was found.
- **A report-rate check**: after a press of over half a second with far
  fewer reports than expected, the chip is reset (one reading:
  `0x8078F60`).

In the user interface (the vendor's layer over TouchGFX,
`FrontendApplication`):

- **Four directions, not eight.** Once the finger is 3 pixels from where
  it went down, the drag is locked to horizontal if it moved further
  sideways, else to vertical, and the other axis is pinned to the press
  position for the rest of the touch (`0x8047780` to `0x80477B0`,
  checked). Before the lock no drag is passed on.
- **Speed** is three times the movement between the last two samples;
  on release that becomes a horizontal or vertical swipe with that speed
  (`0x8047756` to `0x8047778` checked; release handling one reading,
  `0x80475F8`). No least speed and no ageing at this level.
- **No long press, double tap, edge or two-finger handling** at this
  level (one reading). A press time is stored and never read.

On the watch face (63 dials share one set of handlers; one reading
unless marked):

| Gesture | What it does | Threshold |
|---|---|---|
| Left / right | pages side by side: the dial, then cards. Left goes on, right goes back; at the ends the page gives 50 pixels and springs back | a third of the width moved, or speed over 40, which is 14 pixels between the last two samples (`0x80F8CE8`, `0x80F8D00`: constants checked) |
| Down | pulls a panel down from above the picture; only on the dial page | half the height moved, or speed over 30, which is over 10 pixels between samples (`0x80F9324`: constants checked) |
| Up | pulls a panel up from below; only on the dial page | the same |
| Long press | on the dial page: goes to screen 0x4B (probably choosing a dial); on a card: to screen 0x3F1 (probably editing cards) | 60 ticks pressed without a drag, so under 3 pixels of movement (`0x80F6F80`, checked). The tick rate was not determined; at the panel's frame rate that is about a second |
| Tap | a per-dial routine that does nothing on 61 of the 63 dials | released within 60 ticks without a drag |
| Double tap | none found | |
| Swipe from the edge | none found; no handler compares the press position with the edge | |

Which of the two panels is notifications and which quick settings was
not determined. Elsewhere: on the launcher a drag of 41 pixels to the
right goes back to the watch face (`0x8118852`, one reading).

**Ours against that** (`services::gestures`, 2026-11-23; the recogniser's four directions measured on the watch):

| | Stock | Ours |
|---|---|---|
| Fingers | one | one |
| Drag begins after | 3 pixels, then locked to one of 4 directions | 15 pixels, as before (kept on purpose: a wobbling finger still taps), then locked to one of 4 directions by stock's rule |
| Left / right on the face | pages | page pull: yes |
| Up / down on the face | two panels | two panels that follow the finger (measured on the watch 2026-11-23: both come in and go out under the finger, nothing torn or left behind, no blitter error or missed pulse in about 55 moves): down pulls one in from the top edge, up one from the bottom edge, each is pushed back the way it came; finished past half the height (stock's share) or with a flick over 400 pixels a second (ours). The panel lies over the face, which stays in place: our choice, how stock moves them was not read. Their content is a placeholder |
| Page change finishes at | a third of the width, or 14 pixels a sample | a third of the width, or 400 pixels a second (kept on purpose: measured 40 of 40 swipes; stock's number is stricter at any likely sample rate). Our speed is taken over the last 90 ms, not the last two samples |
| Elastic end | 50 pixels | 50 pixels (measured on the watch 2026-11-23: 20 of 20 went back, clean): to the right on the face and to the left on the status page the page follows the finger that far, black behind it, and goes back on letting go. That it follows the finger one to one is our choice; how stock eases it was not read |
| Long press | 60 ticks, opens a screen | 0.6 s (our own number); opens a page of its own on the face and on the status page, left again by a tap (measured on the watch 2026-11-23: 12 of 12 long presses opened their page, 12 of 12 taps led back, clean). Both pages are placeholders: there is one face to choose from and one page to arrange. That a tap leaves them, and that they appear without a move, are our choices |
| Tap | per dial | dots on the status page |
| Covering hand | display off | the bit is read and blanks the display (measured on the watch 2026-11-23: 9 of 9 covers blanked it, 14 taps and 11 swipes did not; a thumb pressed flat counts as a large object too); it comes back by button or wrist raise. While the display is dark, gestures are not acted on |
| Touch restarts the switch-off timer | yes | no such timer yet |
| Chip sleeps with the display | yes | never |

The recogniser's tests run on the PC (`tools/host`).

Remaining:
1. The recogniser: four directions by stock's rule, with a test for
   each direction; measured on the watch (22 of 22 swipes, 10 of 10
   taps, 5 of 5 up, 5 of 5 down). The numbers stay our
   own where they are more forgiving than stock's (decided 2026-11-23).
   Not read: the tick rate, and what stock does with a touch after a
   long press (ours: nothing more).
2. On the watch face: the up and down panels work, with placeholder
   content; open are what they hold
   (notifications, quick settings) and what the two pages a long press
   opens hold (faces to choose from, pages to arrange). The elastic end
   and the long press work.
3. The covering hand: works. The chip reports a thumb pressed flat as a
   large object as well, so that blanks the display too; whether stock
   behaves the same on the watch was not tried. The stock path,
   read 2026-11-23 (checked): the driver logs "Large Object detected"
   and sets a flag (`0x8079082`); if the display has been lit since the
   last time (a flag the backlight routine sets, `0x8040B54`), it calls
   `0x8034178`, which sends event 0x800 to the main task if the first
   word of a settings block is 0, and otherwise only while screen 0x3F4
   is shown; with the word 0 the cover acts once per lighting. The
   routine `0x8033DB0` logs "[LCD_OFF] 3.cover off" and suspends the
   display. After a large object, a new touch no longer restarts the
   switch-off timer until the display has been suspended (`0x80791D2`).
   Not read: what the settings word is (always-on display, by one
   earlier reading), what screen 0x3F4 is, and the route from event
   0x800 to the routine. Ours acts on every large object while the
   display is lit.
4. The chip's deep sleep with the display, and waking it; belongs with
   blanking and low power.
5. The stock app's settings after start (`0x0807857A`) and its "underwater
   mode".
6. Read: the other screens' touch handlers, the tick rate, which panel
   is which, and the screens 0x3EB and 0x3ED that get TouchGFX's own
   events.
7. Firmware update of the chip from `tp_firmware.bin` (only if ever needed).

### 3.2 Buttons (item 11) - done; other chips not powered down when off

**Measured:** the upper button is PC13 (pull-up, pressed reads low). The
lower button is wired to the Apollo, which reports it in message kind 51.
PA0, once suspected to be a button, is the touch controller's interrupt
line (3.1).

**How the Apollo makes its key events** (read in its firmware: edge
handler `0x40FAC`, timer callback `0x40F00`, sender `0x40CF8`; not
checked: that the image read is the version running in the watch, but the
timings in the log fit):

| Event | When |
|---|---|
| 0 released, 1 pressed | on the edges |
| 2 clicked | 300 ms after a release, from a timer, if the button was held under 800 ms and is not pressed again in that time |
| 3 double click | in place of the second release, if the second press came within 300 ms and was let go within 300 ms; no "clicked" for either press |
| 4 long press | held 1.5 s |
| 5 very long press | held 5 s |

Consequences, all seen on the watch:

- Quick presses are merged; a second press held longer than 300 ms gives
  one "clicked" and the first press counts for nothing.
- A press held between 0.8 and 1.5 s gives pressed and released only.
- The button bounces now and then. A second release report while none is
  expected stops the click timer, and the "clicked" that was due never
  comes (in the log: two releases with the same Apollo time).

So wherever every press matters, short presses are counted on our side
from the press and release reports (`Apollo::take_short_presses`): a press
ended by a release or a double click after 10 to 800 ms by the Apollo's
clock. Measured: 28 presses as fast as possible gave 28 steps, where the
Apollo sent a single "clicked".

**What the buttons do** (`app::controls`; seen working on the watch
2026-11-23, by the user's account - the log of that run was not pulled):
a table gives an action for every kind of press of each button. It can
be changed while the firmware runs (`Bindings::set`); nothing changes it
yet, and nothing stores it. This is our own design, not the stock app's.

| Button | Press | Action at start |
|---|---|---|
| Upper | click | home or sleep: a dark display is shown; a lit one goes back to the face from another page, and dark from the face |
| Upper | held 1 s | the power page |
| Upper | held 10 s | restart at once; fixed, cannot be changed |
| Lower | click | status page; from there back to the face |
| Lower | double click | brightness step |
| Lower | long (1.5 s, the Apollo's) | start or stop a heart-rate measurement |
| Lower | very long (5 s, the Apollo's) | nothing |

The upper button's presses are told apart from how long it is held
(`Hold`); it has no double click, which would make every click late. The
lower button's short presses are told apart as click and double click
here (`Taps`, 300 ms), and a click waits those 300 ms only while a
double click does something.

**The power page** (`app::power`): restart, power off, stock firmware. A
tap chooses a row, a second tap on it does it, a tap elsewhere takes the
choice back. Restart and the return to stock work.

**Power off** (two halves; the second not yet tried on the watch).

1. The Apollo. Read in the stock app: "model sys shutdown" (0x0807FAB2)
   calls 0x0805C6C6, which sends the Apollo message kind 102 with field
   1 = 3. Read in the Apollo: its dispatcher (0x3EC88) logs "shut down
   event!" and the routine at 0x54164 stops the sensors, switches supply
   pins and puts the Apollo to sleep. Measured: the Apollo acknowledges
   after 31 ms and then takes no more messages; the STM32 keeps running.
   On the charger the STM32 was restarted within 10 s instead (by what
   was not seen; the Apollo holds the STM32's reset line).
2. The STM32. Read in the stock app: after the acknowledgement a task
   runs 0x08024776: a power-down of the other chips (0x08024702, not
   read), PF3 as input with pull-down and that pull-down kept through
   PWR_PDCRF, then Shutdown mode (0x0804C9EA). No wake-up pin is enabled
   there. This firmware does the same 500 ms after the message
   (`hal::power::shutdown`), without the power-down of the other chips,
   Measured: the display stays dark, the lower button does nothing,
   the clock keeps running.
3. Switching on. Measured: on the charger the Apollo wakes and restarts
   the STM32 through its reset line (reset cause "pin"), and the watch
   comes up with the Apollo talking. Tried and dropped: the upper button
   as the STM32's wake-up pin 2. It starts the STM32 (reset cause
   "brown-out pin"), but the Apollo sleeps on, and neither the reset
   pulse on PG12 nor holding the upper button for up to 8.5 s wakes it.

Read in the Apollo, and seen on the watch by the user (a hold of the
lower button of about 3 s started it; no log read for that run): how a
button switches the watch on.
When it starts after a shutdown ("isShutDown is valid", 0x41414) and no
charger is there, the Apollo marks the STM32 as off and waits. Its key
handler (lcPowerPowerKeyParse, 0x40CF8) then answers a long press of its
own key with "Wake stm32!" and a pulse on the STM32's reset line
(0x33E24, pin 16); with the STM32 marked as on, the same handler sends
the key events of kind 51 instead, so the key is the lower button. A
press that stays short does not count, and after 10 s without a long
press the Apollo sleeps again ("Shutdown apollo !"). The Apollo has to
start up first after the press that wakes it (in our logs its first
message comes about 5 s after a reset), so the hold has to outlast
that; how long exactly has not been measured. The upper button does not
reach the Apollo as far as the code read shows.

Not known: how much current the watch draws while off without the
power-down of the other chips.

A restart comes back to this firmware only once a PC has been in touch
since the start (the bootloader is then booked to install it again);
before that it leads to the stock firmware, and the page says which.
The same goes for the upper button's 10 seconds.

Remaining:
1. Power off: try it on the watch (does the power go, how does the watch
   start again, and into which firmware).
2. Keeping the table: a settings file and a page to change it.
3. The rule that a start without a PC within 30 minutes returns to the
   stock firmware, and that a restart before a PC was seen does: both
   belong to the bring-up and stand in the way of stand-alone use. To
   be decided by the user.

### 3.3 Display (item 4) - pipeline done; one open limit

**How the stock app sends pictures** (read in B.70; its display layer is
TouchGFX, as its assertion texts show):

- Whole frames, never regions: before every frame it sends the column
  window again, always the full width (`0x08039D56`, `0x080ABE1A`).
- Each frame is started in its handler for the panel's tearing-effect
  pulse; the pulse is set to line 444 with the "tear scanline" command
  before the first frame (`0x08039D1C`).
- Two frame buffers behind each other in the PSRAM, 512 x 454 pixels each
  (464,896 bytes); before a frame the display controller is pointed at
  the other one (`0x080ABBD0`, `0x080ABC0A`).
- It reckons with up to 60 frames a second (`0x080ABDDE`).
- It draws with the chip's hardware blitter (DMA2D; its base address is
  referenced at `0x0808EE14`).

**What is built** (all measured on the watch, 2026-11-22):

- **Frames in step with the panel.** The tear scanline is set as in the
  stock app, and the tearing-effect pulse starts the frame in the DSI
  interrupt handler, as the stock app does: host off, the display
  controller pointed at the new frame, immediate reload, host on, frame
  requested. The link carries 58 frames a second.
- **Eight frames in the PSRAM**, 0x71800 bytes apart (the stock app has
  two, at that distance). Every page has frames of its own
  (`app::pages::Page::frames`): two for a page that changes while it is
  shown (the face, the status page), one for a page that always looks
  the same. A picture is drawn into the page's frame that is not shown,
  then presented; `Panel::present` returns while the frame travels. A
  page's picture stays in its frames while another page is shown.
- **Only what changed is drawn** (`services::damage`): a list of changed
  rectangles for each of the two frames (96 entries; more than that
  repaints the whole picture). A page is a set of elements, each with a
  rectangle, a key that says what it shows, and a drawing routine; an
  element is redrawn when its key changes.
- **The blitter** (`services::canvas`, through the HAL's DMA2D driver)
  fills rectangles of 320 pixels and more, copies frames, and blends
  text. Measured: filling 22 ns a pixel against the processor's 116,
  copying 49 against 245, about 30 us for one operation in all. The
  driver's operation must be asked exactly twice with the blitter's own
  flags watched in between; asking again and again restarts a transfer
  that has just finished. An operation that has not finished after 50 ms
  is aborted, and the processor draws from then on (logged).
- **Text from character bitmaps** (`services::font`, data generated by
  `tools/mkfont.py`): coverage bitmaps of one byte a pixel, DejaVu Sans
  Mono Bold in 10 x 19, 12 x 24 and 16 x 33, blended into the frame by
  the blitter, one operation a character. The status page drawn from
  scratch: 29 ms (65 ms with the library's fonts); an update of it:
  1.6 ms.
- **A watch face** (`app::face`) as the real workload: three rings for
  hours, minutes and seconds with soft edges, seven-segment time, date,
  status line, two dials and moving marks. About 5 to 6 ms of drawing a
  picture, a picture every 66 ms by choice; its first full picture takes
  about 100 ms.
- **Page changes follow the finger** (`app.rs`); how the pictures of a
  move are made is described below.

**The limit: the frame memory must be left alone while a frame travels.**
Measured on the watch, 2026-11-23:

- A frame takes 10.44 to 10.47 ms from its start to "end of refresh",
  every time (timed in the DSI interrupt). The panel's pulses are 17.1 ms
  apart, so 6.6 ms of every period are free.
- Any use of the external memory while a frame travels shifts the picture
  sideways from some line on. Shown with a white bar crossing a black
  screen, a new picture on every pulse: whole when nothing else happened
  (293 pictures in 5 s), displaced when the blitter copied one spare
  frame over the other during each frame's travel, and displaced as well
  when the processor only read a spare frame. Neither spare frame is ever
  shown.
- It is not what was suspected at one time or another: the frames in
  memory stay intact (511 pictures of page changes compared with what
  they should be, 0 pixels wrong); the panel is sent the frame that was
  just drawn and not the one before (four plain colours presented once
  each were seen in order); the display controller's underrun and
  transfer-error flags and the DSI host's two error registers stay 0;
  and the frame's travel time does not change.
- How the shift comes about inside the chip is not known. The display
  controller reads the frame through the same memory controller as
  everything else; that a late read moves the rest of the picture along
  without a flag is inferred from what it looks like.

So `Panel::settle` waits for the travelling frame before every access,
and that is a rule, not a diagnosis. What fits into the free 6.6 ms runs
at the panel's full rate.

**Moves are made by the display controller** (measured on the watch,
2026-11-23). It has two layers, each showing a rectangle of a frame
somewhere on the screen. During a move the page on the screen stays in
its frame, the page coming is drawn once into the other frame, and every
picture of the move is a piece of each (`app::motion::Motion::pieces`),
set at the frame's start in the DSI interrupt with the layers' windows
and addresses (`hal::dsi::refresh_layers`). Nothing is copied for a
picture. After a move one blitter copy makes the two frames equal again.

- Under the finger a picture comes every 17.5 ms (135 pictures in
  2365 ms), where copying gave one every 34 ms; after letting go the run
  takes 17 to 19 ms a picture.
- Preparing a move is only bringing the picture of the page coming up
  to date, in its own frame: what changed since it was last shown.
  Measured: 1 to 9 ms towards the status page, 5 to 15 ms towards the
  face (once 31 ms after 23 seconds away), 4 to 14 ms for a panel. With
  every page drawn from scratch it was 29 ms and 121 ms.
- No copy after a move, and no state of the page being left to save and
  put back.
- 43 moves of all kinds (pages, both panels in and out, the give, falling
  back): no missed pulse, no starved frame, nothing displaced.

Remaining:
3. A shared "page of elements" loop before a second real screen, and
   the ring and digit routines in a shared graphics module.
4. Scrolling lists.
5. Full power-down and power-up with the stock sequences (`0x08039464`,
   `0x080397E2`); blanking keeps the panel powered. Belongs with low power.
6. Always-on mode (`lcd_alwayson` in the stock strings): later.
7. The stage timing in the log can go once it is no longer needed.

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
| Time | stock sends kind 5: GMT seconds and time zone in minutes | done and measured (2026-11-22): the Apollo logged `Time set:UTC=1795365228 timeZone=60` and `Time set:2026-11-22 17:33:48 7 is OK!`; its clock had read 2026-11-21 12:54 before. It sets its running clock and its clock chip, stores the zone and tells its daily algorithms the time is valid (0x4010C in its image). The time zone comes from RTC backup register 3, where the stock app keeps it (`hal::rtc::time_zone`); `tools/ota.sh time` sets it. Sent again when the clock is set while running (`clock::take_changed`); that path is not tested yet |
| Version | stock sends kind 6: its version text | done and measured (2026-11-22): the Apollo keeps the text, logged `st version:W301_RUST` and answered with its own, `CONBINE_LTAQ4002_0704_202112141814` (0x3F27A in its image) |
| Setup status | stock sends kind 140 = 3 when first-time setup is over; the Apollo then switches its capacitive wear detection on (`lcAlgoOOBEStatusSet`) | sent on the announcement, repeated until acknowledged; **measured: acknowledged, capacitive sensor on, wrist detected about 4 s after putting the watch on** |
| Settings | stock then sends its settings in this order (`0x08033D38`): do-not-disturb (kind 122), sedentary reminder (125), raise-to-wake (121), heart-rate monitoring (123) with stress monitoring (135), kind 142, SpO2 monitoring (124), screen status (130) | **done and measured:** raise-to-wake (on, all day) and screen status. Do-not-disturb and sedentary are read through on both sides, written and switched off. **Not read through:** heart-rate monitoring, stress, kind 142, SpO2. None can be changed yet because the firmware has no settings of its own |
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
1. The other two ways the stock app can switch the capacitive sensor:
   kind 120 with sensor 5, and "power control" (sensor 7). The stock app
   uses them only in a timed test mode (`0x080A7290`); not read further.
2. Reason 2 (capacitive far) has not been seen on the watch yet.
3. The stock app's reactions to a wear change that need features this
   firmware does not have yet (see the table above): the 5 s timer, the
   messages to other tasks, automatic do-not-disturb.

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
1. Finish the start-up exchange above, in the stock order: do-not-disturb,
   sedentary, heart-rate monitoring, stress, kind 142, SpO2.
2. Sensor switches (kind 120).
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

### 3.10 eMMC (item 1) - done; card power-down belongs to low power

The HAL has no driver for this chip's card controller (also not in the
pinned upstream commit: its chip data lists SDMMC1 without registers), so
`hal::sdmmc` works on the registers.

Done and measured (2026-11-22), each step as the stock app does it (B.70)
and each checked at start by reading the same sectors before and after:

| Step | Stock | 1 MB read in |
|---|---|---|
| as before: 12 MHz, one data line, one sector per command | | 1049 ms |
| 48 MHz: CMD6 `0x03B90100`, clock undivided, high-speed bit | `0x0804D93A` | 443 ms |
| four data lines: CMD6 `0x03B70100`, 4-bit bus | `0x0804D9D2` | 312 ms |
| 64 sectors per command: CMD23 + CMD18 | `0x080864D6` | 64 ms |

A step that fails is taken back by restarting the card; if the card cannot
be restarted the image gives up and the watch returns to stock.

The card about itself (its extended settings, CMD8): 7,405,568 sectors
(3616 MB), settings revision 7, supported speeds 0x07; read back after the
steps above: timing 1 (high speed), bus width 1 (four lines), power notice
0. The capacity is handed to the filesystem crate.

The filesystem crate (embedded-sdmmc 0.10) reads and writes exactly one
sector per call. `services::storage` therefore:

- reads ahead: a request for the sector after the previous one fetches a
  run of 64 with one command. Measured on a 1.97 MB file: 944 ms sector by
  sector, 335 ms reading ahead.
- collects writes: consecutive sector writes (the crate writes file data
  in runs of one cluster, 64 sectors) go out with one command, CMD23 +
  CMD25 as in the stock app (`0x080865C6`). Everything collected is on the
  card before a writing function returns. Measured on a 256 KB file:
  397 ms one sector per command, 100 ms collected, read back identical.

Tried and dropped: keeping the last allocation-table sector in memory. It
made no measurable difference (386 ms against 335 ms, one run each); the
table is only read once per cluster.

Remaining, with low power (section 4):
1. Switching the card off and on around the processor's stop mode. The
   stock app's card shutdown routine (`0x0804DB62` in B.70: CMD6
   `0x03220200` "about to be switched off", controller off, supply cut
   through pins on port G) has one caller, `0x080245CA`, in the routine
   that prepares the low-power state; its start-up gives the matching
   "powered on" notice (CMD6 `0x03220100`, `0x0804D8AA`). So the notice
   is not something to send before a reset: this firmware resets with
   the card powered, and the bootloader starts the card again itself.

### 3.11 PSRAM, remaining work (item 5) - S

1. A simple allocator or fixed map for the 16 MB (stock map: resources,
   animation storage, framebuffer).
2. Bus clock: done. It is the system clock / 2 = 60 MHz, as in the stock
   app, with the stock timing values. Measured: all 16 MB written and read
   back correctly in 3.2 s.

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

The time zone is kept in backup register 3 as signed minutes, as the
stock app keeps it; `AT^W301TIME` takes an optional zone (`+0100`).

Remaining:
1. Daylight saving changes: nothing switches the zone by itself.
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
| 120 MHz system clock | **done and measured (2026-11-22):** 120 MHz with all bus dividers 1, as in the stock app; flash latency 5 and boost mode read back in the log. The embassy crates are pinned to upstream commit e1b23ae for this (see `Cargo.toml`): the 0.6.0 release sets at most latency 4 | done |
| Resource files `stm32_res1.bin` (279 MB), `stm32_res2.bin` | Stock fonts and images; format unknown | M |
| Robust update path | Today a broken image falls back to stock; a second slot (`packetB`) could hold a known-good Rust image | M |
| Factory data file | Address and serial are in `0:flash_device.txt`; never write to volume 0 | S |

### How the firmware is put together (2026-11-22)

Two tasks on one executor. The display task owns the panel, the blitter,
the touch controller and the gestures; the main task has the Apollo,
Bluetooth, the buttons, the light sensor and the log file. They exchange
values and a short queue of events through `app::shared`.

A task that is working is not interrupted by the other, so a short wait
in one can last as long as the other works in one stretch (over 100 ms
for a whole page). Two things follow from that:

- What has to happen within a fraction of a millisecond belongs in an
  interrupt handler (the frame start, see 3.3).
- A block exchange with the Apollo and a stretch of drawing do not begin
  while the other is under way (`services::turn`).

**The stack** is the 64 KB at the bottom of the RAM, so running out of it
stops the processor instead of overwriting data. It did run out once:
`Screen` held about 13 KB of tables and is handed over by value while the
firmware starts, and the function doing that reserved 54 KB. Rules since
then:

- Bulky state lives in statics, not in objects that are moved.
- `boot::stack` fills the unused stack with a pattern at start; the log
  shows the most ever used twice a minute. Measured with two tasks:
  21,184 of 65,536 bytes.
- Before an image that adds sizeable data goes to the watch, the stack
  reserved by each function of the build is listed and looked at
  (`tools/stackframes.py`).

**Checks without the watch:** `tools/check.sh` runs the tests of the
firmware's pure logic on the PC and lists the build's largest stack
frames. The tests live in `tools/host`, a small crate of its own that
uses the firmware's source files as they are; so far it covers the
gesture recogniser. It also holds a program that draws the watch face
into a picture file (`cargo run --bin preview`). Not covered by tests
yet: change tracking, and the read-ahead and write collecting of the
storage layer.

## 5. Suggested order

Done so far: buttons (3.2), display pipeline and brightness (3.3), light
sensor and automatic brightness (3.4), touch positions and the first
gestures (3.1), real-time
clock with time zone (3.12), Bluetooth address from factory data (3.8),
Apollo start-up through time, name, setup status, screen status and
raise-to-wake, wear detection, single heart-rate measurement, vibration
pulses (3.5).

Decided 2026-11-22: bring what works up to the stock firmware's level
before adding new chips.

1. **Full speed underneath:** 120 MHz system clock (section 4), PSRAM
   timing (3.11), eMMC at 48 MHz on four data lines with multi-sector
   reads and writes (3.10): done.
2. **Display pipeline** (3.3): frames in step with the panel, two frame
   buffers, change tracking, blitter, text, a watch face, page changes
   under the finger: done. Left: the limit on drawing while a frame
   travels, and the shared page loop.
3. **Finish the half-done STM32 pieces:** the rest of touch (3.1: the
   gestures not covered, sleep, settings, underwater mode), upper button
   long press (3.2), clock backup file (3.12).
4. **Low power** (section 4): after that the watch can be worn.
5. Alongside: the remaining Apollo settings (3.5).
6. Then: audio (3.6), GPS, pairing, BLE, NFC.

## 6. Open questions

- What PF4 feeds (switched together with the touch reset).
- Why drawing while a frame travels disturbs the picture, and how the
  stock app avoids or lives with it (3.3).
- Whether the Apollo image that was read for the button logic is the
  version running in the watch (3.2).
- What SPI2 and the outputs PA4 and PB3 are used for (PB1 is the audio
  codec's supply enable on board revisions 1 and 3).
- Whether the device at 0x36 on I2C2 is the speaker amplifier, and which
  pins power or enable it and the NFC controller (neither answers on the
  bus as things are).
- The panel's driver IC (the commands fit several 454 x 454 AMOLED drivers).
- Whether OCTOSPI2 is really used: one reference exists in the stock app,
  but no clock selection for it and no flash chip strings.
- Format of the 279 MB resource file.
