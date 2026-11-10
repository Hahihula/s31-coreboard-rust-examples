//! Board-level constants and common initialisation.
//!
//! | Function                  | GPIO                         |
//! |---------------------------|------------------------------|
//! | ES8311 I2C SDA / SCL      | 51 / 50                      |
//! | ES8311 I2S MCLK           | 52                           |
//! | ES8311 I2S BCLK / WS      | 53 / 55                      |
//! | ES8311 I2S DOUT / DIN     | 56 (to codec) / 54 (from it) |
//! | NS4150B speaker PA enable | 57 (active high)             |
//! | RGB LED (WS2812)          | 60                           |
//! | BOOT button               | 61 (active low)              |
//! | Ethernet PHY MDC / MDIO   | 5 / 6                        |
//! | Ethernet PHY reset        | 7                            |
//! | RGMII TXD0-3 / TX_CTL     | 8-11 / 12                    |
//! | RGMII TX_CLK / RX_CLK     | 13 / 14                      |
//! | RGMII RX_CTL / RXD3-0     | 15 / 16-19                   |
//! | UART0 TX / RX             | 58 / 59                      |
//!
//! Header J2 breaks out GPIO 0-4, 20-25 (SDIO names), 35-49, 58-61.
//!
//! Espressif's board definition contradicts itself about which of GPIO50-52
//! carry SDA, SCL and MCLK; the mapping above was verified on a board (the
//! ES8311 answers its chip ID on SDA 51 / SCL 50).

use esp_hal::{
    Async,
    clock::CpuClock,
    i2c::master::{Config as I2cConfig, ConfigError, I2c, Instance as I2cInstance},
    peripherals::{GPIO50, GPIO51, PSRAM, Peripherals},
    psram::Psram,
    time::Rate,
};

/// 7-bit addresses of the devices on the I2C bus.
pub mod i2c_addr {
    /// ES8311 audio codec (CE pin low; the schematic lists 8-bit 0x30).
    pub const ES8311: u8 = 0x18;
}

/// Initialises the HAL with the maximum CPU clock (320 MHz).
///
/// Logging and the heap are the application's choice:
///
/// ```ignore
/// let p = esp32_s31_coreboard::board::init();
/// esp32_s31_coreboard::board::init_logger(); // with the `log` feature
/// esp_alloc::heap_allocator!(size: 96 * 1024);
/// ```
///
/// Use plain RAM for the internal heap, not the template's
/// `#[esp_hal::ram(reclaimed)]`: on the ESP32-S31 the reclaimed bootloader
/// RAM overlaps buffers the ROM may still use after boot. esp-alloc takes
/// at most three regions in all, PSRAM included.
///
/// The caller still has to start `esp_rtos` with a `TIMG0` timer.
#[must_use]
pub fn init() -> Peripherals {
    esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()))
}

/// Logs through `esp-println` on UART0 (the board's USB-to-UART port), at
/// the level in the `ESP_LOG` environment variable at build time.
#[cfg(feature = "log")]
pub fn init_logger() {
    esp_println::logger::init_logger_from_env();
}

/// I2C clock of the codec bus.
pub const I2C_FREQ: Rate = Rate::from_khz(100);

/// Creates the async I2C master for the codec bus.
///
/// # Errors
///
/// If the configuration is rejected.
pub fn i2c<'d>(
    i2c: impl I2cInstance + 'd,
    sda: GPIO51<'d>,
    scl: GPIO50<'d>,
) -> Result<I2c<'d, Async>, ConfigError> {
    let config = I2cConfig::default().with_frequency(I2C_FREQ);
    Ok(I2c::new(i2c, config)?
        .with_sda(sda)
        .with_scl(scl)
        .into_async())
}

/// Maps the 16 MB PSRAM and hands the first `reserve` bytes (rounded up to a
/// 64-byte cache line) to the caller, e.g. as a DMA buffer. The rest of
/// PSRAM stays unused.
///
/// Call at most once (and not together with [`psram_split`]); taking
/// `PSRAM` by value enforces that.
pub fn psram_reserve(psram: PSRAM<'static>, reserve: usize) -> &'static mut [u8] {
    psram_map(psram, reserve, false)
}

/// Like [`psram_reserve`], but adds the rest of PSRAM to the heap.
///
/// Call at most once (and not together with [`psram_reserve`]); taking
/// `PSRAM` by value enforces that.
pub fn psram_split(psram: PSRAM<'static>, reserve: usize) -> &'static mut [u8] {
    psram_map(psram, reserve, true)
}

fn psram_map(psram: PSRAM<'static>, reserve: usize, rest_to_heap: bool) -> &'static mut [u8] {
    let psram = Psram::new(psram, Default::default());
    let (start, size) = psram.raw_parts();
    let reserve = reserve.next_multiple_of(64).min(size);

    // SAFETY: the PSRAM mapping is valid for `size` bytes for the rest of the
    // program, nothing else uses it, and the heap only gets the part after
    // `reserve`.
    unsafe {
        if rest_to_heap && size > reserve {
            esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
                start.add(reserve),
                size - reserve,
                esp_alloc::MemoryCapability::External.into(),
            ));
        }
        core::slice::from_raw_parts_mut(start, reserve)
    }
}
