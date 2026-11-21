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
//! |    43 | barometer ("BAR")               | 1 time ms, 2 pressure Pa, 3 altitude m        |
//! |    45 | wear detection ("CAP", isWear)  | 1 time ms, 2 state                            |
//! |    47 | battery ("Bat Receive")         | 1 time ms, 2 usb, 3 percent, 5 millivolts     |
//! |    50 | temperatures ("NTC msg")        | 1 time ms, 2..4 degrees Celsius               |
//! |    52 | sensor chip names               | 21..29 strings                                |
//! |    54 | sensor status (unconfirmed)     | 1 time ms, 3, 4, 8 numbers                    |
//! |    59 | Apollo's own reset info         | 1 shutdown type, 2 reset reason               |
//! |    80 | 180-byte record, type 19        | 1 time s, 2 type, 3 bytes                     |
//! |    81 | daily-activity record, type 42  | 1 time s, 2 type, 3 bytes                     |
//!
//! Times are the watch's real-time clock as Unix time.

pub const ACK: u32 = 3;
pub const HEARTBEAT: u32 = 20;
pub const BAROMETER: u32 = 43;
pub const WEAR: u32 = 45;
pub const BATTERY: u32 = 47;
pub const TEMPERATURES: u32 = 50;
pub const RESET_INFO: u32 = 59;

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
#[derive(Default)]
pub struct Readings {
    /// Percent, millivolts, USB power present.
    pub battery: Option<(u32, f32, bool)>,
    /// Three temperature sensors, degrees Celsius.
    pub temperatures: Option<[f32; 3]>,
    /// Pascal, metres.
    pub barometer: Option<(f32, f32)>,
    pub worn: Option<bool>,
    /// The Apollo's reported (shutdown type, reset reason).
    pub apollo_reset: Option<(u32, u32)>,
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
                // An absent field means zero in this encoding.
                self.worn = Some(field(2).and_then(|v| v.as_u32()).unwrap_or(0) != 0);
            }
            RESET_INFO => {
                let n = |i| field(i).and_then(|v| v.as_u32()).unwrap_or(0);
                self.apollo_reset = Some((n(1), n(2)));
            }
            _ => {}
        }
    }
}
