//! What a touch report from the touch controller says.
//!
//! The layout is the stock app's reading of it (`0x08078DCC` onwards,
//! version B.70): byte 2 is the report's kind, 1 for touches. Byte 5
//! holds the number of touches in bits 4:0 and, in bit 5, "large object
//! detected" (`0x08079082`): something much bigger than a fingertip lies
//! on the glass, a palm for example. Ten bytes per touch follow from
//! byte 7: byte 1 of them is the touch number (bits 4:0) and the event
//! (bits 6:5, 3 = finger lifted), bytes 2-3 are X and 4-5 are Y, low byte
//! first. Only the first touch is read, as the stock app's interface does.

/// The kind of report that carries touches.
pub const KIND_TOUCH: u8 = 0x01;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TouchReport {
    /// Something large lies on the glass.
    pub large_object: bool,
    /// Where the first finger is; `None` if no finger is down.
    pub finger: Option<(u16, u16)>,
}

/// Read a report (from its length bytes on). `None` if it is not a touch
/// report or too short to be one.
pub fn parse(report: &[u8]) -> Option<TouchReport> {
    if report.len() < 7 || report[2] != KIND_TOUCH {
        return None;
    }
    let count = report[5] & 0x1F;
    let lifted = count == 0 || report.len() < 17 || (report[8] >> 5) & 3 == 3;
    let finger = (!lifted).then(|| (u16::from_le_bytes([report[9], report[10]]), u16::from_le_bytes([report[11], report[12]])));
    Some(TouchReport { large_object: report[5] & 0x20 != 0, finger })
}
