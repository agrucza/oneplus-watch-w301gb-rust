//! What the firmware does with the drivers.

pub mod bluetooth;
pub mod brightness;
pub mod canvas;
pub mod clock;
pub mod damage;
pub mod factory;
pub mod font;
pub mod font_data;
pub mod gestures;
pub mod log;
#[allow(dead_code)] // some read helpers have no caller at the moment
pub mod storage;
pub mod turn;
pub mod update;
pub mod wav;
