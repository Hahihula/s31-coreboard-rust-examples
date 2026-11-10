//! Driver for the Everest ES8311 mono audio codec (ADC path: microphone).
//!
//! Ported from Espressif's `esp_codec_dev` ES8311 driver (Apache-2.0,
//! `components/esp_codec_dev/device/es8311/es8311.c`): the same register
//! sequence as `es8311_open`, `es8311_set_fs` and `es8311_start` for an
//! analog microphone, with the codec as I2S slave and MCLK on its MCLK pin.
//!
//! Generic over any `embedded-hal-async` I2C bus, so it also builds (and is
//! unit-tested) on a PC. Pass the bus by `&mut` to share it.
//!
//! ```ignore
//! let mut codec = Es8311::new(&mut i2c);
//! codec.init(Config { sample_rate: 16_000, mclk_hz: 256 * 16_000 }).await?;
//! codec.set_mic_gain(MicGain::Db24).await?;
//! ```

use embedded_hal_async::i2c::I2c;

/// 7-bit I2C address with the CE pin low (as on the CoreBoard).
pub const ADDRESS: u8 = 0x18;

/// Value of the chip ID registers 0xFD and 0xFE.
pub const CHIP_ID: [u8; 2] = [0x83, 0x11];

/// Register addresses, named as in the datasheet and Espressif's driver.
mod reg {
    pub const RESET: u8 = 0x00;
    pub const CLK_MANAGER_01: u8 = 0x01;
    pub const CLK_MANAGER_02: u8 = 0x02;
    pub const CLK_MANAGER_03: u8 = 0x03;
    pub const CLK_MANAGER_04: u8 = 0x04;
    pub const CLK_MANAGER_05: u8 = 0x05;
    pub const CLK_MANAGER_06: u8 = 0x06;
    pub const CLK_MANAGER_07: u8 = 0x07;
    pub const CLK_MANAGER_08: u8 = 0x08;
    /// Serial data port in (to the DAC).
    pub const SDP_IN: u8 = 0x09;
    /// Serial data port out (from the ADC).
    pub const SDP_OUT: u8 = 0x0A;
    pub const SYSTEM_0B: u8 = 0x0B;
    pub const SYSTEM_0C: u8 = 0x0C;
    pub const SYSTEM_0D: u8 = 0x0D;
    pub const SYSTEM_0E: u8 = 0x0E;
    pub const SYSTEM_10: u8 = 0x10;
    pub const SYSTEM_11: u8 = 0x11;
    pub const SYSTEM_13: u8 = 0x13;
    /// Analog PGA and input selection.
    pub const SYSTEM_14: u8 = 0x14;
    pub const ADC_15: u8 = 0x15;
    /// ADC gain scale (0-42 dB in 6 dB steps).
    pub const ADC_16: u8 = 0x16;
    /// ADC digital volume.
    pub const ADC_17: u8 = 0x17;
    pub const ADC_1B: u8 = 0x1B;
    pub const ADC_1C: u8 = 0x1C;
    pub const DAC_37: u8 = 0x37;
    pub const GPIO_44: u8 = 0x44;
    pub const GP_45: u8 = 0x45;
    pub const CHIP_ID1: u8 = 0xFD;
    pub const CHIP_ID2: u8 = 0xFE;
    pub const CHIP_VERSION: u8 = 0xFF;
}

/// Errors from the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The I2C transfer failed.
    I2c(E),
    /// No clock coefficients for this MCLK and sample rate (see
    /// [`Config`]).
    UnsupportedClock,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::I2c(e)
    }
}

/// Clock setup. The codec is I2S slave and runs from MCLK; the I2S data
/// format is 16-bit Philips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// MCLK frequency in Hz. Supported: `256 * sample_rate` for 8, 11.025,
    /// 12, 16, 22.05, 24, 32, 44.1, 48, 64 and 96 kHz.
    pub mclk_hz: u32,
}

/// ADC gain scale (register 0x16), on top of the fixed +30 dB analog PGA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum MicGain {
    /// 0 dB.
    Db0 = 0,
    /// 6 dB.
    Db6,
    /// 12 dB.
    Db12,
    /// 18 dB.
    Db18,
    /// 24 dB.
    Db24,
    /// 30 dB.
    Db30,
    /// 36 dB.
    Db36,
    /// 42 dB.
    Db42,
}

impl MicGain {
    /// All steps in ascending order.
    pub const ALL: [MicGain; 8] = [
        MicGain::Db0,
        MicGain::Db6,
        MicGain::Db12,
        MicGain::Db18,
        MicGain::Db24,
        MicGain::Db30,
        MicGain::Db36,
        MicGain::Db42,
    ];

    /// The gain in dB.
    #[must_use]
    pub fn db(self) -> u8 {
        self as u8 * 6
    }

    /// The next higher step, wrapping from 42 dB back to 0 dB.
    #[must_use]
    pub fn next_wrapping(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }
}

/// Clock divider coefficients for one MCLK / sample rate pair, as in
/// Espressif's `coeff_div` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Coeff {
    mclk: u32,
    rate: u32,
    pre_div: u8,
    /// Pre-multiplier: 1, 2, 4 or 8.
    pre_multi: u8,
    adc_div: u8,
    dac_div: u8,
    /// 0 = single speed, 1 = double speed.
    fs_mode: u8,
    lrck_h: u8,
    lrck_l: u8,
    bclk_div: u8,
    adc_osr: u8,
    dac_osr: u8,
}

/// The rows of Espressif's table with MCLK = 256 x fs, the ratio esp-hal's
/// I2S master produces for 16- and 32-bit data.
#[rustfmt::skip]
const COEFFS: [Coeff; 11] = [
    //    mclk         rate   pre_div  multi  adc_div dac_div fs   lrck_h  lrck_l bclk_div adc_osr dac_osr
    c( 2_048_000,   8_000,  1, 2, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x20),
    c( 2_822_400,  11_025,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x20),
    c( 3_072_000,  12_000,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x20),
    c( 4_096_000,  16_000,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x20),
    c( 5_644_800,  22_050,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x10),
    c( 6_144_000,  24_000,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x10),
    c( 8_192_000,  32_000,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x10),
    c(11_289_600,  44_100,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x10),
    c(12_288_000,  48_000,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x10),
    c(16_384_000,  64_000,  1, 1, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x10),
    c(24_576_000,  96_000,  2, 2, 1, 1, 0, 0x00, 0xFF, 4, 0x10, 0x10),
];

#[allow(clippy::too_many_arguments, reason = "one table row")]
const fn c(
    mclk: u32,
    rate: u32,
    pre_div: u8,
    pre_multi: u8,
    adc_div: u8,
    dac_div: u8,
    fs_mode: u8,
    lrck_h: u8,
    lrck_l: u8,
    bclk_div: u8,
    adc_osr: u8,
    dac_osr: u8,
) -> Coeff {
    Coeff {
        mclk,
        rate,
        pre_div,
        pre_multi,
        adc_div,
        dac_div,
        fs_mode,
        lrck_h,
        lrck_l,
        bclk_div,
        adc_osr,
        dac_osr,
    }
}

fn coeff(mclk: u32, rate: u32) -> Option<&'static Coeff> {
    COEFFS.iter().find(|c| c.mclk == mclk && c.rate == rate)
}

/// The ES8311 on an I2C bus.
pub struct Es8311<I2C> {
    i2c: I2C,
}

impl<I2C: I2c> Es8311<I2C> {
    /// Wraps the bus; talks to [`ADDRESS`]. Nothing is sent yet.
    pub fn new(i2c: I2C) -> Self {
        Self { i2c }
    }

    /// Gives the bus back.
    pub fn release(self) -> I2C {
        self.i2c
    }

    /// Reads the chip ID; [`CHIP_ID`] for an ES8311.
    ///
    /// # Errors
    ///
    /// If the I2C transfer fails.
    pub async fn chip_id(&mut self) -> Result<[u8; 2], Error<I2C::Error>> {
        Ok([
            self.read(reg::CHIP_ID1).await?,
            self.read(reg::CHIP_ID2).await?,
        ])
    }

    /// Reads the chip version register.
    ///
    /// # Errors
    ///
    /// If the I2C transfer fails.
    pub async fn chip_version(&mut self) -> Result<u8, Error<I2C::Error>> {
        Ok(self.read(reg::CHIP_VERSION).await?)
    }

    /// Configures the codec and starts the ADC (microphone) path at
    /// [`MicGain::Db24`]. MCLK, BCLK and WS must already be running.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedClock`] if the clock pair isn't in the table
    /// (checked before anything is written), or an I2C error.
    pub async fn init(&mut self, config: Config) -> Result<(), Error<I2C::Error>> {
        let coeff = coeff(config.mclk_hz, config.sample_rate).ok_or(Error::UnsupportedClock)?;
        self.open().await?;
        self.set_format().await?;
        self.set_clock(coeff).await?;
        Ok(self.start_adc().await?)
    }

    /// Sets the ADC gain scale.
    ///
    /// # Errors
    ///
    /// If the I2C transfer fails.
    pub async fn set_mic_gain(&mut self, gain: MicGain) -> Result<(), Error<I2C::Error>> {
        Ok(self.write(reg::ADC_16, gain as u8).await?)
    }

    /// Default register setup (`es8311_open`): slave mode, MCLK from the
    /// MCLK pin, not inverted, analog references on.
    async fn open(&mut self) -> Result<(), I2C::Error> {
        if self.read(reg::SYSTEM_0D).await? != 0xFA {
            self.write(reg::SYSTEM_0D, 0xFA).await?;
        }
        // Better I2C noise immunity. Espressif writes it twice because the
        // first write after power-up occasionally fails.
        self.write(reg::GPIO_44, 0x08).await?;
        self.write(reg::GPIO_44, 0x08).await?;

        for (r, v) in [
            (reg::CLK_MANAGER_01, 0x30),
            (reg::CLK_MANAGER_02, 0x00),
            (reg::CLK_MANAGER_03, 0x10),
            (reg::ADC_16, 0x24),
            (reg::CLK_MANAGER_04, 0x10),
            (reg::CLK_MANAGER_05, 0x00),
            (reg::SYSTEM_0B, 0x00),
            (reg::SYSTEM_0C, 0x00),
            (reg::SYSTEM_10, 0x1F),
            (reg::SYSTEM_11, 0x7F),
            // Power on, slave mode (bit 6 clear).
            (reg::RESET, 0x80),
            // All clocks on, MCLK from the MCLK pin, not inverted.
            (reg::CLK_MANAGER_01, 0x3F),
        ] {
            self.write(r, v).await?;
        }
        // SCLK not inverted.
        self.modify(reg::CLK_MANAGER_06, |v| v & !0x20).await?;

        self.write(reg::SYSTEM_13, 0x10).await?;
        self.write(reg::ADC_1B, 0x0A).await?;
        self.write(reg::ADC_1C, 0x6A).await?;
        // No DAC echo reference on the ADC's right channel.
        self.write(reg::GPIO_44, 0x08).await
    }

    /// 16-bit Philips I2S on both serial ports (`es8311_set_fs`).
    async fn set_format(&mut self) -> Result<(), I2C::Error> {
        for r in [reg::SDP_IN, reg::SDP_OUT] {
            // Word length 16 bits (bits 4:2 = 0b011), format I2S (bits 1:0 = 0).
            self.modify(r, |v| (v | 0x0C) & 0xFC).await?;
        }
        Ok(())
    }

    /// Clock dividers for the sample rate (`es8311_config_sample`).
    async fn set_clock(&mut self, c: &Coeff) -> Result<(), I2C::Error> {
        let multi = match c.pre_multi {
            2 => 1,
            4 => 2,
            8 => 3,
            _ => 0,
        };
        self.modify(reg::CLK_MANAGER_02, |v| {
            (v & 0x07) | ((c.pre_div - 1) << 5) | (multi << 3)
        })
        .await?;
        self.write(
            reg::CLK_MANAGER_05,
            ((c.adc_div - 1) << 4) | (c.dac_div - 1),
        )
        .await?;
        self.modify(reg::CLK_MANAGER_03, |v| {
            (v & 0x80) | (c.fs_mode << 6) | c.adc_osr
        })
        .await?;
        self.modify(reg::CLK_MANAGER_04, |v| (v & 0x80) | c.dac_osr)
            .await?;
        self.modify(reg::CLK_MANAGER_07, |v| (v & 0xC0) | c.lrck_h)
            .await?;
        self.write(reg::CLK_MANAGER_08, c.lrck_l).await?;
        let bclk = if c.bclk_div < 19 {
            c.bclk_div - 1
        } else {
            c.bclk_div
        };
        self.modify(reg::CLK_MANAGER_06, |v| (v & 0xE0) | bclk)
            .await
    }

    /// Powers up the ADC path for an analog microphone (`es8311_start`).
    async fn start_adc(&mut self) -> Result<(), I2C::Error> {
        self.write(reg::RESET, 0x80).await?;
        self.write(reg::CLK_MANAGER_01, 0x3F).await?;
        // Bit 6 set = serial port muted; unmute both.
        self.modify(reg::SDP_IN, |v| v & !0x40).await?;
        self.modify(reg::SDP_OUT, |v| v & !0x40).await?;
        for (r, v) in [
            // ADC digital volume 0 dB.
            (reg::ADC_17, 0xBF),
            (reg::SYSTEM_0E, 0x02),
            // Analog mic on MIC1P, PGA +30 dB, digital mic off.
            (reg::SYSTEM_14, 0x1A),
            (reg::SYSTEM_0D, 0x01),
            (reg::ADC_15, 0x40),
            (reg::DAC_37, 0x08),
            (reg::GP_45, 0x00),
        ] {
            self.write(r, v).await?;
        }
        Ok(())
    }

    async fn read(&mut self, reg: u8) -> Result<u8, I2C::Error> {
        let mut value = [0];
        self.i2c.write_read(ADDRESS, &[reg], &mut value).await?;
        Ok(value[0])
    }

    async fn write(&mut self, reg: u8, value: u8) -> Result<(), I2C::Error> {
        self.i2c.write(ADDRESS, &[reg, value]).await
    }

    async fn modify(&mut self, reg: u8, f: impl FnOnce(u8) -> u8) -> Result<(), I2C::Error> {
        let value = self.read(reg).await?;
        self.write(reg, f(value)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_256_fs() {
        for c in &COEFFS {
            assert_eq!(c.mclk, 256 * c.rate, "{} Hz", c.rate);
        }
    }

    #[test]
    fn finds_16k() {
        let c = coeff(4_096_000, 16_000).unwrap();
        assert_eq!((c.pre_div, c.pre_multi, c.adc_osr), (1, 1, 0x10));
    }

    #[test]
    fn rejects_unknown_clock() {
        assert!(coeff(12_288_000, 16_000).is_none());
        assert!(coeff(256 * 88_200, 88_200).is_none());
    }

    #[test]
    fn gain_steps() {
        assert_eq!(MicGain::Db0.db(), 0);
        assert_eq!(MicGain::Db24.db(), 24);
        assert_eq!(MicGain::Db42.db(), 42);
        assert_eq!(MicGain::Db36.next_wrapping(), MicGain::Db42);
        assert_eq!(MicGain::Db42.next_wrapping(), MicGain::Db0);
        for (i, g) in MicGain::ALL.iter().enumerate() {
            assert_eq!(*g as usize, i);
        }
    }
}
