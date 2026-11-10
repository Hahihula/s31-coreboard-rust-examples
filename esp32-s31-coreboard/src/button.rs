//! The BOOT button (GPIO61), the board's only user button.
//!
//! It is a plain digital input, active low. It is also a strapping pin:
//! holding it during reset enters download mode.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    gpio::{Input, InputConfig, Pull},
    peripherals::GPIO61,
};

/// Contact bounce settling time.
const DEBOUNCE: Duration = Duration::from_millis(20);

/// The BOOT button on GPIO61 (pressed = GND).
pub struct BootButton<'d> {
    input: Input<'d>,
}

impl<'d> BootButton<'d> {
    /// Configures GPIO61 as an input with pull-up.
    pub fn new(gpio61: GPIO61<'d>) -> Self {
        let input = Input::new(gpio61, InputConfig::default().with_pull(Pull::Up));
        Self { input }
    }

    /// Whether the button is held right now (not debounced).
    pub fn is_pressed(&self) -> bool {
        self.input.is_low()
    }

    /// Waits for a complete debounced press and returns how long it was held.
    pub async fn next_press(&mut self) -> Duration {
        let pressed_at = loop {
            self.input.wait_for_low().await;
            Timer::after(DEBOUNCE).await;
            if self.input.is_low() {
                break Instant::now();
            }
        };
        loop {
            self.input.wait_for_high().await;
            Timer::after(DEBOUNCE).await;
            if self.input.is_high() {
                return pressed_at.elapsed();
            }
        }
    }
}
