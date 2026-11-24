//! Text log that ends up in `3:log/rust.log` on the eMMC, where the
//! Bluetooth file transfer can fetch it.
//!
//! There is one log for the whole firmware. `Log` is a handle to it: get
//! one with `Log::get()` wherever something is worth writing down. Lines
//! are collected in RAM and written out with `flush`. Flush before any step
//! that might hang: what is still in RAM is lost on reset.
//!
//! Only code running in tasks may log, not interrupt handlers: everything
//! here runs on the one executor thread and never waits, so a line cannot
//! be torn apart by another task. From an interrupt handler the calls do
//! nothing.

use core::cell::UnsafeCell;
use core::fmt;

use cortex_m::peripheral::scb::VectActive;
use cortex_m::peripheral::SCB;
use embassy_time::Instant;

use crate::services::storage::{self, Partition};

const CAPACITY: usize = 32 * 1024;

struct Buffer {
    text: [u8; CAPACITY],
    len: usize,
    dropped: bool,
    /// Where `flush` writes; `None` until `Log::start`.
    partition: Option<Partition>,
}

struct Shared(UnsafeCell<Buffer>);
// Safety: only touched from thread mode (checked in `buffer`), where a
// single executor runs the tasks one at a time, and never across an await.
unsafe impl Sync for Shared {}

static SHARED: Shared =
    Shared(UnsafeCell::new(Buffer { text: [0; CAPACITY], len: 0, dropped: false, partition: None }));

/// The log buffer, or `None` when called from an interrupt handler.
fn buffer() -> Option<&'static mut Buffer> {
    match SCB::vect_active() {
        // Safety: see `Shared`. The reference does not outlive the calling
        // function, and none of them calls back into the log while holding it.
        VectActive::ThreadMode => Some(unsafe { &mut *SHARED.0.get() }),
        _ => None,
    }
}

/// A handle to the firmware's log.
#[derive(Clone, Copy)]
pub struct Log;

impl Log {
    /// Set the volume the log file lives on and return a handle.
    pub fn start(partition: Partition) -> Self {
        if let Some(b) = buffer() {
            b.partition = Some(partition);
        }
        Log
    }

    /// A handle to the log, for code that was not handed one.
    pub fn get() -> Self {
        Log
    }

    /// The volume the log file lives on, once `start` has set it.
    pub fn partition(&self) -> Option<Partition> {
        buffer().and_then(|b| b.partition)
    }

    /// True if there is text that `flush` has not written out yet.
    pub fn pending(&self) -> bool {
        buffer().is_some_and(|b| b.len > 0)
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

    /// Append everything collected so far to `/LOG/RUST.LOG`. On failure
    /// returns the step (see `storage::append`; 0 = no volume set, or
    /// called from an interrupt handler); the text is kept.
    pub fn flush(&mut self) -> Result<(), u8> {
        let b = buffer().ok_or(0)?;
        let partition = b.partition.ok_or(0)?;
        if b.dropped {
            b.dropped = false;
            let note = b"(log buffer overflowed; some text was dropped)\n";
            let n = note.len().min(CAPACITY - b.len);
            b.text[b.len..b.len + n].copy_from_slice(&note[..n]);
            b.len += n;
        }
        storage::append(partition, "LOG", "RUST.LOG", &b.text[..b.len])?;
        b.len = 0;
        Ok(())
    }
}

impl fmt::Write for Log {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let Some(b) = buffer() else {
            return Ok(());
        };
        // Keep 64 bytes back for the overflow note.
        let room = (CAPACITY - 64).saturating_sub(b.len);
        let n = s.len().min(room);
        b.dropped |= n < s.len();
        b.text[b.len..b.len + n].copy_from_slice(&s.as_bytes()[..n]);
        b.len += n;
        Ok(())
    }
}
