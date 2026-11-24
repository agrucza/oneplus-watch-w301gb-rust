//! What the Apollo's messages mean, as far as it is known.
//!
//! The wire format is protobuf. The stock app holds the full schema as
//! nanopb field descriptors (top level at 0x081AC324: field 1 = sequence
//! number, then one sub-message out of 79 kinds) and a handler per kind
//! (table at 0x081B67D8). Names below come from the log texts in those
//! handlers and from values captured on the watch:
//!
//! |  kind | meaning                         | fields                                        |
//! |------:|---------------------------------|-----------------------------------------------|
//! |     3 | acknowledgement                 | 1 sequence, 2 kind                            |
//! |     4 | 200-byte data block (dump?)     | 1, 2 counters, 4 bytes                        |
//! |     7 | not known                       | 1..4 numbers                                  |
//! |    20 | heartbeat                       | 2 = 0xA5A5A5A5 from the Apollo                |
//! |    40 | reminder to move                | 1 time, 2 = 1; see `sedentary`                |
//! |    41 | wrist gesture (raise to wake)   | see `WristGesture`                            |
//! |    43 | barometer ("BAR")               | 1 time ms, 2 pressure Pa, 3 altitude m        |
//! |    44 | heart rate, SpO2, stress        | 1 time ms, 2 bpm, 5 SpO2, 7 stress, 8 type, 25 status |
//! |    11 | off-wrist timeout and notification | see `off_wrist_timeout`                    |
//! |    45 | wear detection                  | see `Wear`                                    |
//! |    47 | battery ("Bat Receive")         | 1 time ms, 2 usb, 3 percent, 5 millivolts     |
//! |    50 | temperatures ("NTC msg")        | 1 time ms, 2..4 degrees Celsius               |
//! |    51 | lower button                    | 2 event (see `KeyEvent`), 3 Apollo uptime ms  |
//! |    52 | sensor chip names               | 21..29 strings                                |
//! |    54 | sensor status (unconfirmed)     | 1 time ms, 3, 4, 8 numbers                    |
//! |    59 | Apollo's own reset info         | 1 shutdown type, 2 reset reason               |
//! |    80 | 180-byte record, type 19        | 1 time s, 2 type, 3 bytes                     |
//! |    84 | vibration motor (to the Apollo) | see `motor_pulses`                            |
//! |   102 | system state (to the Apollo)    | see `power_off`                               |
//! |   121 | raise to wake on/off (to the Apollo) | see `raise_to_wake`                      |
//! |   122 | do-not-disturb (to the Apollo)  | see `do_not_disturb`                          |
//! |   125 | sedentary reminder (to the Apollo) | see `sedentary`                            |
//! |   123 | heart rate on/off (to the Apollo)| see `heart_rate`                              |
//! |   130 | screen status (to the Apollo)   | see `screen_status`                           |
//! |   140 | setup status (to the Apollo)    | see `setup_finished`                          |
//! |    81 | the day's activity so far      | 1 time s, 2 = 42, 3 bytes; see `Readings::steps` |
//!
//! Times are the watch's real-time clock as Unix time.

pub const ACK: u32 = 3;
pub const HEARTBEAT: u32 = 20;
/// A 200-byte binary block (field 4) with three small numbers (fields 1
/// to 3), sent in pairs after the Apollo's start. Not its log text, which
/// travels in blocks starting with 0x7F (see `log_setting`). What it holds
/// has not been read.
pub const DATA_BLOCK: u32 = 4;
pub const TIME: u32 = 5;
pub const OFF_WRIST: u32 = 11;
pub const PIN: u32 = 7;
pub const VERSION: u32 = 6;
pub const SENSOR_NAMES: u32 = 52;
pub const MOVE_REMINDER: u32 = 40;
pub const DAILY: u32 = 81;
pub const WRIST_GESTURE: u32 = 41;
pub const BAROMETER: u32 = 43;
pub const WEAR: u32 = 45;
pub const BATTERY: u32 = 47;
pub const TEMPERATURES: u32 = 50;
pub const KEY: u32 = 51;
pub const RESET_INFO: u32 = 59;
pub const MOTOR: u32 = 84;
pub const HEART: u32 = 44;
pub const HEART_RATE_SET: u32 = 123;
pub const SETUP_STATUS: u32 = 140;
pub const SCREEN: u32 = 130;
pub const RAISE_TO_WAKE: u32 = 121;
pub const DO_NOT_DISTURB: u32 = 122;
pub const LOG_SETTING: u32 = 100;
pub const SYSTEM: u32 = 102;
pub const SEDENTARY: u32 = 125;

/// Builds a sub-message field by field. Fields with the value zero are
/// left out, as the encoding has it.
pub struct Encoder {
    bytes: [u8; 64],
    len: usize,
}

impl Encoder {
    pub fn new() -> Self {
        Self { bytes: [0; 64], len: 0 }
    }

    /// Add a number field (field numbers up to 15).
    pub fn number(mut self, field: u8, mut value: u32) -> Self {
        if value == 0 || self.len + 6 > self.bytes.len() {
            return self;
        }
        self.bytes[self.len] = field << 3;
        self.len += 1;
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            self.bytes[self.len] = if value == 0 { byte } else { byte | 0x80 };
            self.len += 1;
            if value == 0 {
                return self;
            }
        }
    }

    /// Add a signed number field (field numbers up to 15). A negative
    /// value takes ten bytes, as the encoding has it for this field type.
    pub fn signed(mut self, field: u8, value: i32) -> Self {
        if value >= 0 {
            return self.number(field, value as u32);
        }
        if self.len + 11 > self.bytes.len() {
            return self;
        }
        let mut bits = value as i64 as u64;
        self.bytes[self.len] = field << 3;
        self.len += 1;
        for i in 0..10 {
            let byte = (bits & 0x7F) as u8;
            bits >>= 7;
            self.bytes[self.len] = if i == 9 { byte } else { byte | 0x80 };
            self.len += 1;
        }
        self
    }

    /// Add a text field (field numbers up to 15); longer text is cut.
    pub fn text(mut self, field: u8, text: &str) -> Self {
        let room = self.bytes.len().saturating_sub(self.len + 2);
        let n = text.len().min(room).min(127);
        if n == 0 {
            return self;
        }
        self.bytes[self.len] = field << 3 | 2;
        self.bytes[self.len + 1] = n as u8;
        self.bytes[self.len + 2..self.len + 2 + n].copy_from_slice(&text.as_bytes()[..n]);
        self.len += 2 + n;
        self
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

// What the stock app sends when the Apollo has started. Its trigger is the
// Apollo's list of sensor chips (kind 52): the handler at 0x0806CFE8
// acknowledges it and calls 0x080AB4BC, which sends the time (kind 5), the
// STM32 firmware's version text (kind 6) and then the settings, beginning
// with the setup status (kind 140, see `setup_finished`).

/// The time: field 2 = seconds since 1970 in GMT, field 3 = time zone in
/// minutes, signed (stock sender 0x0806D594).
///
/// In the Apollo (main task, 0x4010C in its image): local time = GMT +
/// zone x 60. Its running clock is moved to that, its clock chip is set to
/// it, the zone is stored (also in its flash, item 0x21) and its daily
/// algorithms are told that the time is valid. It logs `Time set:UTC=...
/// timeZone=...` and the resulting date and time.
pub fn time(gmt_seconds: u32, zone_minutes: i16) -> Encoder {
    Encoder::new().number(2, gmt_seconds).signed(3, zone_minutes as i32)
}

/// Answer to the Apollo's question for the board revision.
///
/// After its start the Apollo sends kind 7 with field 1 = 11 and field
/// 4 = 1 ("get", from its main task near 0x3FDB8), once. The stock app answers with kind 7, field 3 = the revision
/// and field 4 = 2 (0x0805C654).
///
/// What the Apollo does with the answer (0x3F08E to 0x3F1C4, 0xFDE6): it
/// names the revision in its log (0 B280_DVT, 1 B280_PVT, 2 B281_EVT,
/// anything else B281_DVT) and stores it; its start value is -1. For
/// every value but 2 it prints its start-up banner; for 1 it also
/// reconfigures two of its pins (45, 47). The only other reader is the
/// link's pin set-up (0x54248), which gives its pin 9 a pull for revision
/// 0 only. It accepts the answer only with field 2 = 0 (0x3F036) and
/// sends nothing back. Together with its question it sends another kind
/// 7 (field 2 = 0x25, field 4 = 2): its stored battery identification.
pub fn revision_answer(revision: u8) -> Encoder {
    Encoder::new().number(3, revision as u32).number(4, 2)
}

/// True if a kind 7 message is the Apollo's question for the board
/// revision.
pub fn asks_for_revision(payload: &[u8]) -> bool {
    let field = |n: u32| Fields(payload).find(|&(tag, _)| tag == n).and_then(|(_, v)| v.as_u32()).unwrap_or(0);
    field(1) == 11 && field(4) == 1
}

/// The STM32 firmware's version text: field 1 = 1, field 2 = the text, up
/// to 47 characters (stock sender 0x0806D5E8; the stock text is
/// "CONBINE_LTAQ4002_0664_202109111518").
///
/// In the Apollo (main task, 0x3F27A in its image): it keeps the text,
/// logs it as `st version:...`, and because field 1 is 1 it answers with
/// a message of the same kind carrying field 1 = 0 and its own version
/// text.
pub fn version(text: &str) -> Encoder {
    Encoder::new().number(1, 1).text(2, text)
}

/// Motor command "custom vibration": `count` pulses of `on_ms`, `off_ms`
/// apart.
///
/// From the stock app's `service_LinearMotor_Control` (0x08091BD4), which
/// sends kind 84 for every vibration. Its table of effects at 0x081C54C0
/// gives the type and the numbers; for type 2 the sender at 0x08091B1A
/// fills field 1 = 1 (on), 2 = number of pulses, 3 = pause in ms, 5 = 2
/// (the type) and 9 = pulse length in ms. Examples from the table: one
/// pulse of 200 ms, one of 1000 ms, two of 200 ms with 100 ms between.
pub fn motor_pulses(on_ms: u16, off_ms: u16, count: u8) -> Encoder {
    Encoder::new().number(1, 1).number(2, count as u32).number(3, off_ms as u32).number(5, 2).number(9, on_ms as u32)
}

/// Tell the Apollo that the watch is past its first-time setup.
///
/// Without this the Apollo never detects the wrist, and without a wrist it
/// does not switch the heart-rate sensor on:
///
///   - The stock app sends kind 140 with the value 3 when setup is over
///     (0x08033CBC, log text "SEND OOBE status").
///   - The Apollo's handler for it (`lcAlgoOOBEStatusSet`, 0x151B2 in its
///     image) stores the value and, if it is 3, switches its capacitive
///     wear detection on (`lcAlgoOffbodySwitchCap`).
///   - Its wrist check (`lcAlgoWearWristCheckExe`) then sets the wear state
///     to 8 once both the capacitive sensor and the optical sensor see
///     skin, and `lcAlgoHeartRateEnable` does nothing unless the state is 8.
///
/// The Apollo keeps the value in its own flash.
pub fn setup_finished() -> Encoder {
    Encoder::new().number(1, 3)
}

// Settings. After the setup status the stock app sends its settings to the
// Apollo, in this order (0x08033D38): do-not-disturb (kind 122), sedentary
// reminder (125), raise-to-wake (121), heart-rate monitoring (123) with
// stress monitoring (135), kind 142, SpO2 monitoring (124) and the screen
// status (130). The first three and the last are read through on both
// sides and sent here. The Apollo keeps none of them across its restart.

/// A time of day in minutes since midnight.
pub type Minutes = u16;

/// Do-not-disturb: switched on or off, with up to two daily time windows.
///
/// Stock sender 0x0806D822 (B.70), called from 0x0803394A. With nothing
/// set the stock app sends off, a first window of the whole day and an
/// unused second one (0xFFFF); with a schedule, on and its window.
///
/// In the Apollo (image 0704): `lcSensorDisturbSet` (0x146FC) stores the
/// fields, logs them as "onOff, 0: start~end, 1: start~end" and
/// acknowledges. `lcAlgoCheckDNDMode` (0x371C0) works out from them and
/// the Apollo's clock whether do-not-disturb holds right now: on, and
/// the minute of the day inside either window (from before to: both
/// minutes included; otherwise over midnight; equal: all day). The one
/// thing that looks at the result is the wrist gesture (0x12B8C): while
/// it holds, "raised" is not reported, except during sport. Nothing
/// comes back to the STM32, and the sedentary reminder is not held back
/// by it: what else is to be quiet is the STM32's business. Start
/// value: off.
pub fn do_not_disturb(on: bool, first: (Minutes, Minutes), second: Option<(Minutes, Minutes)>) -> Encoder {
    let second = second.unwrap_or((0xFFFF, 0xFFFF));
    Encoder::new()
        .number(1, on as u32)
        .number(2, first.0 as u32)
        .number(3, first.1 as u32)
        .number(4, second.0 as u32)
        .number(5, second.1 as u32)
}

/// Sedentary reminder: the daily time window in which the Apollo reminds
/// the wearer to move, `None` for off; with `lunch_break` not between
/// 12:00 and 14:00.
///
/// Stock sender 0x0806D994 (B.70), called from 0x08033C16 with the stock
/// setting 0xA8, whose start values are on, 09:00 to 21:00, no lunch
/// break. Field 1 = on, 2 and 3 = the window, 4 = the lunch break, 5 and
/// 6 = its times, which the stock app sends as 720 and 840 and the
/// Apollo never reads: the two hours are fixed in its code.
///
/// In the Apollo (image 0704): the handler (0x1544E) stores the fields
/// and switches its algorithm 7 by field 1; it sends no acknowledgement.
/// `lcAlgoSedentaryProcess` (0x377A8) keeps the algorithm on inside the
/// window and off while the watch is off the wrist for a minute or more,
/// during sport, and after ten minutes of what looks like its sleep
/// state. The detection itself is the motion library's, set up for 60
/// minutes without movement (`CWM_AP_Init`, 0x2040C). When it fires,
/// `lcAlgo_send_sedentary_reminder` (0x379C4) sends kind
/// `MOVE_REMINDER` (field 1 the Apollo's time, field 2 = 1) and nothing
/// else: the vibration and whether the wearer is to be left alone are
/// the STM32's business (the stock app's handler 0x0806CB8C plays its
/// motor effect 6 unless its own do-not-disturb holds or the wearer
/// sleeps).
pub fn sedentary(window: Option<(Minutes, Minutes)>, lunch_break: bool) -> Encoder {
    let (from, to) = window.unwrap_or((0, 0));
    Encoder::new()
        .number(1, window.is_some() as u32)
        .number(2, from as u32)
        .number(3, to as u32)
        .number(4, lunch_break as u32)
        .number(5, 720)
        .number(6, 840)
}

/// Start or stop a single heart-rate measurement.
///
/// The stock app sends kind 123 from `sendHeartRateSet` (B.70:
/// 0x0806D85E); the Apollo's `lcSensorHeartRateSet` (0x147DC in its image
/// 0704) stores the fields and acknowledges. What the Apollo does with
/// them (read there, 2026-11-24):
///
/// - 8, the mode (`lcAlgoHeartRateSetMonitorMode`, 0x30E54): 0 = no
///   monitoring; 1 = periodic, the green light on for 30 in every 630
///   seconds while worn, both numbers constants in the code
///   (`lcAlgoHeartRateMonitor`, 0x31AB0); 2 = continuous; 3 = one
///   measurement by hand, which is what this sends; 4 = sport, used by
///   the Apollo itself; 0xFF = factory test, which sets a flag that
///   stays, and is not to be sent. Modes 0 to 2 ignore field 1.
/// - 1: for mode 3, on or off. The Apollo counts these: two "on" need
///   two "off", and nothing in it ends a measurement by itself. For
///   every mode it also says whether results are reported at all (only
///   while worn).
/// - 7: seconds between two reports.
/// - 2 and 4: the warning for a high heart rate, switch and limit in
///   beats a minute (`lcAlgoHeartReteEarlyWarnning`, 0x322A8); the
///   warning comes as kind 55 (start time, lowest, highest). Zero here.
/// - 3, 5, 6: never read by the Apollo. The stock app sends 40 in 3.
///
/// Results come back as kind `HEART`, field 8 = 3 for the heart rate:
/// 2 = beats a minute (0 until there is one), 3 = the day's resting
/// rate, 4 = the algorithm's quality, 25 = 0 nothing yet, 1 valid,
/// 2 failed, 3 not worn. The Apollo keeps none of this across its
/// restart. Not read here yet, all in docs/hardware-plan.md: stress
/// (kind 135), SpO2 (124), the resting rate handed back (142), the
/// light's own report (58).
///
/// Measured earlier: with mode 2 alone and field 1 = 0 the Apollo
/// acknowledged and no report came, which fits the above (field 1
/// decides about reports), though it was taken then for "measures
/// nothing".
pub fn heart_rate(on: bool) -> Encoder {
    Encoder::new().number(1, on as u32).number(3, 40).number(7, 1).number(8, 3)
}

/// Switch the watch off: kind 102 with field 1 = 3.
///
/// Kind 102 carries one number, a system state. The stock app's "model
/// sys shutdown" (0x0807FAB2) sends 3 through 0x0805C6C6; its other call
/// sites send 2 (restart), 4 (the low-battery shutdown), 5, 6 and 7, none
/// of which is sent here. The Apollo's dispatcher (0x3EC88) logs "shut
/// down event!" for 3 and hands it to its main task; the routine that
/// then runs (0x54164) stops the sensors, switches supply pins and puts
/// the Apollo itself to sleep. Measured: the Apollo acknowledges, then
/// takes no more messages, and the STM32 keeps running; switching the
/// STM32 off is the STM32's own step (`hal::power::shutdown`). On the
/// charger the STM32 was restarted within 10 s of the message.
pub fn power_off() -> Encoder {
    Encoder::new().number(1, 3)
}

/// Motor command "stop" (stock: 0x08091D18): field 1 = 0, field 5 = 6.
pub fn motor_stop() -> Encoder {
    Encoder::new().number(5, 6)
}

/// One protobuf field value.
#[derive(Clone, Copy)]
pub enum Value<'a> {
    Varint(u64),
    Fixed32(u32),
    #[allow(dead_code)] // decoded for completeness; no known message uses it yet
    Fixed64(u64),
    Bytes(&'a [u8]),
}

impl Value<'_> {
    pub fn as_u32(&self) -> Option<u32> {
        match *self {
            Value::Varint(v) => Some(v as u32),
            _ => None,
        }
    }
    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Value::Fixed32(v) => Some(f32::from_bits(v)),
            _ => None,
        }
    }
}

/// Iterates over the fields of a protobuf message: (field number, value).
/// Stops at the first malformed field.
pub struct Fields<'a>(pub &'a [u8]);

fn varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, &b) in data.iter().enumerate().take(10) {
        value |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u32, Value<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let (key, used) = varint(self.0)?;
        let rest = &self.0[used..];
        let (value, used) = match key & 7 {
            0 => {
                let (v, n) = varint(rest)?;
                (Value::Varint(v), n)
            }
            1 => (Value::Fixed64(u64::from_le_bytes(rest.get(..8)?.try_into().ok()?)), 8),
            5 => (Value::Fixed32(u32::from_le_bytes(rest.get(..4)?.try_into().ok()?)), 4),
            2 => {
                let (len, n) = varint(rest)?;
                (Value::Bytes(rest.get(n..n + len as usize)?), n + len as usize)
            }
            _ => return None,
        };
        self.0 = &rest[used..];
        Some(((key >> 3) as u32, value))
    }
}

/// The latest sensor values the Apollo has reported.
#[derive(Default, Clone, Copy)]
pub struct Readings {
    /// Percent, millivolts, USB power present.
    pub battery: Option<(u32, f32, bool)>,
    /// Three temperature sensors, degrees Celsius.
    pub temperatures: Option<[f32; 3]>,
    /// Pascal, metres.
    pub barometer: Option<(f32, f32)>,
    /// The latest wear report.
    pub wear: Option<Wear>,
    /// The Apollo's off-wrist notification (kind `OFF_WRIST`, field 2):
    /// true once the watch has been off the wrist for longer than the
    /// timeout set with `off_wrist_timeout`.
    pub off_wrist_long: Option<bool>,
    /// The Apollo's reported (shutdown type, reset reason).
    pub apollo_reset: Option<(u32, u32)>,
    /// Heart rate in beats per minute from the latest heart report (kind
    /// `HEART`, field 2; the stock app logs that field as "hr" at
    /// 0x0806CB8A). 0 while the Apollo has no result yet.
    pub heart_bpm: Option<u32>,
    /// Steps today, from the latest summary of the day's activity (kind
    /// `DAILY`): the first four bytes of its field 3, which the stock
    /// app's decoder (B.70: 0x08053D56) logs as `rpt->step_number`.
    ///
    /// Read in the Apollo's image 0704: its daily-activity algorithm
    /// runs from start without being asked, counts in RAM and starts at
    /// zero when the day changes (`lcAlgoDaily_pre_cal`, 0x26058). By
    /// that reading a restart of the Apollo starts it at zero too, but
    /// measured (2026-11-24) the count was still there 11 s after a
    /// restart of this firmware: 7629 steps in the evening of a worn
    /// day. Either the Apollo is not restarted with the STM32 or it
    /// gets the count back; not looked into. The summary is sent
    /// (`lcAlgoDaily_send_report_internal`, 0x2847A) when something
    /// changed and only while the Apollo was told the screen is on;
    /// there is no message to ask for it. So this is the count as of
    /// the last time the display was lit.
    pub steps: Option<u32>,
}

impl Readings {
    /// Take the values out of a sub-message of the given kind. Kinds without
    /// sensor values are ignored.
    pub fn update(&mut self, kind: u32, payload: &[u8]) {
        let field = |n: u32| Fields(payload).find(|&(tag, _)| tag == n).map(|(_, v)| v);
        match kind {
            BATTERY => {
                if let (Some(percent), Some(mv)) =
                    (field(3).and_then(|v| v.as_u32()), field(5).and_then(|v| v.as_f32()))
                {
                    let usb = field(2).and_then(|v| v.as_u32()).unwrap_or(0) != 0;
                    self.battery = Some((percent, mv, usb));
                }
            }
            TEMPERATURES => {
                let t = |n| field(n).and_then(|v| v.as_f32());
                if let (Some(a), Some(b), Some(c)) = (t(2), t(3), t(4)) {
                    self.temperatures = Some([a, b, c]);
                }
            }
            BAROMETER => {
                if let (Some(p), Some(h)) = (field(2).and_then(|v| v.as_f32()), field(3).and_then(|v| v.as_f32())) {
                    self.barometer = Some((p, h));
                }
            }
            WEAR => {
                let n = |i| field(i).and_then(|v| v.as_u32()).unwrap_or(0);
                self.wear = Some(Wear {
                    on_wrist: n(2) == 2,
                    reason: match n(3) {
                        0 => WearReason::Start,
                        1 => WearReason::InfraredFar,
                        2 => WearReason::CapacitiveFar,
                        4 => WearReason::PutOn,
                        other => WearReason::Other(other),
                    },
                    capacitive_status: n(4),
                });
            }
            OFF_WRIST => {
                // Only the Apollo's notification (field 1 = 1); the same
                // kind with field 1 = 0 is our own setting.
                if field(1).and_then(|v| v.as_u32()) == Some(1) {
                    self.off_wrist_long = Some(field(2).and_then(|v| v.as_u32()).unwrap_or(0) != 0);
                }
            }
            DAILY => {
                if let Some(Value::Bytes(&[a, b, c, d, ..])) = field(3) {
                    self.steps = Some(u32::from_le_bytes([a, b, c, d]));
                }
            }
            HEART => {
                self.heart_bpm = Some(field(2).and_then(|v| v.as_u32()).unwrap_or(0));
            }
            RESET_INFO => {
                let n = |i| field(i).and_then(|v| v.as_u32()).unwrap_or(0);
                self.apollo_reset = Some((n(1), n(2)));
            }
            _ => {}
        }
    }
}

/// A wear report (kind `WEAR`).
///
/// How the Apollo decides (`lcAlgoWearWristCheckExe`, 0x3282C in its image;
/// its own log texts name the inputs "cap" and "ir"):
///
///   - Two sensors feed it: the capacitive sensor (near / far) and the
///     optical sensor's infrared check (near / far).
///   - Not worn becomes worn when both have reported "near" and each has
///     done so for at least a second. Reason: `PutOn`.
///   - Worn becomes not worn when the infrared check reports "far"
///     (`InfraredFar`), or when the capacitive sensor reported "far" and a
///     second check confirmed it (`CapacitiveFar`).
///   - Inside the Apollo the state is 8 for worn and 3 for not worn; the
///     message carries 2 and 1.
///
/// The Apollo sends a report when the state has changed and stayed changed
/// for half a second, and once after its start (`Start`). Its capacitive
/// sensor is only on after the setup status (`setup_finished`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Wear {
    pub on_wrist: bool,
    pub reason: WearReason,
    /// Field 4: three status bytes of the capacitive sensor at the moment
    /// of a `CapacitiveFar` removal (`lcAlgoSetCapSensorResult`), 0
    /// otherwise. The stock app only logs it ("offbodyData").
    pub capacitive_status: u32,
}

/// Why the wear state is what it is (field 3 of the report).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WearReason {
    /// The first report after the Apollo's start.
    Start,
    /// Taken off: the infrared check no longer sees skin.
    InfraredFar,
    /// Taken off: the capacitive sensor no longer senses the arm.
    CapacitiveFar,
    /// Put on.
    PutOn,
    /// A value the Apollo's code does not set.
    Other(u32),
}

impl WearReason {
    pub fn name(self) -> &'static str {
        match self {
            WearReason::Start => "first report",
            WearReason::InfraredFar => "infrared sees no skin",
            WearReason::CapacitiveFar => "capacitive sensor senses no arm",
            WearReason::PutOn => "put on",
            WearReason::Other(_) => "unknown reason",
        }
    }
}

/// Switch the Apollo's wrist-raise detection on or off, with the daily
/// time window in which it is active.
///
/// Stock sender 0x0806D6A0 (B.70: 0x0806D7F4), called from 0x08033C64:
/// field 1 = on, field 2 = start and field 3 = end of the window in
/// minutes of the day (the stock app always sends 0 and 1439), field 4 = a
/// byte from its settings that must be 0 for the Apollo to act.
///
/// In the Apollo (handler 0x146C8, then 0x12F5C, in its image) this
/// switches its algorithm 13, the wrist-raise algorithm, on while the
/// time of day is inside the window. Its start state is off. The
/// algorithm's two results are treated differently (0x23F7A): "lowered"
/// is always reported, "raised" only while algorithm 13 is on. Both
/// arrive as `WristGesture`.
///
/// "Off" is what the stock app sends when the user switches the feature
/// off (its settings writer 0x08080290 calls the sender after storing).
/// The Apollo then only clears the algorithm's bit (0x36E0C, 0x23DB4):
/// the detection keeps running, the report is dropped. Its detector
/// (0x488E8) looks for a raise only while it was told the screen is off,
/// so with the feature off and the screen off nothing is sent at all.
/// The detector has no setting for its sensitivity: every limit is a
/// constant in its code, and it reads nothing but the three acceleration
/// axes and the screen state.
pub fn raise_to_wake(on: bool, window: (Minutes, Minutes)) -> Encoder {
    Encoder::new().number(1, on as u32).number(2, window.0 as u32).number(3, window.1 as u32)
}

/// What the Apollo's wrist-raise algorithm saw (kind `WRIST_GESTURE`,
/// field 2). The Apollo runs it on every motion sample together with the
/// screen state it was told (`screen_status`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WristGesture {
    /// The wrist was raised and turned towards the wearer (value 1). The
    /// stock app turns the screen on (0x08041210).
    Raised,
    /// The wrist was lowered or turned away (value 2). The stock app turns
    /// the screen off if the screen is on and one of its settings has the
    /// value 2; which setting is not read yet.
    Lowered,
    Other(u32),
}

/// Decode a `WRIST_GESTURE` sub-message.
pub fn wrist_gesture(payload: &[u8]) -> WristGesture {
    match Fields(payload).find(|&(tag, _)| tag == 2).and_then(|(_, v)| v.as_u32()).unwrap_or(0) {
        1 => WristGesture::Raised,
        2 => WristGesture::Lowered,
        other => WristGesture::Other(other),
    }
}

/// Tell the Apollo where its own log text goes and what it contains.
///
/// Stock: the command `AT^LOGSETPARA=APOLLO=0x...` (0x0809D238 in B.70;
/// low 4 bits = level, bits 8 and up = module mask) ends in the sender
/// 0x0806D774: field 1 = 2, field 2 = level, field 3 = module mask.
///
/// In the Apollo (`lcLogEventParse`, 0x408B0 in its image): with level and
/// mask both non-zero, field 1 becomes the output mode and the level and
/// mask are stored; otherwise the mode goes back to 1. In mode 2 the log
/// text is sent to the STM32 in blocks that start with 0x7F, a type byte
/// and a length (0x1CFF6). The Apollo's start values are mode 1, level 4,
/// mask 0x001801CF. How the message gets from the Apollo's dispatcher to
/// that routine has not been read.
pub fn log_setting(to_stm32: bool, level: u8, modules: u32) -> Encoder {
    if to_stm32 {
        Encoder::new().number(1, 2).number(2, level as u32).number(3, modules)
    } else {
        Encoder::new()
    }
}

/// Tell the Apollo whether the screen is on.
///
/// Stock sender 0x0806D92E: field 1 = 0, field 2 = 1 for on, 0 for off. It
/// is sent when the backlight is set (on, 0x08040AD2), when the screen
/// goes off (0x08040F76), and once with the current state after the
/// Apollo's start, as the last of the settings (0x08033D68).
///
/// What the Apollo does with it (`lcSensorScreenStatusSet`, 0x14D14 in its
/// image, and the readers of the stored value); its start value is "off":
///
///   - It holds its cached health data back while the screen is off and
///     sends it when the screen is on (`lcDataCachePop`).
///   - On "on" it makes the sport module send a fresh report.
///   - It hands the state to its wrist-raise algorithm with every motion
///     sample (`lcSensorAlgoInput`, which logs "raise"); when that
///     algorithm fires, the Apollo sends kind 41 (`WRIST_GESTURE`, field
///     2 = the algorithm's result) from 0x12908. The stock app turns the
///     screen on or off from it.
///   - When the screen goes off and no sport is running, it stretches the
///     barometer's sampling interval to ten minutes (0x14E06, 0x343DA).
///   - The off-wrist timeout only counts while the screen is off
///     (`off_wrist_timeout`).
pub fn screen_status(on: bool) -> Encoder {
    Encoder::new().number(2, on as u32)
}

/// Set after how many minutes off the wrist the Apollo sends its off-wrist
/// notification; 0 switches the notification off.
///
/// Kind 11 goes both ways. From the STM32 with field 1 = 0 it sets the
/// timeout (field 2, minutes; the Apollo's dispatcher at 0x10CF4 logs it
/// as "argOffWrist"). From the Apollo with field 1 = 1 it reports in field
/// 2 whether the watch has been off the wrist for that long (0x372B8,
/// 0x37310); see `Readings::off_wrist_long`. The Apollo only counts while
/// it has been told that the screen is off (kind 130, not sent yet), so
/// this is not switched on yet.
#[allow(dead_code)] // waits for the screen status message
pub fn off_wrist_timeout(minutes: u8) -> Encoder {
    Encoder::new().number(2, minutes as u32)
}

/// What the lower side button did. It is wired to the Apollo, which reports
/// it in messages of kind `KEY`.
///
/// How the Apollo arrives at these (read in its firmware: the edge handler
/// at 0x40FAC, the timer callback at 0x40F00, the sender at 0x40CF8):
///
/// - `Pressed` and `Released` are sent on the edges.
/// - `Clicked` comes 300 ms after a release, from a timer, if the button
///   was held for less than 800 ms and is not pressed again in that time.
/// - Pressed again within those 300 ms, and let go within 300 ms of that
///   press: `DoubleClicked` is sent in place of the second release, and no
///   `Clicked` for either press. Held longer the second time: one
///   `Clicked` after its release, and the first press counts for nothing.
/// - Held: nothing at 800 ms (an inner step that is not sent), then
///   `LongPressed` at 1.5 s and `VeryLongPressed` at 5 s. A release after
///   800 ms gives `Released` and no `Clicked`.
/// - A second release report while none is expected (the button bounces;
///   seen on the watch as two with the same Apollo time) stops the timer,
///   and the `Clicked` that was due never comes.
///
/// Because of the last three points, short presses are counted here from
/// `Pressed` and `Released` (`Apollo::take_short_presses`) wherever every
/// press matters.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Released,
    Pressed,
    Clicked,
    DoubleClicked,
    LongPressed,
    VeryLongPressed,
    /// A value the Apollo's code does not send.
    Other(u32),
}

impl KeyEvent {
    pub fn name(self) -> &'static str {
        match self {
            KeyEvent::Released => "released",
            KeyEvent::Pressed => "pressed",
            KeyEvent::Clicked => "clicked",
            KeyEvent::DoubleClicked => "double click",
            KeyEvent::LongPressed => "long press",
            KeyEvent::VeryLongPressed => "very long press",
            KeyEvent::Other(_) => "unknown event",
        }
    }
}

/// Decode a `KEY` sub-message: the event and the Apollo's uptime in ms.
pub fn key_event(payload: &[u8]) -> (KeyEvent, u32) {
    let field = |n: u32| Fields(payload).find(|&(tag, _)| tag == n).and_then(|(_, v)| v.as_u32()).unwrap_or(0);
    let event = match field(2) {
        0 => KeyEvent::Released,
        1 => KeyEvent::Pressed,
        2 => KeyEvent::Clicked,
        3 => KeyEvent::DoubleClicked,
        4 => KeyEvent::LongPressed,
        5 => KeyEvent::VeryLongPressed,
        other => KeyEvent::Other(other),
    };
    (event, field(3))
}
