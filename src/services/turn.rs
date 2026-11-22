//! Whose turn it is, where the firmware's two tasks must not cut into
//! each other.
//!
//! The tasks run in turn on one executor: when one waits, the other
//! works, and a task that is working is not interrupted by the other. So
//! a short wait in one task can become a long one, by as much as the
//! other task works in one stretch (drawing a whole page: over 100 ms).
//!
//! The exchange of a block with the Apollo cannot bear that. It ends with
//! a line that must drop as soon as the last byte has moved; if it stays
//! up for milliseconds, the Apollo takes it for the next block. (Seen on
//! the watch: button reports arriving twice and clicks getting lost.)
//!
//! So an exchange and a stretch of drawing each hold `TURN`, and neither
//! begins while the other is under way. An exchange that has not begun
//! can wait, as it always could.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::{Mutex, MutexGuard};

static TURN: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

/// Wait for the turn and take it; it is given back when the answer is
/// dropped.
pub async fn take() -> MutexGuard<'static, CriticalSectionRawMutex, ()> {
    TURN.lock().await
}
