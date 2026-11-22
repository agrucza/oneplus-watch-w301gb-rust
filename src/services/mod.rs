//! What the firmware does with the drivers.

pub mod bluetooth;
pub mod brightness;
pub mod canvas;
pub mod log;
#[allow(dead_code)] // some read helpers have no caller at the moment
pub mod storage;
pub mod update;
