//! App-level helpers for the demos, kept out of the `esp32-s31-coreboard`
//! library so a firmware using the board doesn't pull them in.
//!
//! - [`demo`]: the `start!` macro every demo begins with.
//! - [`net`]: Wi-Fi station with reconnect, embassy-net stacks with DHCP
//!   for Wi-Fi and Ethernet, SNTP client, clock and DNS helpers.
//! - [`http`]: request-line parsing, query parameters, colors and response
//!   headers for the web demos.
//! - [`sntp`], [`time`]: SNTP packets and dates from Unix time.
//!
//! [`http`], [`sntp`] and [`time`] have no hardware dependencies and also
//! build for a PC, where they are unit-tested:
//! `cargo test -p esp32-s31-coreboard-examples --lib --target <host triple>`.

#![no_std]
#![warn(missing_docs)]

pub mod http;
pub mod sntp;
pub mod time;

#[cfg(target_arch = "riscv32")]
pub mod demo;
#[cfg(target_arch = "riscv32")]
pub mod net;
