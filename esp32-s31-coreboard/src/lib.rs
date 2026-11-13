//! Board support for the ESP32-S31-Function-CoreBoard-1: pin map, start-up,
//! PSRAM and the shared I2C bus.
//!
//! Pin assignment follows the board user guide and Espressif's
//! `esp-board-manager` definition of this board:
//! <https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-function-coreboard-1/user_guide.html>
//!
//! # Using it in a firmware
//!
//! - The firmware's binary crate needs a `build.rs` that passes the linker
//!   script; a dependency can't do that for it:
//!
//!   ```ignore
//!   fn main() {
//!       println!("cargo:rustc-link-arg=-Tlinkall.x");
//!   }
//!   ```
//!
//! - The ESP32-S31 support in released esp-hal crates is incomplete, so
//!   this crate is built against an esp-hal git commit. `[patch.crates-io]`
//!   only applies in the root manifest, so copy the `[patch.crates-io]`
//!   section of this repository's `Cargo.toml` into the firmware's.
//! - [`board::init`] starts the HAL; the logger ([`board::init_logger`],
//!   `log` feature) and the heap are the firmware's choice.
//!
//! # The codec driver
//!
//! [`es8311`] is the [`es8311-async`](https://crates.io/crates/es8311-async) crate, re-exported:
//! a driver over any `embedded-hal-async` I2C bus, also usable on other boards.
//! The other modules build only for the ESP32-S31.

#![no_std]
#![warn(missing_docs)]

pub use es8311_async as es8311;

#[cfg(target_arch = "riscv32")]
pub mod audio;
#[cfg(target_arch = "riscv32")]
pub mod board;
#[cfg(target_arch = "riscv32")]
pub mod button;
#[cfg(target_arch = "riscv32")]
pub mod ethernet;
