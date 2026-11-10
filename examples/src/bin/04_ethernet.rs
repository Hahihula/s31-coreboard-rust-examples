//! Demo 04: Gigabit Ethernet (YT8531 PHY over RGMII), DHCP, DNS and NTP.
//!
//! Runs the MPLL at 500 MHz for the gigabit RGMII clock, resets the PHY,
//! starts the EMAC, waits for a cable link (logging the negotiated speed), gets an address over DHCP, resolves a host name and
//! fetches the time over NTP. Afterwards it prints the time and address every
//! 10 seconds. Unplugging and replugging the cable is logged and recovered.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::efuse::MacAddress;
use esp32_s31_coreboard::{board, ethernet, rgmii_pins};
use esp32_s31_coreboard_examples::net;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const DNS_TEST_HOST: &str = "docs.espressif.com";

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // MPLL at 500 MHz, so the RGMII transmit clock can run at 125 MHz (1000 Mbps).
    esp32_s31_coreboard_examples::start!(
        p,
        "Demo 04: Ethernet, DHCP, DNS, NTP",
        init = board::init_gigabit()
    );

    info!(
        "MAC address {}",
        MacAddress::new_eui48(ethernet::mac_address())
    );
    let eth = ethernet::new(p.ETH, p.GPIO7, rgmii_pins!(p))
        .await
        .expect("Ethernet init (is the PHY answering on MDIO?)");
    let stack = net::start_ethernet(&spawner, eth);

    info!("Waiting for the cable link...");
    stack.wait_link_up().await;
    info!("Waiting for DHCP...");
    stack.wait_config_up().await;
    net::print_config(stack);
    net::check_dns(stack, DNS_TEST_HOST).await;
    net::log_time_forever(stack).await
}
