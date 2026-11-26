//! What is going over Bluetooth right now, for the page that shows it:
//! set by the update server, read by the display task, which can also
//! ask for the transfer to be ended.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// A transfer over Bluetooth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transfer {
    /// A firmware package arrives.
    Receiving,
    /// A file from the watch's storage goes to the PC.
    Sending,
    /// A PC asks to send a firmware package: the wearer is to say yes
    /// or no (`decide`).
    Offered,
    /// A PC asks for a file from the watch, the same way.
    OfferedGet,
    /// A PC asks to write a small file to the watch, the same way.
    OfferedPut,
    /// The wearer said yes, and the PC has not begun yet.
    Waiting,
}

impl Transfer {
    /// Whether this is a question to the wearer.
    pub fn is_offer(self) -> bool {
        matches!(self, Transfer::Offered | Transfer::OfferedGet | Transfer::OfferedPut)
    }
}

static KIND: AtomicU8 = AtomicU8::new(0);
static PERCENT: AtomicU8 = AtomicU8::new(0);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// The answer to an offer: 0 none yet, 1 accepted, 2 denied.
static DECISION: AtomicU8 = AtomicU8::new(0);

/// Say that `transfer` has begun; it has no request to end it yet.
pub fn begin(transfer: Transfer) {
    PERCENT.store(0, Ordering::Relaxed);
    CANCEL.store(false, Ordering::Relaxed);
    DECISION.store(0, Ordering::Relaxed);
    KIND.store(transfer as u8 + 1, Ordering::Relaxed);
}

/// Say how far it is: `done` of `total` bytes.
pub fn at(done: usize, total: usize) {
    PERCENT.store((done as u64 * 100 / total.max(1) as u64).min(100) as u8, Ordering::Relaxed);
}

/// Say that it is over, however it went.
pub fn end() {
    KIND.store(0, Ordering::Relaxed);
}

/// The transfer under way and how many percent of it are through.
pub fn now() -> Option<(Transfer, u8)> {
    let transfer = match KIND.load(Ordering::Relaxed) {
        1 => Transfer::Receiving,
        2 => Transfer::Sending,
        3 => Transfer::Offered,
        4 => Transfer::OfferedGet,
        5 => Transfer::OfferedPut,
        6 => Transfer::Waiting,
        _ => return None,
    };
    Some((transfer, PERCENT.load(Ordering::Relaxed)))
}

/// Answer the offer on the display: accept it or deny it.
pub fn decide(accept: bool) {
    DECISION.store(if accept { 1 } else { 2 }, Ordering::Relaxed);
}

/// The answer to the offer, once there is one.
pub fn decision() -> Option<bool> {
    match DECISION.load(Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Ask for the transfer under way to be ended.
pub fn cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// Whether ending was asked for since the transfer began.
pub fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}
