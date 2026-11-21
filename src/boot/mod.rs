//! What keeps the watch recoverable: the return ticket to the stock
//! firmware, the internal-flash writes it needs, and the watchdog. Depends
//! on nothing else in the firmware, on purpose.

pub mod faults;
pub mod flash;
pub mod ticket;
pub mod watchdog;
