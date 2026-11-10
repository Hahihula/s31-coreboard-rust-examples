# ESP32-S31-Function-CoreBoard-1 Rust examples

Bare-metal (`no_std`) Rust demos for the
[ESP32-S31-Function-CoreBoard-1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-function-coreboard-1/user_guide.html)
development board (ESP32-S31-WROOM-3, 16 MB flash, 16 MB PSRAM, ES8311 codec, Gigabit
Ethernet), built on [esp-hal](https://github.com/esp-rs/esp-hal) and Embassy.

Sister repositories:
[ESP32-S31-Korvo-1](https://github.com/Hahihula/s31-korvo-1-rust-examples) and
[ESP-VoCat](https://github.com/Hahihula/vocat-rust-examples).

## Requirements

- Rust stable with the `rust-src` component (pinned in `rust-toolchain.toml`).
- [`espflash`](https://github.com/esp-rs/espflash) 4.x.
- A USB-C cable into the **USB-to-UART** port of the board.

The ESP32-S31 support in the released esp-rs crates is incomplete, so the root
`Cargo.toml` pins esp-hal, esp-rtos, esp-alloc, esp-println, esp-backtrace and
esp-bootloader-esp-idf to a single esp-hal `main` commit with `[patch.crates-io]`. The
first build downloads that commit from GitHub.

## Running a demo

Every demo is its own binary in `examples/src/bin/`:

```bash
cargo run --release --bin 00_bringup
```

The cargo runner (`.cargo/config.toml`) flashes the board through the USB-to-UART bridge
and then opens the serial monitor. Press `Ctrl+R` to reset the board and `Ctrl+C` to
quit. If flashing fails, hold **Boot**, tap **Reset**, then release **Boot** to enter
download mode by hand.

Log verbosity follows `ESP_LOG` (default `info`), for example
`ESP_LOG=debug cargo run --release --bin 00_bringup`.

## Project layout

The repository is a Cargo workspace with two crates:

- **`esp32-s31-coreboard`**: the board library. Pin map, start-up, PSRAM, the codec I2C
  bus. Use it in your own firmware (see below).
- **`esp32-s31-coreboard-examples`** (`examples/`): the demos, plus the app-level helpers
  that only the demos need.

| Path | Contents |
|------|----------|
| `esp32-s31-coreboard/src/lib.rs` | Crate docs: what a firmware using the library needs |
| `esp32-s31-coreboard/src/board.rs` | Board pin map, I2C device addresses, `init()`, logger, codec I2C bus, PSRAM mapping (`psram_reserve`, `psram_split`) |
| `examples/src/lib.rs`, `demo.rs` | Helper crate root; the `start!` macro the demos begin with |
| `examples/build.rs` | Linker script (`linkall.x`) and hints for common link errors |
| `examples/src/bin/NN_*.rs` | The demos |

## Board pin map

| Function | GPIO |
|----------|------|
| ES8311 I2C SDA / SCL | 51 / 50 |
| ES8311 I2S MCLK | 52 |
| ES8311 I2S BCLK / WS / DOUT / DIN | 53 / 55 / 56 / 54 |
| NS4150B speaker PA enable | 57 (active high) |
| RGB LED (WS2812) | 60 |
| BOOT button | 61 (active low) |
| Ethernet PHY (YT8531) MDC / MDIO / reset | 5 / 6 / 7 |
| RGMII TXD0-3 / TX_CTL / TX_CLK | 8-11 / 12 / 13 |
| RGMII RX_CLK / RX_CTL / RXD3-0 | 14 / 15 / 16-19 |
| UART0 TX / RX | 58 / 59 |
| Header J2 | 0-4, 20-25, 35-49, 58-61 |

The pins come from the user guide and Espressif's
[`esp-board-manager`](https://github.com/espressif/esp-board-manager) definition of this
board. That definition contradicts itself about which of GPIO50-52 carry I2C SDA, SCL
and I2S MCLK; the mapping above was verified on a board.

## Using `esp32-s31-coreboard` in your own firmware

The library isn't on crates.io; depend on it by git:

```toml
[dependencies]
esp32-s31-coreboard = { git = "https://github.com/Hahihula/s31-core-board-examples", features = ["log"] }
```

```rust
let p = esp32_s31_coreboard::board::init();      // HAL at 320 MHz
esp32_s31_coreboard::board::init_logger();       // `log` feature: esp-println on UART0
esp_alloc::heap_allocator!(size: 96 * 1024);     // your heap; plain RAM (see board::init)
```

What the firmware has to bring itself:

- **The esp-hal pins.** Copy the `[patch.crates-io]` section from the root `Cargo.toml`.
  Cargo applies `[patch]` only in the root manifest, so it can't travel with the library.
- **The linker script.** A `build.rs` in the binary crate with
  `println!("cargo:rustc-link-arg=-Tlinkall.x");`; a dependency can't pass it.
- Copy `.cargo/config.toml` and `rust-toolchain.toml` as well.
- **The logger and the heap**: `board::init()` sets up neither.

## Tests and CI

Hardware-independent modules get unit tests that run on a PC. The workspace builds for
the ESP32-S31 by default, so give the host target:

```bash
cargo test -p esp32-s31-coreboard -p esp32-s31-coreboard-examples --lib --target aarch64-apple-darwin
```

GitHub Actions (`.github/workflows/ci.yml`) runs the tests, `cargo fmt --check`, clippy
with warnings as errors and a release build of every demo, on every push and pull request
and once a week (to catch a vanished esp-hal pin).

## Demos

### 00 – Board bring-up

```bash
cargo run --release --bin 00_bringup
```

1. Prints the chip revision and the Wi-Fi station MAC address.
2. Writes a pattern over all 16 MB of PSRAM, reads it back and reports errors and
   throughput.
3. Adds the PSRAM to the heap and allocates 1 MiB from it.
4. Reads the ES8311 codec's chip ID (`83 11`) and version over I2C (SDA GPIO51,
   SCL GPIO50).
5. Scans that I2C bus.

Expected output (verified on a board):

```
INFO - === ESP32-S31-Function-CoreBoard-1 bring-up ===
INFO - Chip revision: v0.0
INFO - Wi-Fi STA MAC: 1c:29:04:..:..:..
INFO - PSRAM: 16384 KiB mapped at 0x50000000
INFO - PSRAM test: 0 bad words, write 245 ms (~65 MiB/s), read 231 ms (~69 MiB/s)
INFO - Heap: 1 MiB allocation at 0x50000000 OK
INFO - ES8311 chip ID [83, 11], version Some(01)
INFO - Scanning the codec I2C bus...
INFO -   0x18: ES8311 audio codec
INFO - I2C scan: 1 device(s)
INFO - Bring-up done.
```

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
