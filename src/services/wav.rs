//! The head of a WAV file: what kind of sound it holds and where its
//! samples are.
//!
//! A WAV file is "RIFF", a length, "WAVE", and then chunks, each a
//! four-letter name, a length and that many bytes (padded to an even
//! number). "fmt " says how the samples are stored, "data" holds them;
//! others (the watch's sound files have "fact" and "bext" ones) are
//! passed over.

/// What a WAV file holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wav {
    pub channels: u16,
    /// Samples a second, for each channel.
    pub rate: u32,
    pub bits: u16,
    /// Where the samples begin in the file, and how many bytes of them
    /// there are.
    pub data_at: u32,
    pub data_len: u32,
}

/// Read a file's head. `head` is the start of the file, as much of it as
/// is at hand (1024 bytes are enough for the watch's own files); `None`
/// if it is no WAV file of plain samples or its "data" chunk does not
/// begin inside `head`.
pub fn parse(head: &[u8]) -> Option<Wav> {
    let word = |at: usize| Some(u32::from_le_bytes(head.get(at..at + 4)?.try_into().ok()?));
    let half = |at: usize| Some(u16::from_le_bytes(head.get(at..at + 2)?.try_into().ok()?));
    if head.get(..4)? != b"RIFF" || head.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let mut format = None;
    loop {
        let name = head.get(at..at + 4)?;
        let length = word(at + 4)?;
        let body = at + 8;
        match name {
            b"fmt " => {
                // 1 = plain samples (PCM).
                if length < 16 || half(body)? != 1 {
                    return None;
                }
                format = Some((half(body + 2)?, word(body + 4)?, half(body + 14)?));
            }
            b"data" => {
                let (channels, rate, bits) = format?;
                return Some(Wav { channels, rate, bits, data_at: body as u32, data_len: length });
            }
            _ => {}
        }
        at = body.checked_add(length as usize)?.checked_add(length as usize & 1)?;
    }
}
