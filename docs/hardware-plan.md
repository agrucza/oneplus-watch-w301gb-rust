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
| 7 | Audio codec Cirrus CS47L15 with DSP (by the user's datasheet; ID register not read yet) | SPI (control), SAI1 (audio) | `codec/adsp_for_*.bin` (13 files) | nothing |
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
| I2C2 | | device with 16-bit registers: the speaker amplifier, an Awinic AW8896 by the user's datasheet (address range 0x34 to 0x37, 16-bit data; see section 7); silent in a bus scan (**measured**) | 0x36 |
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
| Touch restarts the switch-off timer | yes | done and measured: dark 15.0 s after the display came on, after a button, after the wrist gesture, and after the finger lifted (`DISPLAY_TIMEOUT`, a value of our own). A touch on the dark display does nothing |
| Chip sleeps with the display | yes | done and measured: the stock power command (`05 00 01 08` to sleep, `05 00 00 08` to wake) whenever the display goes dark or lit. Three sleeps, each confirmed at the first try within 11 to 33 ms (answer `05 00 f0 01 08`); three wakes, each answered within 1 ms (`05 00 f0 00 88`); no touch reports while asleep; touches read again after every wake. Not done: giving up the interrupt pin while the chip sleeps, as stock does |

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

**Dark display** (done; measured: 13 times off and on by button, timer and covering hand, every power-up 184 ms, none failed, no command refused, the page shown before comes back). Read in the stock
app (B.70, `0x08039B72`): to switch the display off it sets the
brightness to 0, writes 0x15 to the vendor register 0x6A on page 1,
waits 15 ms, sends "display off" and "sleep in", shuts the DSI host and
the display controller down (`LCD_DeInit`, `0x08039464`), pulls the
panel's reset low and 2 ms later cuts its power (PF5). The supplies on
PG5 and PF12 stay on. To switch it on it runs the whole start-up again,
up to several times if a step fails (`0x0803989C`), with the brightness
at 0; the backlight level follows later. With its always-on setting it
takes another path (`0x08040C34`), not read. `Panel::set_visible` does
the same; the level is set once the first picture has gone out, since
the panel's frame memory is lost with its power. Not done: stopping the
pixel clock's PLL (PLLSAI2) while the display is off.

**The look of every page: the user's "Nightwatch OS" design** (built
and drawn on the PC with `tools/host`'s preview; not yet on the watch).
The design is the base for everything the watch shows. Its parts live
in one place, `app::theme`: the colours (signal red, cyan, yellow,
green, the greys), the page title over a line, the tag with leaning
sides, the frame and the plate with two corners cut, the handle bar.
`tools/mkfont.py` makes the bitmaps from the design's three fonts
(Orbitron, Rajdhani, JetBrains Mono; fetched once, not in the
repository) and from its icon sheet; the plain DejaVu fonts are gone.
`services::font` has fonts with a width per character and drawings of
any size (`Art`), both blended by the blitter like text.

- Watch face (round kit, screen 01): sixty ticks that light up with the
  seconds and a dot going round with them, hours in red with a glow over
  minutes in white, a date tag, an arc and a value for the battery, two
  pills (pulse, messages), a line of text bent along the top and the
  bottom. The glow is a second, blurred bitmap of each digit, at about
  half the design's strength (the user found the full one too much).
- Status page: title, a line with date and time, nine rows of label and
  value with dividers. It no longer reads the clock and the Apollo's
  readings itself but is handed plain values, so the preview draws it.
- Power page: three framed rows with icon, title and note in the row's
  colour (cyan restart, red power off, yellow stock); the row tapped
  once becomes a plate in its colour with black text.
- Settings (`app::settings`; built and drawn on the PC, not yet on the
  watch): the panel pulled down from the top, the design's "quick
  access". Three settings, each of which the firmware acts on: the
  brightness (by the light, or one of the four fixed steps, as a row of
  buttons), the time after which the display goes dark (10, 15, 30 or
  60 s; was fixed at 15), and whether raising the wrist lights the
  display (a switch). What is chosen is a yellow plate, as in the
  design. A tap changes the setting at once; the main task writes the
  file. The file is `log/RUST.CFG` on the log volume, three lines of
  text ("brightness=auto", "display_off=15", "raise_to_wake=on"), read
  once at start; a missing or senseless file gives the start values.
  Raising the wrist, when switched off, is ignored by this firmware
  and, from the next image on, switched off in the Apollo too (message
  121 with "off", which is what the stock app sends when the user
  switches the feature off; built, not yet tried on the watch): the
  Apollo then sends no "raised" and no longer wakes the processor. The button action that
  steps the brightness changes the same setting.
- Quiet hours and the reminder to move (built 2026-11-24, not yet tried
  on the watch): two more switches on the settings page, each with a
  daily time (22:00 to 07:00 and 09:00 to 21:00, the stock firmware's
  start values; set on the page for setting a time, or in the file,
  lines `quiet_time=` and `move_time=`). Quiet hours are the Apollo's do-not-disturb
  (message 122): inside them it reports no raised wrist. The reminder
  is the Apollo's sedentary reminder (message 125, which it does not
  acknowledge); when it fires (message 40) this firmware vibrates
  twice, lists it on the alerts page and brings that page up, except
  in the quiet hours, where it is only listed. Both are sent once at
  start, also when off, and on every change; the Apollo keeps neither
  across its restart. What each does inside the Apollo is in
  `drivers::apollo::messages`. Not read: how often the motion
  library repeats a reminder, and whether the Apollo holds message 40
  back while it was told the screen is off.
- Launcher (`app::launcher`; built 2026-11-24; on the watch the same
  evening, two minutes in the log: fifteen swipes between face and
  launcher, all four apps opened, the upper button back to the launcher
  and on to the face, no starved frame. Seen once: the touch controller
  reported one touch as two taps 54 ms apart, which chose a tile and
  opened it at once; dealt with in the gestures since): the page beside the watch face, after the design's radial
  launcher, from which every other page is opened. The apps are tiles
  on a ring; a tap chooses one (its piece of the outer ring lights up,
  the middle names it), a tap on the chosen tile or the middle opens
  it. Four apps so far, those there are pages for: settings (the quick
  settings panel), alerts, status, power. The status page is no longer
  beside the face but one of them. The upper button leads from a page
  opened here back to the launcher, and from there to the face. The
  ring has room for eight tiles; for more it is to turn under the
  finger (worked out in the code, not switched on: turning and swiping
  back to the face have to be told apart first). Next: a settings app
  with a list that scrolls, and the design's vitals page. The launcher
  has a frame of its own; the picker now shares the frame of the two
  long-press pages.
- Settings app (`app::menu`; built 2026-11-24, not yet tried on the
  watch): what the launcher's "settings" opens. Every setting as a row
  of a list, with what it is set to; a tap switches it or takes the
  next of its choices, the rows with a daily time lead to the page for
  setting one (which goes back to where it was opened from), and the
  first row opens the quick-settings panel. The list is longer than
  the page: it follows the finger from the touch controller's own
  places, as the wheels do, runs on when let go and stops at its ends;
  a mark at the right says how far down it is. New with it: the step
  goal (6000 to 15,000, line `step_goal=` in the file), which the
  face's arc now takes. It shares the frame of the picker and the two
  long-press pages.
- Heart-rate monitoring (built 2026-11-26): a row of the settings app, "off", "10 min" (the Apollo's
  periodic kind: its green light for 30 s in every ten and a half
  minutes while worn) or "always"; line `heart_monitor=` in the file.
  Sent as message 123 with the mode, at start and on every change;
  "off" is field for field what the stock app sends at its start. With
  monitoring on the Apollo is asked for a result every 10 s while its
  light is on, which the stock app does not do (it sends "no results"
  and takes the values from the Apollo's records): read in the Apollo,
  not a path stock takes. The face's pulse pill shows the latest
  result the Apollo called valid, or the running one during a
  measurement by hand; after such a measurement the monitoring is sent
  again, because the Apollo keeps only the latest heart-rate message.
  The Apollo's own report of its lights going on and off (message 58)
  is logged, to see on the watch what the setting switches on.
  Measured 2026-11-26, worn: set to "10 min" at 53 s, the green light
  came on at 652.6 s, 600 s after the message, stayed on for 30.0 s, and
  came again 630 s later each time (1282.5 s, 1912.5 s); three results
  10 s apart in each, also while the display was dark, of which the
  last was valid in two of three measurements (100 with quality 1,
  then 70 with quality 6; none in the third, where the wear check
  dropped out meanwhile). "Always" gave a result every 10 s (63 to 77);
  "off" put the light out in the same instant. A value now counts as
  the latest only with quality 3 or more. Not measured: what the
  monitoring costs.
- Compass (`app::compass`, 2026-11-26): a fifth app in the launcher.
  While its page is on a lit screen the chip is switched on with the
  stock compass page's own message (kind 120, sensor 0, on, gap 20 ms)
  and off again with its other one when the page is left or the
  display goes dark. Measured with a first page that showed the report
  as numbers: reports come about 23 times a second; field 2 is the
  heading in degrees as a float (137, 291, 306, 335 as the watch was
  turned); field 3, the calibration's quality, did not come at all; the
  three further floats stay small and are not understood. The Apollo's
  count of the compass's users went to 1 on entering and back to 0 on
  leaving and each time the display went dark, six times over. The
  page now is a dial that turns under a fixed mark, with the heading as
  a number and its name of sixteen, and "not calibrated" while the
  quality is 0. On the watch the same day: the quality went up after
  moving the watch about; the heading jumped while the arm moved. The
  dial now follows the heading with a delay of a third of a second. A
  notice "magnet nearby" for a field outside 0.2 to 0.8 was tried and
  taken out again: it showed all the time. The three floats being about
  1.8 together had been taken for a magnet close by; they are that
  large whenever the chip counts as not calibrated (0.39, 1.91, 0.35
  right after switching on), so they are no measure of a disturbance,
  and what they are in that state is not known. The four readings a
  quarter turn apart in that day's log prove nothing: the watch was
  aimed by its own needle.
  Against a phone's compass the user found the heading to point about
  the same way (by eye, no numbers taken). Open:
  whether the heading is right against a known north, how the chip
  gets calibrated (the stock app's compass page was not read). On this
  page the display stays lit without a touch, for five minutes at most.
- One touch reported twice (seen in the launcher, 54 ms apart) now
  counts as one tap: a touch that begins within 100 ms of a tap lifting
  is that tap again (`services::gestures`).
- Setting a time (`app::picker`; built 2026-11-24, not yet tried on
  the watch): a page of its own with a wheel for the hours and one for
  the minutes, "cancel" and "save". A wheel follows the finger from where
  it comes down, and let go it runs out, slowing, to the nearest value
  or after a flick to one further on (the finger's speed taken to last
  150 ms); a tap above or below the middle row steps by one. The first
  version, tried on the watch, turned the wheel from drag gestures: it
  only took hold after the 15 pixels a gesture needs and jumped to its
  value when let go, which the user found awkward. Now the page takes
  the finger's places straight from the touch controller (built, not
  yet tried on the watch). Opened by a tap on
  the time beside a switch on the settings page: first "from", whose
  "save" leads on to "until", whose "save" stores both; "cancel" or
  leaving the page changes nothing. It is a page and not a view of the
  settings panel because a vertical drag on a panel pushes it shut. Its
  On the settings page only the switch itself switches: with the
  whole row doing it, a tap meant for the time below switched "raise
  to wake" (seen in the log). Tried on the watch (2026-11-24): the
  old three-line file read with the new settings at their start
  values, both times set through the page and stored, raise to wake
  switched off and on with the Apollo taking message 121 each time.
  The picker's
  frame is the one the two long-press pages used to have one each of:
  those two now share a frame and are drawn afresh when opened.
- Alerts (`app::alerts`): the panel from below lists the latest three
  alerts as framed cards, newest first; kept in memory only. The
  reminder to move is the one source so far.
- The two long-press pages: title, the page's icon in a frame, and how
  to leave ("faces", "pages").

Differences from the design, on purpose: its text of 9 to 12 pixels is
17 to 19 here (under a millimetre otherwise); no scan lines and no
vignette; glows only where a bitmap exists (the big digits); no status
strip along the top of every page (on a round screen that strip is a
few characters wide); the face's ticks are shorter and the bent text
sits right inside them; the bent lines say what is true of this watch.
Shown as dimmed dashes until there is data: unread messages, and the
steps before the Apollo's first summary. The steps (2026-11-24; on
the watch the first summary after a start said 7629) are the count as
of the last time the display was lit, with the arc full at 10,000.
Not built yet, for lack of anything to show in them: the design's
launcher, vitals, call, the brightness bar.
For later, the size of the image (616 KB of the 1,974 KB there are,
2026-11-24): 397 KB of it are the bitmaps, one byte a pixel, the glow of
the big digits alone 166 KB; the code is 170 KB. Not pressing. When it
is wanted: four bits a pixel, which the blitter takes as it is (about
200 KB less; not tried on this panel), and the glow worked out at start
into the PSRAM, of which 12 MB are unused (166 KB less). Large content
(more faces, pictures, animations) belongs in a file on the eMMC,
loaded into the PSRAM, as the stock app has it.

Host tests check that every element of the face draws only inside its
rectangle and that the rows of the status and power pages and the
buttons of the settings lie on the round glass; that the settings file
gives back what was written and that every button sets what it says. The image is about 625 KB (294 KB before the fonts).

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
| Settings | stock then sends its settings in this order (`0x08033D38`): do-not-disturb (kind 122), sedentary reminder (125), raise-to-wake (121), heart-rate monitoring (123) with stress monitoring (135), kind 142, SpO2 monitoring (124), screen status (130) | **done and measured:** raise-to-wake (on, all day) and screen status. **Built from the user's settings, not yet tried on the watch (2026-11-24):** raise-to-wake off, do-not-disturb, sedentary reminder with its incoming reminder (kind 40). **Not read through:** heart-rate monitoring, stress, kind 142, SpO2 |
| Board revision | Apollo asks "get pin 11" (kind 7) once after its start; stock answers with the revision it measured at start on PB0 (ADC1 channel 15: five samples, highest and lowest dropped, `0x0805C7A4`) | **measured: reading 3411 of 4095 (1.50 V), revision 3, which the Apollo calls B281_DVT.** Answered with the stock app's bytes. The Apollo's handler (`0x3F020`) takes an answer with field 4 = 2 and field 2 = 0, stores field 3 and sends nothing back, so the watch cannot show that it was taken. For revision 3 it only prints its banner; revision 1 would reconfigure two of its pins, revision 0 changes a pull on its link pin 9 |
| Battery identification | kept inside the Apollo (its stored item 0x25); together with its revision question it sends the value to the STM32 (kind 7, field 2 = 0x25, field 4 = 2; measured: value 0) and it can be set or read with kind 24 | the value is logged; nothing else done with it |
| Sensor switches | stock switches sensors with kind 120 (see "Sensor switches and what arrives unasked" below) | **not sent; read through 2026-11-24** |

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

**Sensor switches and what arrives unasked** (read in the Apollo's
image 0704 and the stock app B.70, 2026-11-24; nothing of it tried on
the watch):

- Kind 120 has five fields: 1 the sensor (0 compass, 1 barometer, 2
  battery, 3 motion, 4 GPS, 5 capacitive, 6 temperature, 7 the optical
  sensor's test switch, which was taken for "power control" before),
  2 on or off, 3 the gap between samples in ms, 4 a sub-command, 5 a
  value for it. "On" sets the gap to field 3 as it stands and counts
  one user of the sensor; "off" takes the user away again. An "on"
  without field 3 sets the gap to 0, and the encoder here leaves zero
  fields out: every "on" has to carry a gap. The gaps are kept in the
  Apollo's flash once it flushes (start values: motion 40, barometer
  600,000, compass 20, battery 8000, temperature 15,000).
- Never to be sent in passing: sensor 2 with sub-command 1 (ship mode)
  or 3 (charging off), and sensor 7 (takes the heart-rate algorithms
  down and sets them up again).
- In normal use the stock app sends kind 120 only for: the barometer
  page (gap 1000 on entering, 600,000 on leaving, never "off"), the
  compass page (on with gap 20, off on leaving), GPS, ship mode and
  the charging switch. Motion, temperature, capacitive and optical are
  only switched by its test pages.
- Battery (kind 47): sampled every 8 s; every sample is sent while the
  screen is reported on, and while it is reported off only when the
  percent or the USB flag changed (as measured). Temperatures (kind
  50): every 15 s while on; while off only outside 5 to 40 degrees.
  Barometer (kind 43): one record every 540 s through the record
  store, which holds records back while the screen is off; with the
  gap at 1000 one a second, which every "screen off" undoes. There is
  no message that asks for one fresh value: after "screen on" battery
  follows within 8 s and temperatures within 15 s.
- Kind 54 counts the users of compass, barometer, accelerometer,
  gyroscope, GPS, capacitive and optical, and is sent when a count
  changes: it shows what is really switched on.
- Steps: the Apollo's daily-activity algorithm runs from its start
  unasked, counts in RAM, and starts at zero with a new day (it needs
  the time) and with an Apollo restart. Its summary is kind 81, whose
  field 3 begins with the step count (four bytes); it is sent when
  something changed and only while the screen is reported on. All 42
  bytes of it are read (2026-11-26, both sides) and decoded in
  `messages::Day`: steps, minutes of exercise, energy, a mask of the
  hours with activity, distance in metres, floors, the day's stress,
  time in five heart-rate zones. A summary from the watch (16:18 on a
  working day): 3396 steps, 2518 m, 201 kcal, 2 floors, 12 minutes of
  exercise, 9 active hours. Kind 80 is the same per minute (19 bytes),
  which the stock app keeps in a table and hands back summed as kind 95
  so that the day's totals outlast an Apollo restart. Distance and
  energy are the motion library's and take a body of 30 years, 70 kg
  and 175 cm unless message 128 says otherwise (gender, age, weight,
  height; the Apollo keeps it in its flash; the stock app sends it only
  when the phone sets it). The Apollo sends kind 96 when a goal is
  reached. Kinds
  92 and 95 can hand a count back after a restart (not read).
  Measured 2026-11-24: 11 s after a restart of this firmware the
  summary said 7629 steps, in the evening of a worn day, so the count
  outlasts our restart, against what the reading suggested.
- Compass: kind 120 sensor 0 on gives kind 42 (a word that is probably
  the heading, an accuracy byte, the field vector); to be switched off
  on leaving. Raw motion: kind 48, test pages only.
- Not read: what the drivers do to the chips (pins, modes), GPS, the
  other fields of kinds 47, 43 and 81.

**How the Apollo detects a raised wrist** (read in its image 0704,
2026-11-24): `OPPO_API_lift_wrist_input` (0x23E88) feeds every
acceleration sample and the screen state to a detector (0x488E8) that
reads nothing else: no gyroscope, no wear state, no setting. Every
limit is a constant in its code, so there is no sensitivity to set.
While the screen is reported off it looks for a raise only (rising
stretches of the y and z axes and of two angles over the last 25
samples, two of three or the z pair needed), while it is reported on
for a lowering only; 20 samples must pass after either. So every
"raised" needs a "screen off" before it, and the 690 of the worn day
(2026-11-24) were 690 arm movements the detector took for raises. What
can keep a "raised" from being sent, all in the Apollo: the feature
switched off (message 121); do-not-disturb active (message 122, inside
its windows, outside sport); the off-wrist timeout (message 11), which
switches the feature off while the watch is off the wrist for long.
The stock app (B.70, handler 0x0806CBE0) lights the screen on every
"raised" without any filter of its own. Not read: the sample rate, and
so what 20 or 25 samples are in seconds.

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

Read in the Apollo's image 0704 and the stock app B.70 (2026-11-24;
addresses in `drivers::apollo::messages` and below):

- Kind 123, field 8 is the mode: 0 off, 1 periodic, 2 continuous, 3 one
  measurement by hand (counted: every "on" needs its "off", nothing
  ends it by itself), 4 sport (the Apollo's own), 0xFF factory test
  (sets a flag that stays; not to be sent). Field 1 switches mode 3 and
  decides for every mode whether results are reported; 7 is the seconds
  between reports; 2 and 4 are the switch and the limit of the warning
  for a high heart rate; 3, 5 and 6 are never read.
- Periodic monitoring cannot be given an interval: the Apollo lights
  the green light for 30 in every 630 s while worn, both constants
  (`lcAlgoHeartRateMonitor`, 0x31AB0), about 5 % of the time.
  Continuous keeps it on while worn. Neither looks at the screen
  status, do-not-disturb or sleep. The stock app's start values:
  monitoring off, warning limit 100; from the phone it sets mode 1 or 2
  and always field 1 = 0, so it gets no live reports while monitoring.
- Warning for a high heart rate: kind 55 from the Apollo (start time,
  lowest, highest) after 30 quiet samples at or above the limit, with
  cooldowns of 600 and 1800. No warning for a low one was found.
- Stress, kind 135 (`lcSensorStressSet`, 0x149BC): 1 on (reports), 2
  and 3 warning switch and limit, 4 seconds between reports, 5 mode
  (0 off, 1 and 2 follow the heart mode, 3 by hand, counted). Uses the
  heart rate's green light and only runs with it: by hand the stock app
  first sends kind 123 with field 1 = 1, 7 = 1, 8 = 2, then kind 135
  with 1 = 1, 4 = 1, 5 = 3, and at the end both back. Result: kind 44
  with field 8 = 1, the value (1 to 100) in 7. Warning: kind 56.
- SpO2, kind 124 (`lcSensorSpO2Set`, 0x148E0): 1 on, 2 seconds between
  reports, 3 mode (1 every 720 for 30 while asleep, 2 all through
  sleep, 3 by hand, counted). Red light. Result: kind 44 with field
  8 = 2, the value in 5 once field 25 says valid (3 = not worn, 4 =
  arm held wrong); values of the night come later with field 8 = 4 and
  their own time. An oddity in the code (0x38F12): a value of 91 or
  more with algorithm status 0 is replaced by 97 or 98 at random.
- Kind 142 hands the Apollo back today's resting heart rate after a
  restart (bpm, day number); the stock app keeps it in
  `2:temp_info.txt`. Not acknowledged.
- Kind 58 from the Apollo says when a light goes on or off (4 green
  on, 5 green off, 6 red on, 7 red off): the way to check on the watch
  what a message switched on.
- Nothing of this is kept by the Apollo across its restart.
- Not read: how the monitor's "seconds" are counted (tick count /
  1024), the Apollo's sleep state, the raw-sample fields of kind 44.

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
whose register 3 is read at start, is the speaker amplifier: an Awinic
AW8896 by the user's datasheet (section 7), not an NXP part as first
guessed. The "tfa" in a file name is then only a name.

Steps:
1. Confirm SPI2 as the codec's bus; find the reset and sleep pins.
2. Read the codec's ID register.
3. Port the register init and the firmware loader; load
   `adsp_for_playback_48.bin`.
4. SAI1 output; play one of the `audio/*.wav` tones.
5. Microphone path.

Risk: the largest single driver in the stock app. The codec needs its
register file in the right order before it makes a sound.

**First contact, measured 2026-11-26** (a probe run at
start, each chip as the stock app approaches it, on the first try):
the codec's register 0 reads 0x6370, so it is the CS47L15 and SPI2,
PB1, PB6 and the 3 MHz clock are right; the amplifier's register 0
reads 0x0310 (its register 3 reads 0x3FFF), so the fitted part is the
Awinic AW8896 and PG6 was what it lacked; the NFC controller took the
core reset (see 3.7). All three were switched off again afterwards.

**Read in the stock app B.70, 2026-11-26** (addresses in
the notes kept outside the repository):

- The codec's control bus is SPI2 (PB10, PC1, PC2, chip select PB12;
  mode 0, 7.5 MHz). A transfer is four address bytes, top bit set for
  a read, two bytes of padding, then two bytes of value (four from
  address 0x3000 up). PB6 is its reset, PB1 high takes it out of sleep
  (on board revisions 1 and 3), PA3 carries a 3 MHz clock from timer 2
  that the playback settings multiply up. No interrupt pin was found.
  Register 0 is expected to read 0x6370.
- The amplifier at 0x36 may be one of two parts, told apart at start:
  register 3 reading 0x1B96 is an NXP TFA, otherwise register 0 reading
  0x0310 an Awinic AW8896; which one is fitted also selects the set of
  voice files. PG6 high and PG10 low (its reset, active high) are the
  stock app's state from its start, and PG6 is the likely reason
  neither part answered a bus scan so far.
- Nothing is loaded into the codec at start: after the ID read the
  stock app switches audio off again and sets it up for each use. For
  playback that is one file of register writes
  (`adsp_for_playback_48.bin` or `_441`: a length, then address and
  value pairs, 102 of them), no DSP image; for voice a register file, a
  DSP file and a second register file. The codec is clock master of the
  sample lines; SAI1 (PA8, PA9, PA10, PB5) is slave, I2S, 16 bit. The
  file's routing suggests the codec's first port feeds the amplifier,
  the second is the STM32 and the third the Bluetooth chip.
- The microphone is analog on the codec's input IN1L, powered from
  the codec's own bias output; no STM32 pin switches it.
- AW8896 start for playback: 18 register writes from a table
  (0x081E3058) and two blobs written through registers 0x40 and 0x41,
  whose format is not read. The TFA's init is not read.
- Off, as the stock app leaves things between uses: amplifier muted
  and powered down, PG6 low (with the AW part), PB1 low, SPI2, SAI1 and
  timer 2 off, PG10 low, PB6 low.
- Not found: what PG6 and PA12 switch electrically; PF4 in any audio
  or NFC path.

**The stock app's playback, read in full 2026-11-26** (B.70; the
numbers, the amplifier's tables and data blocks and the codec file
decoded line by line are kept outside the repository with the other
reading notes; nothing of this is built yet):

- The stock app's ordinary tones do not go through SAI1 at all: the
  tone routine (0x0806769E) loads the 44.1 kHz codec file and streams
  the wav file to the Bluetooth chip, which feeds the codec on its
  third port. SAI1 is used by the factory's speaker test (`wav_play`,
  0x080BCACC, with `tone/spkT.wav`, 48 kHz mono) and by the playback of
  a recording. That path is the one read and the one to take here: it
  needs nothing from the Bluetooth chip.
- Codec file for playback: a software reset, the datasheet's init
  sequence, the FLL from the 3 MHz clock, the system clock from the
  FLL, three audio ports (first and second: codec is master, I2S, 16
  bit, 32 clocks a frame; third: slave), the first port's output mixed
  from the second and third ports' inputs. The codec's own analog
  outputs stay off. By the routing the first port feeds the amplifier,
  the second is the STM32, the third the Bluetooth chip (inferred, no
  schematic). The stock app writes the file without any wait, also
  after its reset, and reads no status.
- The file's FLL numbers do not give 48 kHz by the datasheet: with the
  synchroniser on they come to 96.004 MHz where 98.304 are wanted,
  2.3 % low, unless the Bluetooth chip's clock on the third port pulls
  it right. The SAI is slave, so it plays either way, possibly flat.
- Volume is the codec's: four mixer registers (0x701, 0x703, 0x709,
  0x70B; for the STM32's port 0x703 and 0x70B), 0x80 = 0 dB, 0x40 =
  -32 dB, the stock app's sixteen steps 2 dB apart. The file leaves
  them at 0 dB.
- Amplifier start: its I2S format (register 5), 18 register writes
  from a table, a check that its PLL has locked to the codec's bit
  clock, a read of its ROM version that decides between two patches
  for its processor, mute, the patch and 62 words of settings written
  into its memory through registers 0x40 and 0x41, a watchdog check,
  power up, wait for the PLL, unmute. Stop: mute, power down. There is
  no speaker calibration in this driver (that belongs to the other
  amplifier the stock app supports).
- SAI1 block A: slave transmitter, 16 bit in 2 slots of 16, frame 32,
  frame sync low and one bit early, mono mode for one-channel data;
  started last, on a buffer of silence, refilled by halves from the
  file; at the end everything is cut and powered down at once.
- The sound files: all 44.1 kHz stereo 16 bit except the two test
  files; several peak at full scale.
- Cautions: the amplifier's PLL needs the codec's bit clock for 5 ms
  before it is configured; the table's last write unmutes before the
  patch is in (stock mutes again three transfers later); the wrong
  patch for the ROM must not be written; the codec's volume has to be
  set after its file, which resets it to 0 dB.

**The amplifier's patch, looked up 2026-11-26.** The maker's Linux
driver for this chip is public (github.com/awinic-driver/aw8896, GPL 2
or later, 2017 to 2019) and does what the stock app does, value for
value: chip ID 0x0310, the patch to memory 0x8C00, the settings to
0x8380, the ROM version at 0x0F80 and 0x0F81 (0 or 0xDEAC97D6), all
through registers 0x40 and 0x41 in pieces of 128 bytes with the byte
pairs swapped, the PLL checked before and the watchdog register after.
It loads four files it does not contain: `aw8896_fw_d.bin` (ROM
version 0) or `aw8896_fw_e.bin` (0xDEAC97D6), `aw8896_cfg.bin` and
`aw8896_reg.bin`; without them it stays muted, so there is no playing
without a patch. A second public repository of the maker's
(awinic-driver/aw889x, for the same chip ID, 2018) does contain two
such files: a patch of 368 bytes and settings of 124. Compared with the
stock app's: that patch is neither of the stock app's two (308 and 412
bytes), but begins like the one for ROM version 0 and jumps to the same
six ROM addresses, so it is another revision for that ROM and of no use
on a chip of the other version; the settings have the same size and
layout and other values (another speaker's tuning). Proposed, not
yet decided by the user: no patch in this repository, but read from
files on the watch's storage, as the Linux driver reads its files.
Which patch that has to be depends on the chip's ROM version, which a probe at start
reads at start (the codec's playback file in, the amplifier clocked,
kept muted, nothing loaded, nothing played). Measured 2026-11-26: the
codec started again 1 ms after its file's reset and took all 102
settings (system clock register 0x0444, status 0x1881 = 0x0100, which
is its FLL locked, volume 0x0040); the amplifier took its format and
table and reported status 0x0311, its PLL locked to the codec's bit
clock; its ROM version reads 0xDEAC97D6, the one patch "e" is for. So
the public patch (for the other ROM) is of no use here and the one to
load is the stock app's 412 bytes. The watchdog register read 0, as it
should with nothing loaded.

**First sound (`drivers::audio`, 2026-11-26).** First try on the watch:
both files put on the watch with the new command; codec settings
taken, clock locked; amplifier ROM 0xDEAC97D6, patch (412 bytes) and
settings loaded, its watchdog register counting (0x19, 0x1F), up and
unmuted with status 0x1311; then no samples went out, because the
HAL's buffer for sending has to be filled before the port is started
and reports the port running dry as an error of the next write. The
user heard at most a very faint noise. Corrected: silence into the buffer first, then start, and a dry port
is counted and carried on from. With that the speaker played, the
same evening: the user heard the test tone (a sweep); 68,349 samples
sent in 1.5 s, the port never dry, the amplifier's status 0x1311 before
and after, everything off again afterwards; twice in a row. So the
whole chain is right as read: the codec's second port is the STM32,
its first feeds the amplifier, and the patch cut from the stock app
runs on this chip's ROM. Not judged: the pitch (the 2.3 % of the
codec file's clock settings). Agreed with the user: the amplifier's patch and settings
are not in this repository but files on the watch, read when a sound
starts: `3:log/AW8896FE.BIN` (the patch for this chip's ROM; `...FD.BIN`
would be the other) and `3:log/AW8896CF.BIN`, cut once from the user's
own copy of the stock app outside the repository and put there with
`tools/ota.sh put FILE NAME` (a new command of the update server,
`AT^W301PUT`, for files of up to 440 bytes in the log folder). A tile
"sound" in the launcher plays the factory's speaker test tone
(`packetA/audio/spkT.wav`, 48 kHz, one channel, 1.4 s) once: codec and
amplifier up in the stock app's order, the codec's playback file, the
volume at the stock speaker test's own value (0x54, 22 dB below full),
the amplifier's table, PLL, ROM version, patch and settings, watchdog,
power up, unmute, the samples through SAI1 by DMA with a little silence
around them, then everything off again. Every step is logged and the
first that fails ends it. Different from the stock app, each time to
the careful side: muted until all is loaded, the amplifier's processor
left in the path (the factory test bypasses it and plays at -1 dB), the
patch written in pieces of 64 bytes because this bus allows a transfer
10 ms. The main task does nothing else for the two seconds. The probe
of the three chips at start is gone again; what it found is above and
in 3.7.

**The watch's own sounds (2026-11-26).** On the watch the same
evening: thirteen sounds played, all six of the list, at all four
volumes; the port never ran dry and nothing failed; the alarm played
whole (1,223,424 samples, 13.9 s) and sounds were cut short on a tap
four times; the user found the sound very good. Stack 17,076 of 65,536
bytes at most. The driver is in two parts now: the main task's `Audio`,
which has the sensor bus, brings codec and amplifier up for a file
(`start`) and takes them down (`stop`); a task of its own (`player`)
sends the samples, so a sound of some seconds no longer holds the
firmware up, says when it is through and can be cut short. A file's
head is read (`services::wav`: the chunks walked to "data", so the
files with a longer "fmt " or a "bext" chunk first are right too) and
decides the rest: the codec's file for 48 or for 44.1 kHz (they differ
in the clock's numbers only), the amplifier's rate, one channel or two
on the port. Only the file's samples are sent, not what follows them.
The stock files are 44.1 kHz in two channels but for the two test
files; the alarm is 13.9 s, the ring 8.3 s. A "sounds" app in the
launcher lists six of them (alarm, call, message, low battery,
charging, test tone); a tap plays one, a tap while one plays ends it.
Volume is a setting now (line `volume=` in the file, a row in the
settings app): four steps of 6 dB, from the stock speaker test's value
(0x54) to 4 dB below full (0x78).

**Sounds for what happens (2026-11-26; built, not yet tried on the
watch).** A switch "event sounds" in the settings app (line
`event_sounds=`, off unless switched on): the charger put on plays
`connect_charging.wav`, the battery at 15 % or less plays
`low_battery.wav` (once, and again only after it was charged or back at
20 %; it is also an alert in the list, with the vibration), the
reminder to move plays `sms.wav` with its vibration. None of them in
the quiet hours, and none while another sound plays.

**The microphone (2026-11-26; read and built, not yet tried on the
watch).** Read in the stock app: its only plain recording is the
factory's microphone test (`wav_recorder`, 0x080BE440, and 0x080BE39A),
which loads nothing but the codec's settings for playing at 48 kHz
(`switch_state(1, 7)`; the earlier note here said the voice branch,
which was wrong): that file already switches the input IN1L on, analog
with +15 dB, a 40 Hz high-pass, the microphone's supply, and routes it
to both slots of the codec's second audio port. No DSP program, no
voice file, no amplifier. The voice files are for calls only: there
the microphone goes through the codec's DSP to the Bluetooth chip and
not to the STM32 at all. On the STM32: SAI1 block B, a slave receiver
that goes by block A's clock and frame, PB5 (AF13), mono mode, FIFO
threshold 3/4, DMA2 channel 2 (request 37); stock records two seconds
of 48 kHz in one channel to `0:tone/micRec.wav` and plays that back.
Built here the same way: a row "record 2 s" on the sounds page; the
codec comes up with the playing settings and without the amplifier,
block B is started and then block A (the order of the stock app's loop
test, 0x080BE81C; the HAL switches a block on when it is made, so A
cannot stay off as in the stock app's plain recording), the first
0.2 s are dropped, 96,000 samples go into the PSRAM at 0x6080_0000,
the log tells the largest and the mean sample, and then the recording
is played through the speaker.

On the watch the same evening: three recordings, 96,000 samples each,
none lost, and the voice came back from the speaker, but faint: the
largest sample was 1,076 to 1,191 of 32,768 (29 dB below full) at the
file's +15 dB. Changed after that (not yet tried): the input at +31 dB
for a recording (0x310 = 0x80BE, as the stock app's second set of
voice files has it), and a recording is played back up to 8 times as
loud, until its largest sample is 3 dB below full. The charger's sound
played six times; with the display dark it came late, because a sound
set while the Apollo's messages are read was started by the next pass
of the main loop, and that pass waited: now it does not wait.

Tried: at +31 dB two recordings had 6,390 and 10,142 as their largest
sample, so the input does not clip; played 3 and 2 times as loud at
the loudest volume they sounded a little overdriven. The aim for
playing back is now 9 dB below full, which leaves recordings like
these as they are. The charger's sound now starts 29 ms after the
Apollo's report, with the display dark too.

With that the user found the played-back recording pretty good
(2026-11-26).

### 3.7 NFC (item 9) - L, low priority

Read in the stock app B.70, 2026-11-26: PF2 is the controller's supply,
PB3 its enable, PA4 its download request, PG2 its interrupt (rising).
The factory check (0x08026C4C): PF2 high, 100 ms, PB3 low, 100 ms, PB3
high, 100 ms, then the four bytes `20 00 01 01` (core reset) written;
that the chip takes them is the check. An answer is read as three bytes
and then as many as the third says. Off: PB3 low, PF2 low.
A probe at start did exactly this and logged the answer (since
removed). Measured 2026-11-26: the controller took the four bytes and answered
`40 00 01 00` (reset accepted), then by itself `60 00 0a 02 01 20 04 05
00 a4 01 10 53`: by the NCI standard's layout a reset notification
saying NCI version 2.0, manufacturer 0x04 (NXP) and five bytes of its
own, which would be hardware 0xA4 and firmware 01.10.53 (the layout of
those five is taken from NXP's usual one, not read).

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

**Transfers are shown and need a yes (2026-11-26; built, not yet tried
on the watch).** Nothing goes over Bluetooth behind the wearer's back
any more. A firmware package has to be offered first (`AT^W301OFFER`,
sent by `tools/send.py`): the display lights with a page asking for it
to be accepted or denied; a tap, or the upper button for a yes (which
needs neither touch nor display nor the task that draws: the main
task takes it), answers within 30 s, no
answer is a no. Only within 60 s of a yes is the package's start
command taken. While a package arrives, and while a file is sent to
the PC (a log pull), the page shows what it is, how far it is, and a
button that ends it: a package's rest is then refused ("311", at which
the sender gives up), a file's sending stops. A file to the PC
and a small file from it need the same yes (`AT^W301OFFER=GET` and
`=PUT`, sent by `tools/offer.py` before a pull or a put); a yes is good
for one transfer of its kind. The display task is no
longer held while a package arrives; the Apollo and the sensors still
are. A package ended half way leaves the image file overwritten, so a
restart then leads to the stock firmware until a whole package has
gone through, as with a transfer that broke off. `AT^W301STOCK` is
taken without a yes: it leads to the maker's firmware and is the way
out of an image whose display task does not run. The charger put on or
taken off lights the display (a general wake event, with the reason in
the log).

On the watch the same evening: offers were accepted by a tap and by
the upper button, one left alone was denied after 30 s, a package was
ended by a tap at 623,948 of 681,760 bytes and the next one went
through; three log pulls after a yes; the charger put on and taken off
lit the display within 1 ms of the Apollo's report. A package of
680 KB takes 6 to 11 s with the display task running. After that: a
yes is followed at once by a state of its own, "accepted, waiting for
the PC to begin" (20 s at most, with the button that ends it), where
the face showed in between. Tried on the watch and found good by the user. An offer denied
by a tap is in the log once, from the build before the questions for
files were added.

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

**The clock loses time at every start, through the stock bootloader**
(found 2026-11-23). Measured with `tools/ota.sh clock`, which compares
the watch's clock with the PC's to a few milliseconds: no drift while
the watch runs (5 ms in 61 minutes), and 0.73 s and 0.44 s lost at two
restarts. Read in the bootloader: at every start it calls its
`HAL_RTC_Init` (`0x0800C034`, `0x0800AB28`), which enters the clock's
initialisation mode and rewrites the prescalers whether or not the clock
is running; the part of the current second that has passed is dropped.
The clock's calibration register holds 0x000081E8 from the stock
firmware (512 pulses added, 488 dropped in 2^20: about 23 ppm faster);
it is left alone. Our own resets are therefore timed
(`hal::rtc::reset_on_the_second`): made at the start of a second, four
restarts each cost 0.261 to 0.271 s, the bootloader's time to its clock
set-up, so they are now made 756 ms into the second. Measured: four
restarts cost 15 to 23 ms each; two more, between two readings, cost
151 ms together, so one of those lost more than the rest, for a reason
not found. Resets after a
fault and a start by the Apollo cannot be timed and lose up to a second.

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
| Low power (sleep, stop) | Done: display switch-off timer, touch chip sleep, panel power-down (3.1, 3.3). The processor still only idles between loop passes, at 120 MHz. The stock app goes into Stop 2 whenever its tasks are idle (FreeRTOS tickless idle, `0x080237C8` in B.70: before stopping it checks that no frame is travelling, switches off eight DMA channels and their interrupts and, under conditions, the eMMC; its modules can forbid Stop through two masks it logs as "PM Sin" and "PM Din"). The HAL's Stop-mode support does not restore a PLL clock on this chip family, so ours is to be our own. Stages: (1) baseline drain from the log's battery line: measured 2026-11-23 over 2.13 h off the charger, off the wrist, display dark (panel powered down, touch chip asleep), Bluetooth not connected: 79 % to 71 %, 4186 to 4104 mV, 4.06 % an hour by a line through the 128 one-minute readings, which is about 25 h for a full battery; (2) the wake lines as interrupts and both loops waiting for events while dark (`hal::wake`; done and measured: dark and quiet, the main loop makes 150 passes in 30 s and the display task 75, against about 1000 to 1500 and 11,000 to 21,000 lit; upper button, lower button and wrist gesture wake the display as before. The line counts: the button gives 2 changes a click; the Apollo's request line none in a quiet dark 30 s and 5 to 15 otherwise; the Bluetooth chip's line PC7 none while nothing is connected and 472 during one log pull, so it moves with the chip's traffic); (3) Stop 2 while dark with clock restore. For this the embassy pin moved from the commit of 2026-09-04 to that of 2026-11-18 (Rust 1.95 to 1.100): upstream's LPTIM time base did not build for this chip family before 2026-11-17, and since then upstream also restores an L4+ clock set-up above 80 MHz after a stop, which makes its own Stop-mode support usable here after all. The jump is 1713 upstream commits, with rewrites of the DMA ring buffers, the UART and the I2C driver, so it is tested on its own first (built, not yet tried on the watch; two calls had to follow changed interfaces: the Bluetooth UART's pin order and the ADC constructor). Tried and dropped (2026-11-23): upstream's time base on LPTIM1 from the 32 kHz crystal (`time-driver-lptim1`). It builds, but on the watch waits were missed and only ended when the timer wrapped: the log shows 29 gaps of 2 s and multiples (the 16-bit counter wraps every 2 s), 3004 display-task passes in 30 s where about 15,000 are normal, and normal pass counts only while dark, where the waits are 200 ms and more. Read in that driver: it writes the compare value and switches the compare interrupt on and off while the timer runs, and takes an alarm as armed if the target is at least one tick ahead; this chip family's timer version needs longer for a compare write to take effect, and its manual allows changes to the interrupt enables only while the timer is off (the second from memory of RM0432, not re-read). Which of the two it is was not measured. Taken instead: upstream's usual Stop-mode path (features `low-power` and `executor-thread`): a general timer as time base, the HAL's own executor, and the real-time clock keeping time across a stop. Read in that code: its RTC driver leaves a clock alone whose prescalers and hour format already match (ours do: 127 / 255, 24 h), but sets RTC_CR.BYPSHAD, which stays in the backup domain; `hal::rtc::hand_back` clears it again before a deliberate reset and before Shutdown. The HAL stops the processor whenever no peripheral it manages is busy, and it does not know the register-level drivers here, so `hal::stop` forbids Stop mode from the start. First image: all of this wired in with stopping forbidden throughout (built, not yet tried on the watch). That first image was measured: the clock lost nothing beyond a timed restart, everything else as before. Second image, to measure what stopping saves (built, not yet tried on the watch): after 10 s of dark and quiet with nobody connected the Bluetooth link is closed, because the HAL's UART receiver forbids Stop mode for as long as it exists, and the processor may stop between the main loop's passes (5 s apart, ended at once by a button, the Apollo or a lit display). Closed, our transmit line is held at its idle level and RTS says "not ready", so the chip keeps what it has to say; the chip's wake line is set to "sleep". The link opens again when the quiet ends or the chip's line PC7 moves. The HAL counts every enabled peripheral it manages (I2C, UART, the blitter) against Stop 2, so what it enters here is Stop 1; the log says which. Tried on the watch and failed for Bluetooth: after the first closing the chip sent nothing more to us, not after the link was opened again either (no chip event and no data in ten minutes, although the PC's connection was accepted by the chip; the chip's line PC7 moved once in that time, against 90 times during one log pull before). The rest worked: the HAL reported Stop 1, the link was closed and opened eight times, the upper button lit the display at once each time, the Apollo's messages came through. Why the chip fell silent is not known; the one thing it had never seen before from this firmware is its wake line PB9 going high, which was set on the strength of a function name in the stock app and not on a reading of what that line does. The closing is switched off (`STOP_WHILE_DARK`). The stock app's own way of sleeping with the Bluetooth chip, read afterwards (B.70): its Bluetooth task waits for three events (`0x0805B33C`): one wakes the link, one tries to let it sleep, one handles messages. To let it sleep (`0x080969B6`) three things must hold: the time the link was asked to stay awake has run out (6 or 24 ms after each use, `0x08096946`), nothing is being sent, and the chip's line PC7 reads low. Then PB14, our RTS, which the stock app drives as a plain output, goes high, the wake line PB9 goes high, and the task withdraws its two objections to Stop mode. To wake (`0x08096A58`) it objects to Stop mode again, sets PB14 low and PB9 low; if the link slept for 5 s or more it first restarts the UART's DMA reception with PB14 held high meanwhile (`0x08096B52`). Before it sends anything it does the same and waits 1 ms (`0x08096AA4`). The two routines named `BSP_Cypress_sleep_by_ST` and `BSP_Cypress_wake_by_ST` are factory test commands that only set PB9. So the pin levels of the failed attempt were the stock app's; what differed: the UART was taken down and built again, with its lines floating for a moment each time, where the stock app keeps it and only restarts the reception, and PC7 was not looked at before sleeping. The "wake" event is set by PC7's interrupt, which the stock app takes on the rising edge only (pin set-up at `0x0805D736`: mode 0x10110000, no pull; callback at `0x0805DD22`, which also objects to Stop mode at once). So PC7 high is the chip saying "host, be awake", and low is what the stock app waits for before it lets the link sleep. In the same set-up PB14 is a plain push-pull output, PB9 an output that starts low, PC6 (reset) an output that starts high; To find which part of the failed attempt the chip does not survive, the firmware now runs a trial by itself, once, after five seconds of dark and quiet (`services::bluetooth::Trial`): each case followed by a question the chip always answers; a chip that does not answer is switched off and on and set up as at start (`cypress::Link::power_cycle`, `bluetooth::set_up`). First run, measured: six cases (nothing; wake line alone; UART down alone; both for 2 s; both for 12 s; both for 12 s with the processor allowed to stop), woken in the stock app's order and asked at once: the chip answered after every one, its line PC7 low throughout, and a log pull worked afterwards. So neither taking the UART down nor the wake line nor a stop of 12 s is what lost the chip. Second run, measured: what else the lost image did, namely a sleep of 75 s, waking with the wake line first, and saying nothing for 20 s after waking, singly and all together, the processor allowed to stop throughout: the chip answered after all five cases, PC7 low throughout. What no case covers is the chip waking the link: in the lost image's log the one change of PC7 (at 274.969 s) is followed by the link being opened and by nothing arriving, and every attempt from the PC after it failed. Inferred, not measured: with the chip waiting to send, our "ready to receive" line stops saying "not ready" the moment the parked pin is given up, before the UART is built and its reception running, so the start of what the chip sends is lost. The stock app cannot have this: it drives that line itself and sets it to "ready" only once reception runs (`0x08096B52`). Done the same way now, and measured: PB14 is an output of ours, high until the UART is built and its reception started, low from then on, and high again before the UART is taken down. The HAL builds a transmitter and a receiver separately only by resetting the UART each time, and has no constructor with the chip's CTS line alone, so the UART is built without flow control lines and CTS is added by hand (PB13 to alternate function 7, USART_CR3.CTSE). With that the UART's own RTS, which the firmware used until now, is gone in normal operation too, as in the stock app; Measured with it: a log pull and the transfer of a whole image work as before; the five trial cases all end with the chip answering; and, by accident of the test, the PC connected twice while the link was asleep in a trial case (for 12 s and for 75 s): PC7 read high before waking both times, and after waking everything the chip had kept came in whole (connection events, the PC's `AT^BLUETOOTH`, then six data packets of 962 bytes), with the chip answering afterwards. So with the line driven by hand the chip holds what it has for a sleeping host and delivers it intact, which is the case the lost image failed in. (Those packets were read by the trial's own question and so never reached the update server, which is why the PC got no answer; the trial is not how the link is to be woken.); Put together, and measured once (the link asleep at 27.1 s; 47 s later the PC connected, the chip called, the link was awake in the same millisecond, the connection events and the PC's `AT^BLUETOOTH` arrived and were answered, and the log was pulled; the HAL reports Stop 1; the trial code is removed again): after 10 s of dark and quiet with nobody connected, and only while PC7 reads low, the link is put to sleep (PB14 high, UART down, PB9 high) and the processor may stop between the main loop's passes; PC7 high or the end of the quiet wakes the link (UART up and receiving, PB14 low, PB9 low) and what arrives goes to the update server as always. If the chip called and is still calling 5 s later with nothing received, it is switched off and on and set up again. Drain with this, measured in the night of 2026-11-23 over 8.36 h, same conditions as the baseline: the link slept once for 29,930 s (99.5 % of the time) and woke when the PC connected for the log; the battery line read 41 % from 21:56, 40 % from 03:17 (5.25 h into the run) and still 40 % at the end, 3.1 h later. One step of the gauge in about 5.3 h is about 0.19 % an hour, and the second step not having come puts it under 0.32; against 4.06 before, about a twentieth, or roughly three weeks for a full battery lying dark. The gauge counts whole percent, so this is one step and a bound, not a fitted line. Seen in the same log: while dark the Apollo sends a battery value only when the percent changes and no temperatures or pressure (the line repeated the values of the first minute all night), so the readings on the status page are old right after waking. Drain while worn, measured 2026-11-24 over a working day (11.46 h on the wrist, raise to wake on, nobody connected, with the settings build): from 86 % at 0.68 h (after the gauge had settled from the charger) to 71 % at 11.43 h, 1.40 % an hour, about three days for a full battery. In that stretch the display was lit for 0.98 h (9 %; lit 690 times by a raised wrist, 7 times by a button; blanked 538 times by the wrist going down and 159 times by the timer), the processor awake in the dark for 1.40 h (13 %; mostly the 10 s of quiet waited for after each of those blankings before the link sleeps) and asleep for 8.37 h (78 %, in 426 sleeps, all but 3 ended by the display coming on). Split by the two rates already measured (asleep 0.19, awake and dark 4.06 % an hour), the sleep took about 1.6 of the 15 %, the dark waiting about 5.7, and the lit hour the remaining 7.7, so about 8 % an hour lit; a fit over the fifteen one-percent steps cannot tell lit from awake-dark apart, the two rise and fall together. The settings worked on the watch in the same run: no file at first start (start values), nine changes by tap, each stored in about 40 ms, the brightness followed; reading the file back at a start has not been seen yet. Following from the split, the quiet waited for before the link sleeps is 2 s instead of 10 (`STOP_AFTER`; nothing else about the sleeping changed, the chip's line must still read low). Measured with it the same evening: asleep 2.1 s after the display went dark, twice; woken by the upper button after 63 s and by the PC connecting after 89 s (the chip called, the log was pulled); and the settings file was read back at that start. What it saves, measured 2026-11-25 over a working day on the wrist (11.5 h from 05:59, raise to wake on, nobody connected): 40 % to 30 %, 0.87 % an hour against 1.40 the day before, about five days for a full battery. Lit 0.83 h (7 %; 531 times by a raised wrist), awake in the dark 0.34 h (3 %, down from 1.40 h), asleep 10.46 h (90 %, 484 sleeps). By the two known rates the sleep took 2.0 of the 10 %, the dark waiting 1.3 and the lit display 6.7, again about 8 % an hour lit, so the split of the day before holds. The reminder to move was on that day and fired five times, about an hour apart while sitting (10:36, 11:36, 12:37, 15:11, 17:08): each was listed, vibrated and brought the alerts page up. No error in the log; stack 14,936 of 65,536 bytes. The same run carried on through the night on the wrist, 23.94 h in all without a restart: 40 % to 26 %, 0.58 % an hour, about a week for a full battery. Day and evening (05:59 to 23:00, 17 h): 40 to 28 %, 0.71 % an hour, 608 lightings. Quiet hours (23:00 to 05:30, worn): the Apollo reported no raised wrist at all, the display never lit, the processor slept the whole time, 28 to 26 % with steps of the gauge at 02:32 and 05:21, about 0.3 % an hour; from 05:30 the raises came again at once (14 in 25 minutes). So do-not-disturb does in the Apollo what was read. The step count went to 0 at midnight. The night before is missing: an image sent at 21:08 was not followed by PC contact, and the 30-minute net took the watch to stock, where it went from 68 % to 40 % in 8.3 h on the wrist (about 3.4 % an hour; no log of what the stock app did); (4) sleeping until the next thing is due; (5) Bluetooth across a stop; (6) eMMC, PSRAM, PLLSAI2, touch interrupt pin | L |
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
- Format of the 279 MB resource file: worked out 2026-11-26 (notes,
  an extractor and contact sheets of all pictures are kept outside the
  repository). The stock user interface is TouchGFX. `stm32_res1.bin` is
  nothing but uncompressed pixels, 2768 pictures one after the other:
  1954 in RGB565 (two bytes a pixel, low byte first) and 814 in ARGB8888
  (blue, green, red, alpha; alpha not multiplied in). Its index is the
  TouchGFX bitmap table at the end of `stm32_res2.bin`: 20 bytes a
  picture (address 0x70000000 + offset in res1, width, height, the
  opaque rectangle, and the format in the top bits of two of its
  fields), found from the file's end by the count at offset 8.
  `stm32_res2.bin` starts with a magic word that must match the stock
  app's build, a CRC-32 over everything from 0x1000 on, and the count;
  the stock app copies it whole to PSRAM 0x60180000, and it also holds
  its compiled-in fonts, all its texts and its AT command table, with
  pointers into the app, so it belongs to one build of the stock app.
  A picture is found by its number, a constant in the stock code;
  there are no names. An animation is a run of consecutive pictures of
  one size (126 such runs) that the stock code steps through; no frame
  times are stored. The fonts with all of CJK are separate files
  (`packetA/font/OPPOSans_*_4bpp.bin`, TouchGFX binary fonts, 2 bits a
  pixel despite the name). Not worked out: frame times, which number
  belongs to which screen beyond the boot and charging animations.

## 7. Datasheet check (2026-11-23)

Each chip's feature list from its datasheet, set against what this
firmware uses. Sources: **sheet** = the manufacturer's datasheet from the
user's collection (ten sheets, kept outside the repository), read for
its feature pages and the sections named here, not cover to cover;
**research** = a web research pass over a sister part's datasheet or a
public driver, not read by me; **none** = no document.

Part names checked against the firmware texts: the Apollo's image names
BQ25618, LSM6DSO, AW8695, CW2015, SX932x, MMC5603, ICP101xx and MT6670;
the stock app names only "CS47L".

### On the STM32

| Chip | Source | What the sheet settles | Not used yet |
|---|---|---|---|
| STM32L4R9 | sheet (for the L4R5: same family without DSI) | Stop 2 with RTC 2.8 uA, Standby 125 nA, Shutdown 33 nA, 5 wake-up pins, 5 us wake from Stop | Sleep and Stop modes; RTC alarm and wake-up timer; internal temperature and battery-voltage ADC channels; GFXMMU (round-display frame storage: one frame could live in internal RAM; an idea, untested); random number generator; USB (not known whether the charging contacts carry data) |
| eMMC Kingston EMMC04G-M657-K03U | sheet | the part: its sector count 7,405,568 is the one measured. The sheet gives settings revision 8, measured is 7, so the fitted card is an earlier revision of it. Write cache 512 KB, off by default (`CACHE_CTRL` 0): no flush needed as long as nobody switches it on. Standby 0.13 mA; goes to low power by itself 50 ms after idle | sleep (CMD5), power-off notification (3.10); health report; background operations; TRIM. HS200 / HS400 need 1.8 V signalling and 8 lines |
| Bluetooth CYW20721 | sheet; host protocol from research (vendor header) | Bluetooth 5.1, classic and LE, LE 2M. DEV_WAKE and HOST_WAKE lines let chip and host sleep (our PB9 and PC7 fit these; not confirmed). One I2S / PCM port, shared: A2DP audio or call voice, not both at once. PDM microphone input. Lowest mode 0.4 uA, wake by pin | pairing, visibility, name; BLE advertising, connections, GATT; notifications (ANCS), media control; hands-free and audio streaming; battery level to the phone; sleep through the wake lines. Which protocol groups the chip's firmware implements is not known |
| Audio codec CS47L15 | sheet | the user's sheet is the CS47L15; the stock app says only "CS47L". Device ID 0x6370 in register 0 settles it on the watch. One DSP (150 MIPS) with always-on voice trigger and speaker protection; up to four microphones; headphone / earpiece and speaker outputs; three audio interfaces; SPI control; sleep mode 25 uW (DSP memory is lost in it) | everything |
| Speaker amplifier AW8896 | sheet (a scan; pages 1, 2, 18 to 26 read) | replaces the earlier guess of an NXP part. I2C 0x34 to 0x37 by two pins, 8-bit register address, 16-bit data: fits the unknown device at 0x36 on I2C2. I2S input (8 to 48 kHz), own boost converter, volume 0 to -96 dB, four programmable filters, clip control, receiver mode, reset pin, interrupt pin. Software reset: 0x55AA to the ID register | everything |
| Touch controller (TrueTouch) | research (drivers) | | power command on / sleep / standby and deep sleep; easy-wake gestures (double tap, tap, slides, palm); glove, stylus, proximity modes; more than one finger, pressure, contact size; self tests and recalibration; noise report; configuration read and write. Parameter numbers for water mode are not public |
| Light sensor STK31800 | Sensortek's product table (from the user) for the family STK318x0; registers from research on the sister part STK31862 | type "ALS + Flicker": 16-bit light value, no proximity, an interrupt output, supply 1.7 to 2.0 V, 6 pins. So flicker detection is a function of this part, which fits the sister part's FIFO and its integration times down to 192 us | flicker detection (FIFO, short integration); standby (3 uA against 200 uA measuring on the sister part); window interrupt with thresholds; saturation flag; wait time. Whether the interrupt pin is wired to the STM32 is not known (PG2, an unexplained interrupt input, is a candidate) |
| PSRAM | research (smaller Micron part) | | deep power-down, partial array refresh; synchronous burst; device ID register |
| Panel | research (RM69330 and SH8601 as candidates) | our start-up uses the page-select command 0xFE, which RM69330 has; reading 0x04 / 0xA1 would settle the driver IC | sleep in, deep standby; idle mode and partial mode (always-on face); high-brightness mode; read-back of ID, power state, self-diagnosis |
| NFC SN100T | none | | everything |

### Behind the Apollo

"Not used" here means: not reported to us by the Apollo, or not decoded.
What the Apollo itself does with each chip is mostly not read.

| Chip | Source | What the sheet settles | Not used yet |
|---|---|---|---|
| Apollo3 Blue | sheet | modes Sleep, Deep Sleep, Shutdown. It has a Bluetooth LE radio of its own; whether the watch uses it is not known. It has a reset pin (RSTN); measured here, the pulse on PG12 does not wake it from its sleep, so PG12 is probably not that pin | nothing for the host to use directly |
| Charger BQ25618 | sheet (one sheet for BQ25618 and BQ25619; they differ in switch resistance) | Ship mode: the battery switch opens, at once or after 10 to 15 s (`BATFET_DLY`); the charger plugged in or the QON input pulled low closes it again. QON held low for 8 to 12 s cuts the battery for at least 250 ms: a full power cycle without any firmware (can be switched off by `BATFET_RST_EN`). Whether a button is wired to QON is not known. No ADC: voltages come from the fuel gauge | charge state (pre-charge, fast, done), fault bits (watchdog, input, thermal, timer, battery, NTC), input and charge limits |
| Fuel gauge CW2015 | sheet | percent, cell voltage, remaining run time in minutes (13 bits, updated every second), low-battery alert at 0 to 31 % (default 3 %) with an interrupt pin, sleep mode under 1 uA (it restarts its estimate on waking) | remaining run time, alert threshold, sleep |
| Haptic driver AW8695 | sheet | 8 KB waveform memory, 4 KB FIFO for streaming, playback by memory, stream, three trigger pins or continuous drive; resonance (F0) detection and tracking; coil fault check by resistance; standby 8 uA, shutdown 0.2 uA | stored effects and sequencer, streaming, resonance calibration, diagnostics; we send plain pulses |
| LSM6DSO | sheet (for the LSM6DSOX; the X adds a machine-learning core) | step detector and counter, significant motion, tilt, free fall, wake-up, 6D / 4D orientation, tap and double tap, state machine, 9 KB FIFO, temperature | all of these; only the Apollo's raise-to-wake gesture is used. Which the Apollo uses or forwards is not read |
| MediaTek MT6670 | none public | the name is the Apollo's own (driver functions `MT6670_*`, "MTK library version", MediaTek heart-rate and stress algorithms in its image). Green, red and infrared LEDs and three ambient readings by its log texts | SpO2 (red and infrared), stress, continuous monitoring, raw data |
| MMC5603 | research (vendor sheet) | | all: field (compass), temperature, self test |
| ICP-101xx | research (vendor sheet) | | the choice among its four accuracy modes (the Apollo's choice is not read); it also gives temperature |
| SX932x | research (driver) | | proximity data per phase, body / table detection, doze and sleep modes; only the Apollo's worn / not worn is used |
| GPS AG3335 | research (related module's spec) | | everything: start modes, power modes, assistance data, rates |

Sheets still missing: touch controller, light sensor STK31800, panel or
its driver IC, PSRAM, NFC SN100T, MMC5603, ICP-101xx, SX932x, AG3335,
MT6670.

### Narrowing down the parts without a sheet

Three ways, none of which needs a datasheet:

1. **What the chip says about itself** (already in our log).
   - Touch controller, system information (decoded with the field
     layout of the public cyttsp5 driver, from memory): protocol version
     1.6, firmware 1.10, configuration version 0x24, bootloader 2.0,
     silicon ID 0x0B20229B, 8 x 8 electrodes over 35.6 mm, 454 x 454.
     Its description gives vendor 0x04B4 (Cypress). Protocol 1.x means
     the older command set (PIP1), not the file-based PIP2. The silicon
     ID has not been matched to a part number.
   - Light sensor: product ID 0x82.
2. **How the stock app uses it.**
   - Light sensor: the stock settings table writes registers 0x02,
     0x04, 0x05, 0x10, 0x4E, 0x6F, 0xA1, 0xA5 and 0x60 to 0x63. Those
     are the sister part STK31862's ALSCTRL1, INTCTRL1, WAIT, FLAG,
     GAINCTRL, ALSCTRL2, PDCTRL1, INTCTRL2 and its four FIFO registers,
     so that sheet's register map applies to our part. Read with it:
     both interrupts off (0x04 = 0, 0xA5 = 0), wait 0x40 (about 100 ms
     between measurements), no short integration (0x6F = 0), FIFO
     control 2. Registers 0xDB and 0xF6 are not in that sheet.
   - Panel: the start-up switches register pages with 0xFE (pages 1
     and 5), the scheme of Raydium's RM69xxx drivers.
   - NFC: the stock app names `sn100t_i2c_write` and also a firmware
     library `libpn548ad_fw.so`; the second name belongs to an older NXP
     family, so the part number wants checking on the chip itself.
3. **Asking.** The stock app answers factory queries over the same
   Bluetooth serial port we use; its answer texts include `^LCDID:`,
   `^TPVER:`, `^GETEMMC:`, `^GETRAM:`, `^PSRAM_STATUS_GET:`,
   `^APOLLOID:`, `^GPSVER:`, `^BATID:`, `^R_SUPPLIER:` and a device
   check (`AT^DEVCHECK=all`). Not tried: the exact query for each has
   to be read first, and the same list holds commands that write or
   reset (`^FTYRESET`, `^RESET_ALL`, `^SHIPMODE`, `^W_...`, `^SET_...`),
   which must not be sent. From our own firmware the panel's ID
   registers (0x04, 0xA1) and the PSRAM's ID register could be read
   directly.
