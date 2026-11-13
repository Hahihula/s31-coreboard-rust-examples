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

The ESP32-S31 support in the released esp-rs crates is incomplete (for example, the Wi-Fi
connect hangs in esp-radio 1.0.0-beta.1), so the root `Cargo.toml` pins esp-hal, esp-rtos,
esp-radio, esp-alloc, esp-println, esp-backtrace and esp-bootloader-esp-idf to a single esp-hal `main` commit with `[patch.crates-io]`. The
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
  bus, the ES8311 codec with microphone input and speaker output, Gigabit Ethernet, the RGB LED and the BOOT
  button. Use it in your own firmware (see below).
- **`esp32-s31-coreboard-examples`** (`examples/`): the demos, plus the app-level helpers
  (start-up, Wi-Fi, SNTP, HTTP) that only the demos need, so a firmware using the board doesn't
  pull in the network stack.

| Path | Contents |
|------|----------|
| `esp32-s31-coreboard/src/lib.rs` | Crate docs: what a firmware using the library needs |
| `esp32-s31-coreboard/src/board.rs` | Board pin map, I2C device addresses, `init()` (and `init_gigabit()` with the MPLL at 500 MHz), logger, codec I2C bus, RGB LED setup, speaker amplifier enable, PSRAM mapping (`psram_reserve`, `psram_split`) |
| `esp32-s31-coreboard/src/button.rs` | The BOOT button (GPIO61): debounced presses with hold time |
| `esp32-s31-coreboard/src/ethernet.rs` | Gigabit Ethernet: YT8531 PHY reset, RGMII pins (`rgmii_pins!`), MAC address, EMAC driver with link polling every 500 ms |
| `esp32-s31-coreboard/src/audio.rs` | I2S input (`AudioIn`) and output (`AudioOut`) streams for the ES8311 (MCLK = 256 x fs, 16-bit stereo Philips, DMA ring buffer, restarts after an overflow or underrun) |
| `examples/src/lib.rs`, `demo.rs` | Helper crate root; the `start!` macro the demos begin with |
| `examples/src/net.rs` | Wi-Fi station setup and reconnect task, embassy-net stacks with DHCP for Wi-Fi and Ethernet, SNTP client, clock and DNS helpers |
| `examples/src/http.rs` | Minimal HTTP/1.1: request line, query parameters, `rrggbb` colors, response headers (host-tested) |
| `examples/assets/web_led.html` | The page of demo 05 (HTML + JavaScript, embedded with `include_str!`) |
| `examples/src/sntp.rs`, `time.rs` | SNTP request and reply checks; UTC dates from Unix time (host-tested) |
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

The hardware-independent modules (HTTP
parsing, SNTP packets, dates) have unit tests that run on a PC. The workspace builds for
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

### 01 – RGB LED

```bash
cargo run --release --bin 01_rgb_led
```

Drives the onboard WS2812 addressable LED. GPIO60 feeds it through a BSS138 MOSFET that
shifts the signal to 5 V without inverting it (Espressif's board definition contradicts
itself on that; checked on a board). The signal is generated by RMT channel 0 using the
[`esp-hal-smartled`](https://crates.io/crates/esp-hal-smartled) crate, with gamma
correction from [`smart-leds`](https://crates.io/crates/smart-leds).

It runs four effects:

| Effect | What you see |
|--------|--------------|
| rainbow | Smooth hue cycle, about 4 s per turn |
| breathing | Fades in and out every 3 s, with a different colour each breath |
| color test | Red, green, blue, white and off, one second each. If these don't match the log, the colour order is wrong |
| candle | Warm orange flicker |

The **BOOT** button controls the demo:

- **Short press:** next effect. The first press also turns off auto-advance.
- **Long press (1 s or more):** next brightness level (8, 32, 96, 255; the default is 32).

Until BOOT is pressed for the first time, the effects advance on their own every
10 seconds. Don't hold BOOT while pressing Reset, because that puts the chip into
download mode.

Expected output (verified on a board):

```
INFO - === Demo 01: RGB LED (WS2812 on GPIO60 via RMT) ===
INFO - BOOT: short press = next effect, long press (>= 1 s) = brightness
INFO - effect: rainbow (brightness 32)
INFO - effect: breathing
INFO - effect: color test (red, green, blue, white, off)
INFO -   color test: red
INFO -   color test: green
INFO -   color test: blue
INFO -   color test: white
INFO -   color test: off
INFO - effect: candle
INFO - auto-advance off
INFO - effect: rainbow
INFO - brightness 96
```

### 02 – Microphone

```bash
cargo run --release --bin 02_microphone
```

Records from the onboard microphone through the ES8311 codec and shows the level:

1. Starts I2S as master at 16 kHz, 16-bit stereo Philips, with MCLK (GPIO52) at
   256 × fs = 4.096 MHz, which the codec runs from.
2. Reads the ES8311 chip ID (`83 11`) and sets up its ADC path the way Espressif's
   `esp_codec_dev` driver does: analog microphone, +30 dB PGA, then a digital gain of
   0-42 dB in 6 dB steps (12 dB at start).
3. Prints the RMS and peak level twice a second, in dBFS. The ES8311 is mono and sends
   the same sample in both I2S slots; the demo uses the left one.
4. Uses the RGB LED as a level meter, updated every 50 ms: dark below -60 dBFS, then
   brighter and from green through yellow to red at -10 dBFS.

A **BOOT** press steps the gain up by 6 dB, wrapping from 42 dB back to 0 dB.

On a board at 24 dB, a quiet room read about -61 dBFS and speech close to the board
-15 to -33 dBFS with peaks clipping at full scale, so the demo starts at 12 dB: a quiet
room then reads about -70 dBFS and speech about -36 dBFS with peaks near -17 dBFS.
Expected output (verified on a board):

```
INFO - === Demo 02: microphone (ES8311, 16 kHz) ===
INFO - ES8311 chip ID [83, 11]
INFO - ES8311 running, mic gain 12 dB
INFO - BOOT = next mic gain step (0-42 dB, wraps around)
INFO - mic  -13.4 dBFS (peak    0.0)
INFO - mic  -73.7 dBFS (peak  -59.9)
INFO - mic  -52.3 dBFS (peak  -33.9)
INFO - mic  -36.1 dBFS (peak  -16.8)
INFO - mic  -36.5 dBFS (peak  -20.6)
INFO - mic  -67.2 dBFS (peak  -56.7)
```

The first report includes the codec powering up and reads louder.

The codec driver is the `es8311-async` crate
([source](https://github.com/Hahihula/es8311-async)), re-exported as `esp32_s31_coreboard::es8311`. It is async and follows
Espressif's `esp_codec_dev` driver register for register.

### 03 – Wi-Fi, DHCP, DNS and NTP

```bash
cargo run --release --bin 03_wifi
```

The Wi-Fi credentials are compiled into the firmware from the `WIFI_SSID` and
`WIFI_PASSWORD` environment variables at build time:

```bash
WIFI_SSID='my network' WIFI_PASSWORD='secret' cargo run --release --bin 03_wifi
```

Or export them once in your shell session. Cargo rebuilds automatically when they
change. Leave out `WIFI_PASSWORD`, or
set it to an empty string, for an open network. Only WPA2-Personal and open networks are
supported; WPA3-only networks will not connect. The ESP32-S31 supports 2.4 GHz only.

What the demo does:

1. Starts the Wi-Fi radio (esp-radio) and scans for networks, listing up to 15 with
   signal strength, channel and security type.
2. Connects to `WIFI_SSID`, warning first if the scan did not see that network. The
   station scans all channels before joining, so with several access points of the same
   network it picks the strongest (esp-radio's default fast scan takes the first one
   found). A background task reconnects every 5 s if the connection fails or drops. An attempt that
   gets no answer from the driver within 30 s is cancelled and retried.
3. Gets an IP address, gateway and DNS server over DHCP (embassy-net).
4. Looks up `docs.espressif.com` in DNS.
5. Fetches the time from `pool.ntp.org` with a minimal SNTP request over UDP. Only a reply
   from the server's address that echoes the request's random timestamp and comes from a
   synchronised server (stratum 1-15) is accepted.
6. Every 10 seconds prints the current UTC time (NTP time plus the time elapsed since)
   and the IP address. If NTP failed, it is retried each round instead.

Without `WIFI_SSID` the demo only runs the scan and then prints how to rebuild with
credentials. That is a quick way to check the radio without a network.

To see the Wi-Fi driver's own log while debugging a connection, add
`ESP_LOG=info,esp_radio=debug` in front of the `cargo run` command.

The Wi-Fi driver needs about 60 KiB of internal RAM, so this demo adds a 96 KiB internal
heap on top of the small default one.

Expected output (verified on a board; names and addresses are examples). The
`link_up` and `IPv4` lines come from embassy-net:

```
INFO - === Demo 03: Wi-Fi, DHCP, DNS, NTP ===
INFO - Station configured for 'my network' (WPA2-Personal)
INFO - Scanning...
INFO - Found 14 access point(s):
INFO -    -70 dBm  ch  6  my network                       Some(Wpa2Personal)
INFO -    -60 dBm  ch 11  neighbour                        Some(Wpa2Personal)
INFO -    -38 dBm  ch 11  my network                       Some(Wpa2Personal)
...
INFO - Waiting for DHCP...
INFO - Wi-Fi connecting...
INFO - link_up = true
INFO - Wi-Fi connected to 'my network' (channel 11, RSSI -40 dBm)
INFO - DHCP: address 192.168.1.68/24, gateway Some(192.168.1.1), DNS [192.168.1.1]
INFO - DNS: docs.espressif.com -> [Ipv4(18.166.108.18)]
INFO - NTP: 2026-11-10 13:12:46 UTC (1794316366)
```

### 04 – Ethernet, DHCP, DNS and NTP

```bash
cargo run --release --bin 04_ethernet
```

Plug a network cable from the RJ45 port into a router or switch with a DHCP server.

The ESP32-S31 has an Ethernet MAC (EMAC). On this board it is wired over RGMII to a
Motorcomm YT8531 gigabit PHY, with the RJ45 jack and magnetics behind it. The driver is
esp-hal's `ethernet` module, which also programs the YT8531's 2 ns RGMII clock delays and
retunes the transmit clock when the link speed changes.

**1000 Mbps needs the MPLL at 500 MHz.** The MAC makes the RGMII transmit clock from the
MPLL with an integer divider: 125 MHz for 1000 Mbps, 25 MHz for 100 Mbps. esp-hal runs
the MPLL at 400 MHz by default, which can't be divided to 125 MHz, so it then limits the
link to 100 Mbps and logs a warning. This demo starts with `board::init_gigabit()`, which
sets the MPLL to 500 MHz. PSRAM is clocked from the MPLL too and its default 200 MHz
doesn't divide 500 MHz, so a firmware using both has to map PSRAM with the 250 MHz (or
125 MHz) timing preset.

What the demo does:

1. Prints the Ethernet MAC address: the chip's base MAC (the Wi-Fi station address) + 3,
   as ESP-IDF derives it.
2. Holds the PHY in reset (GPIO7) for 100 ms, waits 300 ms, then starts the EMAC. The PHY
   address is found by scanning the MDIO bus (MDC GPIO5, MDIO GPIO6).
3. Waits for the cable link and logs the negotiated speed and duplex. The PHY is polled
   over MDIO every 500 ms.
4. Gets an address over DHCP, looks up `docs.espressif.com` and fetches the time from
   `pool.ntp.org`, the same way as demo 03.
5. Every 10 seconds prints the UTC time and the IP address. Unplugging the cable logs
   `link down`; plugging it back in brings the link and DHCP back.

Expected output on a gigabit switch (verified on a board; addresses are examples):

```
INFO - === Demo 04: Ethernet, DHCP, DNS, NTP ===
INFO - MAC address 1c:29:04:..:..:..
INFO - Ethernet PHY at MDIO address 0
INFO - Waiting for the cable link...
INFO - Ethernet link up: _1000M, Full duplex
INFO - link_up = true
INFO - Waiting for DHCP...
INFO - DHCP: address 192.168.1.70/24, gateway Some(192.168.1.1), DNS [192.168.1.1]
INFO - DNS: docs.espressif.com -> [Ipv4(18.166.108.18)]
INFO - NTP: 2026-11-10 13:29:19 UTC (1794317359)
INFO - 2026-11-10 13:29:29 UTC | IP 192.168.1.70/24
```

With `board::init()` instead (MPLL at 400 MHz) the same board logs
`MPLL at 400 MHz cannot make the 125 MHz RGMII clock, limiting Ethernet to 100 Mbps` and
links at `_100M`.

### 05 – Web-controlled RGB LED

```bash
cargo run --release --bin 05_web_led
```

A web page with a color picker, preset colors and an Off button that sets the onboard
RGB LED. It is served on **Ethernet and Wi-Fi at the same time**: Ethernet always, Wi-Fi
when `WIFI_SSID` (and `WIFI_PASSWORD`) were set at build time, as in demo 03:

```bash
WIFI_SSID='my network' WIFI_PASSWORD='secret' cargo run --release --bin 05_web_led
```

Open the address from the log (`Ethernet: open http://.../`) in a browser on the same
network. The LED and the circle on the page follow the picker while you drag it. A page
opened in another browser picks up the current color when it loads.

How it works:

- Each network has its own embassy-net stack (from `net.rs`) and two server tasks
  accepting on port 80, so a browser's second connection doesn't wait. A connection
  carries one request and is closed after the response.
- `http.rs` parses just the request line and query string; there is no web framework.
  The page comes from `examples/assets/web_led.html`, embedded with `include_str!`.
- The API is `GET /api/led`, which returns `{"color":"#rrggbb"}`, and
  `GET /api/led?color=rrggbb`, which sets the color first. You can call it with
  `curl`, too: `curl 'http://<address>/api/led?color=ff8000'`.
- A LED task owns the RGB LED and writes each new color, gamma-corrected so it looks
  like the color picked in the browser.
- Like demo 04 it uses `board::init_gigabit()` for 1000 Mbps Ethernet, and a 96 KiB heap
  for the Wi-Fi driver.

Expected output with Wi-Fi only (verified on a board; names and addresses are
examples). Every color change is logged with the network it came from; dragging the
picker sends a stream of them:

```
INFO - === Demo 05: web-controlled RGB LED ===
INFO - Ethernet PHY at MDIO address 0
INFO - Wi-Fi connecting...
INFO - link_up = true
INFO - Wi-Fi connected to 'my network' (channel 11, RSSI -32 dBm)
INFO - Wi-Fi: open http://192.168.1.68/
INFO - Wi-Fi: LED #ff0000
INFO - Wi-Fi: LED #ad7b7b
INFO - Wi-Fi: LED #a97979
```

### 06 – Speaker

```bash
cargo run --release --bin 06_speaker
```

Plays generated sound through the ES8311 DAC and the NS4150B amplifier:

1. Starts I2S as master at 44.1 kHz, 16-bit stereo Philips, with MCLK (GPIO52) at
   256 × fs, and streams silence.
2. Reads the ES8311 chip ID (`83 11`), starts the codec (`init` powers up the DAC muted),
   then sets the DAC volume to -20 dB and unmutes it.
3. Switches the amplifier on (GPIO57) last, so the codec's power-up doesn't pop in the
   speaker, and plays a chime.
4. A small sine synthesiser with a 10 ms fade per note renders the tunes into the I2S
   ring from a task; the RGB LED is lit while a tune plays.

A **BOOT** press plays the next tune: scale, Ode to Joy, chime.

Expected output (verified on a board; the sound is quiet but clearly audible at -20 dB):

```
INFO - === Demo 06: speaker (ES8311, 44.1 kHz) ===
INFO - ES8311 chip ID [83, 11]
INFO - ES8311 DAC running, volume -20 dB
INFO - BOOT = next tune (scale, Ode to Joy, chime)
```

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
