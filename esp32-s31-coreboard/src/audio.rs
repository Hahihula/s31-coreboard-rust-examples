//! I2S input from and output to the ES8311: 16-bit stereo Philips frames. The ES8311 is
//! mono and sends the same ADC sample in both slots (checked on a board). The S31 is master and
//! drives MCLK (GPIO52, 256 x fs), BCLK (GPIO53) and WS (GPIO55); the codec
//! data comes in on GPIO54 and goes out to it on GPIO56.
//!
//! [`AudioIn`] and [`AudioOut`] each take the whole I2S0 peripheral and the
//! shared clock pins, so use one at a time.
//!
//! Same structure as `AudioIn` in the Korvo-1 and VoCat libraries.

use esp_hal::{
    Async,
    dma::{DmaRxStreamBuf, DmaTxStreamBuf},
    dma_rx_stream_buffer, dma_tx_stream_buffer,
    i2s::master::{
        Channels, DataFormat, Error as I2sError, I2s, I2sRxDmaTransfer, I2sTxDmaTransfer, TdmConfig,
    },
    peripherals::{DMA_CH1, GPIO52, GPIO53, GPIO54, GPIO55, GPIO56, I2S0},
    time::Rate,
};

/// Bytes per stereo frame (two 16-bit samples).
const FRAME_BYTES: usize = 4;
/// DMA ring for recording: 8 KiB, about 128 ms at 16 kHz.
const IN_RING_BYTES: usize = 8192;
/// DMA ring for playback: 8 KiB, about 46 ms at 44.1 kHz.
const OUT_RING_BYTES: usize = 8192;

/// Errors from the audio input.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The I2S configuration was rejected.
    Config(esp_hal::i2s::master::ConfigError),
    /// Starting the transfer failed.
    I2s(I2sError),
    /// The DMA stopped (the input ring overflowed or the output ring ran
    /// dry). The stream has been restarted; the next call works again.
    Dma(esp_hal::dma::DmaError),
    /// Restarting after a DMA error failed; the stream is gone.
    Stopped,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Config(e) => write!(f, "I2S configuration rejected: {e:?}"),
            Error::I2s(e) => write!(f, "I2S transfer failed to start: {e:?}"),
            Error::Dma(e) => write!(f, "I2S DMA stopped and was restarted: {e:?}"),
            Error::Stopped => f.write_str("I2S stream stopped"),
        }
    }
}

impl core::error::Error for Error {}

/// The I2S pins for recording; [`i2s_in_pins!`](crate::i2s_in_pins) fills
/// it in from the peripherals.
pub struct I2sInPins<'d> {
    /// Master clock, GPIO52.
    pub mclk: GPIO52<'d>,
    /// Bit clock (SCLK), GPIO53.
    pub bclk: GPIO53<'d>,
    /// Word select (LRCK), GPIO55.
    pub ws: GPIO55<'d>,
    /// Data from the codec (ASDOUT), GPIO54.
    pub din: GPIO54<'d>,
}

/// [`I2sInPins`] taken out of a `Peripherals` variable:
/// `AudioIn::new(p.I2S0, p.DMA_CH1, i2s_in_pins!(p), 16_000)`.
#[macro_export]
macro_rules! i2s_in_pins {
    ($p:ident) => {
        $crate::audio::I2sInPins {
            mclk: $p.GPIO52,
            bclk: $p.GPIO53,
            ws: $p.GPIO55,
            din: $p.GPIO54,
        }
    };
}

/// A running I2S input stream.
pub struct AudioIn<'d> {
    /// `None` only if restarting after an error failed.
    transfer: Option<I2sRxDmaTransfer<'d, Async, DmaRxStreamBuf>>,
    /// Bytes of a frame that was split between two DMA descriptors.
    partial: [u8; FRAME_BYTES],
    partial_len: usize,
}

impl<'d> AudioIn<'d> {
    /// Starts MCLK, BCLK, WS and the receive stream. Initialise the ES8311
    /// after this: it needs the clocks.
    ///
    /// # Errors
    ///
    /// If the I2S configuration is rejected or the transfer can't start.
    pub fn new(
        i2s: I2S0<'d>,
        dma: DMA_CH1<'d>,
        pins: I2sInPins<'d>,
        sample_rate: u32,
    ) -> Result<Self, Error> {
        let config = TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(sample_rate))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO);
        // MCLK = 256 x fs, as the ES8311 clock table expects.
        let i2s = I2s::new(i2s, dma, config)
            .map_err(Error::Config)?
            .with_mclk(pins.mclk)
            .into_async();
        let rx = i2s
            .i2s_rx
            .with_bclk(pins.bclk)
            .with_ws(pins.ws)
            .with_din(pins.din)
            .build();
        let buf = dma_rx_stream_buffer!(IN_RING_BYTES, 1024);
        let transfer = rx.read(buf).map_err(|(e, _, _)| Error::I2s(e))?;
        Ok(Self {
            transfer: Some(transfer),
            partial: [0; FRAME_BYTES],
            partial_len: 0,
        })
    }

    /// Waits for recorded data and passes each frame (left, right) to
    /// `frame`. Call often enough that the ring (~128 ms) never fills.
    ///
    /// # Errors
    ///
    /// If the ring overflowed, the DMA stops. The stream is then restarted
    /// and [`Error::Dma`] returned, so the caller can note the gap; the next
    /// call records again. [`Error::Stopped`] if restarting failed.
    pub async fn read(&mut self, mut frame: impl FnMut(i16, i16)) -> Result<usize, Error> {
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        if let Err(e) = transfer.wait_for_available_async().await {
            self.restart()?;
            return Err(Error::Dma(e));
        }
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        let emit = |f: &[u8; FRAME_BYTES], frame: &mut dyn FnMut(i16, i16)| {
            frame(
                i16::from_le_bytes([f[0], f[1]]),
                i16::from_le_bytes([f[2], f[3]]),
            );
        };
        let mut frames = 0;
        loop {
            let data = transfer.peek();
            if data.is_empty() {
                break;
            }
            let len = data.len();
            let mut rest = data;
            // A descriptor doesn't always end on a frame boundary.
            if self.partial_len > 0 {
                let take = (FRAME_BYTES - self.partial_len).min(rest.len());
                self.partial[self.partial_len..self.partial_len + take]
                    .copy_from_slice(&rest[..take]);
                self.partial_len += take;
                rest = &rest[take..];
                if self.partial_len == FRAME_BYTES {
                    emit(&self.partial, &mut frame);
                    frames += 1;
                    self.partial_len = 0;
                }
            }
            let (whole, tail) = rest.as_chunks::<FRAME_BYTES>();
            for f in whole {
                emit(f, &mut frame);
            }
            frames += whole.len();
            self.partial[self.partial_len..self.partial_len + tail.len()].copy_from_slice(tail);
            self.partial_len += tail.len();
            // Always consume everything, or the ring stops draining.
            transfer.consume(len);
        }
        Ok(frames)
    }

    /// Stops the dead transfer and starts a new one on the same ring. A
    /// frame split across the gap is dropped.
    fn restart(&mut self) -> Result<(), Error> {
        let transfer = self.transfer.take().ok_or(Error::Stopped)?;
        let (rx, buf) = transfer.stop();
        self.partial_len = 0;
        self.transfer = Some(rx.read(buf).map_err(|(e, _, _)| Error::I2s(e))?);
        Ok(())
    }
}

/// The I2S pins for playback; [`i2s_out_pins!`](crate::i2s_out_pins) fills
/// it in from the peripherals.
pub struct I2sOutPins<'d> {
    /// Master clock, GPIO52.
    pub mclk: GPIO52<'d>,
    /// Bit clock (SCLK), GPIO53.
    pub bclk: GPIO53<'d>,
    /// Word select (LRCK), GPIO55.
    pub ws: GPIO55<'d>,
    /// Data to the codec (DSDIN), GPIO56.
    pub dout: GPIO56<'d>,
}

/// [`I2sOutPins`] taken out of a `Peripherals` variable:
/// `AudioOut::new(p.I2S0, p.DMA_CH1, i2s_out_pins!(p), 44_100)`.
#[macro_export]
macro_rules! i2s_out_pins {
    ($p:ident) => {
        $crate::audio::I2sOutPins {
            mclk: $p.GPIO52,
            bclk: $p.GPIO53,
            ws: $p.GPIO55,
            dout: $p.GPIO56,
        }
    };
}

/// A running I2S output stream.
pub struct AudioOut<'d> {
    /// `None` only if restarting after an error failed.
    transfer: Option<I2sTxDmaTransfer<'d, Async, DmaTxStreamBuf>>,
}

impl<'d> AudioOut<'d> {
    /// Starts MCLK, BCLK, WS and a stream of silence. Initialise the ES8311
    /// after this: it needs the clocks.
    ///
    /// # Errors
    ///
    /// If the I2S configuration is rejected or the transfer can't start.
    pub fn new(
        i2s: I2S0<'d>,
        dma: DMA_CH1<'d>,
        pins: I2sOutPins<'d>,
        sample_rate: u32,
    ) -> Result<Self, Error> {
        let config = TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(sample_rate))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO);
        // MCLK = 256 x fs, as the ES8311 clock table expects.
        let i2s = I2s::new(i2s, dma, config)
            .map_err(Error::Config)?
            .with_mclk(pins.mclk)
            .into_async();
        let tx = i2s
            .i2s_tx
            .with_bclk(pins.bclk)
            .with_ws(pins.ws)
            .with_dout(pins.dout)
            .build();
        let mut buf = dma_tx_stream_buffer!(OUT_RING_BYTES, 1024);
        // The stream must be pre-filled before it starts; fill all of it, so
        // there is slack before the first refill.
        while buf.push(&[0u8; 512]) > 0 {}
        let transfer = tx.write(buf).map_err(|(e, _, _)| Error::I2s(e))?;
        Ok(Self {
            transfer: Some(transfer),
        })
    }

    /// Waits until there is room in the ring, then lets `render` fill all
    /// of it, in pieces of up to 256 mono samples. Each sample goes to both
    /// channels (the ES8311 plays the left one).
    ///
    /// Call again before the ring runs dry (about 46 ms at 44.1 kHz). If the
    /// DMA stops anyway, the stream is restarted (and the ring replays its
    /// old contents once) and [`Error::Dma`] is returned so the caller can
    /// log it.
    ///
    /// # Errors
    ///
    /// [`Error::Dma`] after a restart, [`Error::Stopped`] if restarting failed.
    pub async fn fill(&mut self, mut render: impl FnMut(&mut [i16])) -> Result<(), Error> {
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        if let Err(e) = transfer.wait_for_available_async().await {
            self.restart()?;
            return Err(Error::Dma(e));
        }
        let mut mono = [0i16; 256];
        while transfer.available_bytes() >= FRAME_BYTES {
            transfer.push_with(|buf| {
                let frames = (buf.len() / FRAME_BYTES).min(mono.len());
                render(&mut mono[..frames]);
                for (frame, s) in buf
                    .as_chunks_mut::<FRAME_BYTES>()
                    .0
                    .iter_mut()
                    .zip(&mono[..frames])
                {
                    let [lo, hi] = s.to_le_bytes();
                    frame.copy_from_slice(&[lo, hi, lo, hi]);
                }
                frames * FRAME_BYTES
            });
        }
        Ok(())
    }

    /// Stops the dead transfer and starts a new one on the same ring.
    fn restart(&mut self) -> Result<(), Error> {
        let transfer = self.transfer.take().ok_or(Error::Stopped)?;
        let (tx, buf) = transfer.stop();
        self.transfer = Some(tx.write(buf).map_err(|(e, _, _)| Error::I2s(e))?);
        Ok(())
    }
}
