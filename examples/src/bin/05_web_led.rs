//! Demo 05: a web page that sets the color of the onboard RGB LED.
//!
//! Serves the page on Ethernet and, if `WIFI_SSID` was set at build time,
//! on Wi-Fi at the same time. Open the address from the log in a browser and
//! pick a color; every browser shows the color the LED has now.
//!
//! HTTP API: `GET /` is the page, `GET /api/led` returns
//! `{"color":"#rrggbb"}`, `GET /api/led?color=rrggbb` sets the color first.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::Cell;

use embassy_executor::Spawner;
use embassy_net::{
    Stack,
    tcp::{Error as TcpError, TcpSocket},
};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};
use embassy_time::Duration;
use embedded_io_async::Write;
use esp_backtrace as _;
use esp32_s31_coreboard::{
    board::{self, RgbLed},
    ethernet, rgmii_pins,
};
use esp32_s31_coreboard_examples::{http, net};
use log::{info, warn};
use smart_leds::{RGB8, SmartLedsWriteAsync, gamma};

esp_bootloader_esp_idf::esp_app_desc!();

const PAGE: &str = include_str!("../../assets/web_led.html");
/// Server tasks per network, so a browser's second connection isn't blocked.
const SERVERS_PER_NETWORK: usize = 2;
/// A client that sends nothing for this long is dropped.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The LED color as set from the web page.
static COLOR: Mutex<CriticalSectionRawMutex, Cell<[u8; 3]>> = Mutex::new(Cell::new([0; 3]));
/// Wakes the LED task with a new color.
static LED_UPDATE: Signal<CriticalSectionRawMutex, [u8; 3]> = Signal::new();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // MPLL at 500 MHz for gigabit Ethernet; heap for the Wi-Fi driver.
    esp32_s31_coreboard_examples::start!(
        p,
        "Demo 05: web-controlled RGB LED",
        init = board::init_gigabit(),
        heap = 96 * 1024
    );

    let led = board::rgb_led(p.RMT, p.GPIO60).expect("RMT LED init");
    spawner.spawn(led_task(led).expect("spawn LED task"));

    match ethernet::new(p.ETH, p.GPIO7, rgmii_pins!(p)).await {
        Ok(eth) => serve(&spawner, net::start_ethernet(&spawner, eth), "Ethernet"),
        Err(e) => warn!("Ethernet init failed: {e:?}"),
    }
    if let Some(stack) = net::start_wifi_from_env(&spawner, p.WIFI) {
        serve(&spawner, stack, "Wi-Fi");
    }

    loop {
        core::future::pending::<()>().await;
    }
}

/// Spawns the web server tasks for one network.
fn serve(spawner: &Spawner, stack: Stack<'static>, network: &'static str) {
    for i in 0..SERVERS_PER_NETWORK {
        let announce = i == 0;
        spawner.spawn(web_task(stack, network, announce).expect("spawn web task"));
    }
}

/// Writes every new color to the LED, gamma-corrected so it looks like the
/// color picked in the browser.
#[embassy_executor::task]
async fn led_task(mut led: RgbLed<'static>) {
    loop {
        let [r, g, b] = LED_UPDATE.wait().await;
        if let Err(e) = led.write(gamma([RGB8::new(r, g, b)].into_iter())).await {
            warn!("LED write failed: {e:?}");
        }
    }
}

/// Accepts connections on port 80 of `stack` and answers them, one at a time.
/// With `announce`, logs the page address once DHCP is done.
#[allow(
    clippy::large_stack_frames,
    reason = "the socket buffers live in the task's future, which embassy keeps in a static pool"
)]
#[embassy_executor::task(pool_size = 2 * SERVERS_PER_NETWORK)]
async fn web_task(stack: Stack<'static>, network: &'static str, announce: bool) -> ! {
    if announce {
        stack.wait_config_up().await;
        if let Some(config) = stack.config_v4() {
            info!("{network}: open http://{}/", config.address.address());
        }
    }

    let mut rx = [0u8; 1024];
    let mut tx = [0u8; 2048];
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
        socket.set_timeout(Some(CLIENT_TIMEOUT));
        if let Err(e) = socket.accept(80).await {
            warn!("{network}: accept failed: {e:?}");
            continue;
        }
        if let Err(e) = handle(&mut socket, network).await {
            warn!("{network}: connection error: {e:?}");
        }
        // Send what is left, then close.
        socket.close();
        let _ = socket.flush().await;
    }
}

/// Reads one request and writes the response.
#[allow(
    clippy::large_stack_frames,
    reason = "awaited inside `web_task`, so the request buffer is part of its static future"
)]
async fn handle(socket: &mut TcpSocket<'_>, network: &str) -> Result<(), TcpError> {
    let mut head = [0u8; 1024];
    let mut len = 0;
    while len < head.len() && !http::header_complete(&head[..len]) {
        match socket.read(&mut head[len..]).await? {
            0 => return Ok(()), // closed before a full request
            n => len += n,
        }
    }

    let Some(request) = http::parse_request(&head[..len]) else {
        return respond(socket, "400 Bad Request", "text/plain", b"bad request").await;
    };
    match (request.method, request.path) {
        ("GET", "/") => {
            respond(
                socket,
                "200 OK",
                "text/html; charset=utf-8",
                PAGE.as_bytes(),
            )
            .await
        }
        ("GET", "/api/led") => {
            if let Some(value) = http::query_param(request.query, "color") {
                let Some(color) = http::parse_color(value) else {
                    return respond(socket, "400 Bad Request", "text/plain", b"bad color").await;
                };
                set_color(color, network);
            }
            let [r, g, b] = COLOR.lock(Cell::get);
            let mut json = [0u8; 32];
            let body = http::format(
                &mut json,
                format_args!("{{\"color\":\"#{r:02x}{g:02x}{b:02x}\"}}"),
            )
            .unwrap_or(b"{}");
            respond(socket, "200 OK", "application/json", body).await
        }
        _ => respond(socket, "404 Not Found", "text/plain", b"not found").await,
    }
}

/// Stores the color and hands it to the LED task (only if it changed).
fn set_color(color: [u8; 3], network: &str) {
    if COLOR.lock(|c| c.replace(color)) != color {
        let [r, g, b] = color;
        info!("{network}: LED #{r:02x}{g:02x}{b:02x}");
        LED_UPDATE.signal(color);
    }
}

async fn respond(
    socket: &mut TcpSocket<'_>,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), TcpError> {
    let mut buf = [0u8; 192];
    let header = http::response_header(&mut buf, status, content_type, body.len())
        .expect("192 bytes hold any header written here");
    socket.write_all(header).await?;
    socket.write_all(body).await
}
