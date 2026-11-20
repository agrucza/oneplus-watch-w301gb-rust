//! Text log that ends up in `3:log/rust.log` on the eMMC, where the stock
//! firmware's Bluetooth file transfer can fetch it.
//!
//! Lines are collected in RAM and written with `flush`. Flush before any
//! step that might hang: what is still in RAM is lost on reset.

use core::fmt;

use embassy_time::Instant;

use crate::storage::{self, Partition};

const CAPACITY: usize = 32 * 1024;

pub struct Log {
    buf: &'static mut [u8; CAPACITY],
    len: usize,
    dropped: bool,
}

impl Log {
    /// The one log buffer. Returns `None` on the second call.
    pub fn take() -> Option<Self> {
        let buf = cortex_m::singleton!(: [u8; CAPACITY] = [0; CAPACITY])?;
        Some(Self { buf, len: 0, dropped: false })
    }

    /// Start a line with the time since boot.
    pub fn stamp(&mut self) -> &mut Self {
        use fmt::Write;
        let _ = write!(self, "[{:>6} ms] ", Instant::now().as_millis());
        self
    }

    /// Append bytes as hex, 32 per line, each line indented.
    pub fn hex(&mut self, bytes: &[u8]) {
        use fmt::Write;
        for line in bytes.chunks(32) {
            let _ = self.write_str("   ");
            for b in line {
                let _ = write!(self, " {:02x}", b);
            }
            let _ = self.write_str("\n");
        }
    }

    /// Append everything collected so far to `/LOG/RUST.LOG` on `partition`.
    /// On failure returns the step (see `storage::append`); the text is kept.
    pub fn flush(&mut self, partition: Partition) -> Result<(), u8> {
        if self.dropped {
            self.dropped = false;
            let note = b"(log buffer overflowed; some text was dropped)\n";
            let n = note.len().min(CAPACITY - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&note[..n]);
            self.len += n;
        }
        storage::append(partition, "LOG", "RUST.LOG", &self.buf[..self.len])?;
        self.len = 0;
        Ok(())
    }
}

impl fmt::Write for Log {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Keep 64 bytes back for the overflow note.
        let room = (CAPACITY - 64).saturating_sub(self.len);
        let n = s.len().min(room);
        self.dropped |= n < s.len();
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
