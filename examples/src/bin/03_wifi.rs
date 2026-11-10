//! Demo 03: Wi-Fi station, DHCP, DNS and NTP.
//!
//! Scans for access points, connects to the network given at build time in
//! `WIFI_SSID` / `WIFI_PASSWORD`, obtains an address over DHCP, resolves a host
//! name and fetches the current time over NTP. Afterwards it prints the time
//! and address every 10 seconds and reconnects automatically if the link drops.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_radio::wifi::{WifiController, scan::ScanConfig};
use esp32_s31_coreboard_examples::net;
use log::{error, info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

const DNS_TEST_HOST: &str = "docs.espressif.com";

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // The Wi-Fi driver needs about 60 KiB of internal RAM.
    esp32_s31_coreboard_examples::start!(p, "Demo 03: Wi-Fi, DHCP, DNS, NTP", heap = 96 * 1024);

    let Some((ssid, password)) = net::credentials() else {
        // Without credentials only the scan is useful.
        let (mut controller, _interface) = net::init_wifi(p.WIFI, None).expect("Wi-Fi init");
        scan(&mut controller, None).await;
        error!("WIFI_SSID was not set at build time; rebuild with:");
        error!("  WIFI_SSID=<name> WIFI_PASSWORD=<password> cargo run --release --bin 03_wifi");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    };

    let config = net::station_config(ssid, password).expect("SSID or password too long");
    let (mut controller, interface) = net::init_wifi(p.WIFI, Some(config)).expect("Wi-Fi init");
    info!(
        "Station configured for '{ssid}' ({})",
        if password.is_empty() {
            "open"
        } else {
            "WPA2-Personal"
        }
    );
    scan(&mut controller, Some(ssid)).await;

    let stack = net::start_wifi(&spawner, controller, interface);

    info!("Waiting for DHCP...");
    stack.wait_config_up().await;
    net::print_config(stack);
    net::check_dns(stack, DNS_TEST_HOST).await;
    net::log_time_forever(stack).await
}

/// Lists nearby access points and warns when `wanted` is not among them.
#[allow(
    clippy::large_stack_frames,
    reason = "the esp-radio scan future is ~1 KiB; awaited once from main"
)]
async fn scan(controller: &mut WifiController<'_>, wanted: Option<&str>) {
    info!("Scanning...");
    match controller
        .scan_async(&ScanConfig::default().with_max(15))
        .await
    {
        Ok(aps) => {
            info!("Found {} access point(s):", aps.len());
            for ap in &aps {
                info!(
                    "  {:>4} dBm  ch {:>2}  {:<32} {:?}",
                    ap.signal_strength,
                    ap.channel,
                    ap.ssid.as_str(),
                    ap.auth_method
                );
            }
            if let Some(wanted) = wanted
                && !aps.iter().any(|ap| ap.ssid.as_str() == wanted)
            {
                warn!("'{wanted}' was not found in the scan (SSIDs are case-sensitive)");
            }
        }
        Err(e) => warn!("scan failed: {e:?}"),
    }
}
