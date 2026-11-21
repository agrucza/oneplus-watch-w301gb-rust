//! What the HAL does not provide for this chip, at register level. Knows the
//! STM32, nothing about the watch's other chips.

pub mod clocks;
pub mod dsi;
pub mod fmc;
pub mod reg;
pub mod sdmmc;
