//! Start-up every demo shares.

/// Starts a demo: HAL at full clock, logger, an internal heap of
/// `heap` bytes if given (plain RAM, see `board::init`), esp-rtos on TIMG0,
/// then logs the title. Binds the peripherals to `$p`.
///
/// `init = ...` replaces the default `board::init()`, e.g. with
/// `board::init_gigabit()`.
///
/// ```ignore
/// esp32_s31_coreboard_examples::start!(p, "Demo 01: RGB LED");
/// esp32_s31_coreboard_examples::start!(p, "Demo 03: Wi-Fi", heap = 96 * 1024);
/// esp32_s31_coreboard_examples::start!(p, "Demo 04: Ethernet", init = board::init_gigabit());
/// ```
#[macro_export]
macro_rules! start {
    ($p:ident, $title:expr, init = $init:expr $(, heap = $size:expr)?) => {
        let $p = $init;
        esp32_s31_coreboard::board::init_logger();
        $( esp_alloc::heap_allocator!(size: $size); )?
        let timg0 = esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        esp_rtos::start(timg0.timer0);
        log::info!("=== {} ===", $title);
    };
    ($p:ident, $title:expr $(, heap = $size:expr)?) => {
        $crate::start!($p, $title, init = esp32_s31_coreboard::board::init() $(, heap = $size)?);
    };
}
