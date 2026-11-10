//! App-level helpers for the demos, kept out of the `esp32-s31-coreboard`
//! library so a firmware using the board doesn't pull them in.
//!
//! - [`demo`]: the `start!` macro every demo begins with.

#![no_std]
#![warn(missing_docs)]

#[cfg(target_arch = "riscv32")]
pub mod demo;
