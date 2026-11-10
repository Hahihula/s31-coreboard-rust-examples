//! Demo 02: the onboard microphone (ES8311 codec over I2S, 16 kHz).
//!
//! Starts the I2S clocks (MCLK included), checks the codec's chip ID,
//! configures its ADC and prints the microphone level (RMS and peak, dBFS)
//! twice a second. The RGB LED works as a level meter: dark when quiet, green to red
//! as it gets louder. A BOOT press steps the gain through 0-42 dB.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp32_s31_coreboard::{
    audio::{AudioIn, Error},
    board::{self, RgbLed},
    button::BootButton,
    es8311::{self, Es8311, MicGain},
    i2s_in_pins,
};
use log::{error, info, warn};
use smart_leds::{
    RGB8, SmartLedsWriteAsync, brightness, gamma,
    hsv::{Hsv, hsv2rgb},
};

esp_bootloader_esp_idf::esp_app_desc!();

const SAMPLE_RATE: u32 = 16_000;
/// Samples per LED update (50 ms).
const METER_BLOCK: usize = SAMPLE_RATE as usize / 20;
/// LED updates per log line (0.5 s).
const BLOCKS_PER_LOG: usize = 10;
/// Gain at start. At 24 dB speech close to the board clips; at 12 dB a
/// quiet room reads about -73 dBFS and speech peaks stay below full scale.
const START_GAIN: MicGain = MicGain::Db12;
/// Level range shown on the LED: dark at or below `QUIET`, red at `LOUD`.
const QUIET_DBFS: f32 = -60.0;
const LOUD_DBFS: f32 = -10.0;

/// RMS and peak level, in dBFS.
#[derive(Clone, Copy)]
struct Levels {
    rms: f32,
    peak: f32,
}

static LEVELS: Signal<CriticalSectionRawMutex, Levels> = Signal::new();

/// Sums squares and the peak of samples until taken.
#[derive(Default)]
struct Meter {
    sum_sq: f32,
    peak: i32,
    count: usize,
}

impl Meter {
    fn add(&mut self, sample: i16) {
        let v = f32::from(sample);
        self.sum_sq += v * v;
        self.peak = self.peak.max(i32::from(sample).abs());
        self.count += 1;
    }

    /// The levels since the last call, and a fresh start.
    fn take(&mut self) -> Levels {
        let n = self.count.max(1) as f32;
        let levels = Levels {
            rms: dbfs(libm::sqrtf(self.sum_sq / n)),
            peak: dbfs(self.peak as f32),
        };
        *self = Self::default();
        levels
    }
}

fn dbfs(v: f32) -> f32 {
    20.0 * libm::log10f(v.max(1.0) / 32768.0)
}

/// LED color for a level: off below `QUIET_DBFS`, then brighter and from
/// green through yellow to red up to `LOUD_DBFS`.
fn meter_color(level_dbfs: f32) -> RGB8 {
    let x = ((level_dbfs - QUIET_DBFS) / (LOUD_DBFS - QUIET_DBFS)).clamp(0.0, 1.0);
    hsv2rgb(Hsv {
        hue: (85.0 * (1.0 - x)) as u8, // 85 = green, 0 = red
        sat: 255,
        val: (255.0 * x) as u8,
    })
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp32_s31_coreboard_examples::start!(p, "Demo 02: microphone (ES8311, 16 kHz)");

    // I2S first: the codec runs from MCLK and needs it before setup.
    let led = board::rgb_led(p.RMT, p.GPIO60).expect("RMT LED init");
    match AudioIn::new(p.I2S0, p.DMA_CH1, i2s_in_pins!(p), SAMPLE_RATE) {
        Ok(input) => spawner.spawn(mic_task(input, led).expect("spawn mic task")),
        Err(e) => error!("I2S init failed: {e}"),
    }
    Timer::after(Duration::from_millis(20)).await;

    let mut i2c = board::i2c(p.I2C0, p.GPIO51, p.GPIO50).expect("I2C config is valid");
    let mut codec = Es8311::new(&mut i2c);
    match codec.chip_id().await {
        Ok(es8311::CHIP_ID) => info!("ES8311 chip ID {:02X?}", es8311::CHIP_ID),
        Ok(id) => warn!("unexpected chip ID {id:02X?}"),
        Err(e) => error!("ES8311 did not answer: {e:?}"),
    }
    let config = es8311::Config {
        sample_rate: SAMPLE_RATE,
        mclk_hz: 256 * SAMPLE_RATE,
    };
    let mut gain = START_GAIN;
    let started = match codec.init(config).await {
        Ok(()) => codec.set_mic_gain(gain).await,
        Err(e) => Err(e),
    };
    match started {
        Ok(()) => info!("ES8311 running, mic gain {} dB", gain.db()),
        Err(e) => error!("ES8311 init failed: {e:?}"),
    }
    info!("BOOT = next mic gain step (0-42 dB, wraps around)");

    let mut button = BootButton::new(p.GPIO61);
    loop {
        match select(LEVELS.wait(), button.next_press()).await {
            Either::First(l) => info!("mic {:6.1} dBFS (peak {:6.1})", l.rms, l.peak),
            Either::Second(_) => {
                gain = gain.next_wrapping();
                match codec.set_mic_gain(gain).await {
                    Ok(()) => info!("mic gain {} dB", gain.db()),
                    Err(e) => warn!("gain change failed: {e:?}"),
                }
            }
        }
    }
}

/// Drains the I2S ring (it holds ~128 ms), drives the LED meter every 50 ms
/// and reports levels every 0.5 s.
#[embassy_executor::task]
async fn mic_task(mut input: AudioIn<'static>, mut led: RgbLed<'static>) {
    let mut meter = Meter::default();
    let mut log = Meter::default();
    let mut blocks = 0;
    loop {
        let mut led_level = None;
        let result = input
            // The ES8311 is mono and sends the same sample in both slots.
            .read(|left, _right| {
                meter.add(left);
                log.add(left);
                if meter.count == METER_BLOCK {
                    led_level = Some(meter.take().rms);
                    blocks += 1;
                    if blocks == BLOCKS_PER_LOG {
                        LEVELS.signal(log.take());
                        blocks = 0;
                    }
                }
            })
            .await;
        if let Some(level) = led_level {
            let color = gamma([meter_color(level)].into_iter());
            if let Err(e) = led.write(brightness(color, 96)).await {
                warn!("LED write failed: {e:?}");
            }
        }
        match result {
            Ok(_) => {}
            // AudioIn restarted the stream; read on at once.
            Err(Error::Dma(e)) => warn!("mic ring overflowed, restarted: {e:?}"),
            Err(e) => {
                error!("mic stream stopped: {e}");
                return;
            }
        }
    }
}
