//! Network setup shared by the demos: a Wi-Fi station or the Ethernet port,
//! each with an embassy-net stack running DHCP, plus DNS/NTP helpers.
//!
//! Wi-Fi credentials are read at build time from the `WIFI_SSID` and
//! `WIFI_PASSWORD` environment variables. An empty or missing password
//! selects an open network.

use core::net::{IpAddr, SocketAddr};
use embassy_executor::Spawner;

use embassy_net::{
    Config as NetConfig, IpAddress, Runner, Stack, StackResources,
    dns::DnsQueryType,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::{peripherals::WIFI, rng::Rng};
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config as WifiConfig, ControllerConfig, Interface, WifiController,
    WifiError,
    sta::{ScanMethod, StationConfig},
};
use esp32_s31_coreboard::ethernet::Eth;
use log::{info, warn};
use sntpc::NtpContext;
use sntpc_net_embassy::UdpSocketWrapper;
use sntpc_time_embassy::EmbassyTimestampGenerator;
use static_cell::StaticCell;

/// Socket slots: DHCP, DNS, plus a few for the application.
const SOCKETS: usize = 6;
const RECONNECT_DELAY: Duration = Duration::from_secs(5);
/// A connect attempt normally finishes within a few seconds; give up after this.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Build-time Wi-Fi credentials, if `WIFI_SSID` was set.
pub fn credentials() -> Option<(&'static str, &'static str)> {
    let ssid = option_env!("WIFI_SSID").filter(|s| !s.is_empty())?;
    Some((ssid, option_env!("WIFI_PASSWORD").unwrap_or("")))
}

/// Station configuration for `ssid`; an empty `password` selects an open network.
pub fn station_config(ssid: &str, password: &str) -> Result<WifiConfig, WifiError> {
    let auth = if password.is_empty() {
        AuthenticationMethodConfig::Open
    } else {
        AuthenticationMethodConfig::Wpa2Personal(
            password
                .try_into()
                .map_err(|_| WifiError::InvalidArguments)?,
        )
    };
    // Scan all channels before joining, so the strongest access point of
    // the network is chosen (the default fast scan takes the first one found).
    let station = StationConfig::default()
        .with_ssid(ssid.try_into().map_err(|_| WifiError::InvalidArguments)?)
        .with_authentication(auth)
        .with_scan_method(ScanMethod::AllChannels);
    Ok(WifiConfig::Station(station))
}

/// Creates the station interface and starts the Wi-Fi controller with `config`
/// (or an unconfigured station when `None`), following the esp-radio examples.
pub fn init_wifi(
    wifi: WIFI<'static>,
    config: Option<WifiConfig>,
) -> Result<(WifiController<'static>, Interface), WifiError> {
    let interface = Interface::station();
    let controller_config = match config {
        Some(config) => ControllerConfig::default().with_initial_config(config),
        None => ControllerConfig::default(),
    };
    let controller = WifiController::new(wifi, controller_config)?;
    Ok((controller, interface))
}

/// A random seed for the network stack.
fn seed() -> u64 {
    let rng = Rng::new();
    (u64::from(rng.random()) << 32) | u64::from(rng.random())
}

/// Starts a network stack with DHCP on the Wi-Fi station and spawns the
/// runner and the connect/reconnect tasks.
pub fn start_wifi(
    spawner: &Spawner,
    controller: WifiController<'static>,
    interface: Interface,
) -> Stack<'static> {
    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

    let (stack, runner) = embassy_net::new(
        interface,
        NetConfig::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed(),
    );

    spawner.spawn(wifi_net_task(runner).expect("spawn net task"));
    spawner.spawn(connection_task(controller).expect("spawn wifi task"));
    stack
}

/// Connects to the network from `WIFI_SSID` / `WIFI_PASSWORD` and starts
/// its stack (see [`start_wifi`]). `None`, with a log line, if no SSID was
/// set at build time or the radio fails to start.
pub fn start_wifi_from_env(spawner: &Spawner, wifi: WIFI<'static>) -> Option<Stack<'static>> {
    let Some((ssid, password)) = credentials() else {
        info!("WIFI_SSID not set at build time, Wi-Fi stays off");
        return None;
    };
    let started = station_config(ssid, password).and_then(|config| init_wifi(wifi, Some(config)));
    match started {
        Ok((controller, interface)) => Some(start_wifi(spawner, controller, interface)),
        Err(e) => {
            warn!("Wi-Fi start failed: {e:?}");
            None
        }
    }
}

/// Starts a network stack with DHCP on the Ethernet port and spawns its
/// runner. The link comes up by itself when a cable is plugged in.
pub fn start_ethernet(spawner: &Spawner, eth: Eth<'static>) -> Stack<'static> {
    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

    let (stack, runner) = embassy_net::new(
        eth,
        NetConfig::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed(),
    );

    spawner.spawn(eth_net_task(runner).expect("spawn net task"));
    stack
}

#[embassy_executor::task]
async fn wifi_net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn eth_net_task(mut runner: Runner<'static, Eth<'static>>) -> ! {
    runner.run().await
}

/// Logs the address, gateway and DNS servers DHCP handed out.
pub fn print_config(stack: Stack<'_>) {
    if let Some(config) = stack.config_v4() {
        info!(
            "DHCP: address {}, gateway {:?}, DNS {:?}",
            config.address, config.gateway, config.dns_servers
        );
    }
}

/// Wall-clock time from one NTP query plus the uptime since.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    unix: u64,
    synced_at: Instant,
}

impl Clock {
    /// Queries `server` and starts the clock from its answer.
    ///
    /// # Errors
    ///
    /// See [`ntp_time`].
    pub async fn sync(stack: Stack<'_>, server: &str) -> Result<Self, NtpError> {
        let unix = ntp_time(stack, server).await?;
        Ok(Self {
            unix,
            synced_at: Instant::now(),
        })
    }

    /// Current Unix time in seconds.
    #[must_use]
    pub fn unix(&self) -> u64 {
        self.unix + self.synced_at.elapsed().as_secs()
    }

    /// Current UTC date and time.
    #[must_use]
    pub fn now(&self) -> DateTime {
        DateTime::from_unix(self.unix())
    }
}

/// NTP server the demos use.
pub const NTP_SERVER: &str = "pool.ntp.org";

/// Every 10 s logs the UTC time and the IP address, forever. Syncs the clock
/// over NTP first (retried every round until it works) and waits for DHCP
/// again whenever the network goes down.
pub async fn log_time_forever(stack: Stack<'_>) -> ! {
    let mut clock = None;
    loop {
        if clock.is_none() {
            match Clock::sync(stack, NTP_SERVER).await {
                Ok(c) => {
                    info!("NTP: {} ({})", c.now(), c.unix());
                    clock = Some(c);
                }
                Err(e) => warn!("NTP query to {NTP_SERVER} failed: {e}"),
            }
        }

        Timer::after(Duration::from_secs(10)).await;

        if !stack.is_config_up() {
            warn!("network down, waiting for DHCP...");
            stack.wait_config_up().await;
            print_config(stack);
        }
        if let Some(clock) = clock {
            match stack.config_v4() {
                Some(config) => info!("{} | IP {}", clock.now(), config.address),
                None => info!("{} | no IP", clock.now()),
            }
        }
    }
}

/// Resolves `host` and logs the result; for checking DNS works.
pub async fn check_dns(stack: Stack<'_>, host: &str) {
    match stack.dns_query(host, DnsQueryType::A).await {
        Ok(addrs) => info!("DNS: {host} -> {addrs:?}"),
        Err(e) => warn!("DNS lookup of {host} failed: {e:?}"),
    }
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) -> ! {
    loop {
        if controller.is_connected() {
            match controller.wait_for_disconnect_async().await {
                Ok(info) => warn!("Wi-Fi disconnected: {:?}", info.reason),
                Err(e) => warn!("Wi-Fi disconnect wait failed: {e:?}"),
            }
            Timer::after(RECONNECT_DELAY).await;
        }

        info!("Wi-Fi connecting...");
        match with_timeout(CONNECT_TIMEOUT, controller.connect_async()).await {
            Ok(Ok(ap)) => info!(
                "Wi-Fi connected to '{}' (channel {}, RSSI {} dBm)",
                ap.ssid.as_str(),
                ap.channel,
                controller.rssi().unwrap_or(0)
            ),
            Ok(Err(e)) => {
                warn!("Wi-Fi connect failed: {e:?}, retrying in 5 s");
                Timer::after(RECONNECT_DELAY).await;
            }
            Err(_) => {
                warn!(
                    "Wi-Fi connect got no answer within {} s, aborting and retrying",
                    CONNECT_TIMEOUT.as_secs()
                );
                // Abort the pending attempt so the next one starts cleanly.
                if let Ok(Err(e)) =
                    with_timeout(RECONNECT_DELAY, controller.disconnect_async()).await
                {
                    warn!("Wi-Fi abort failed: {e:?}");
                }
                Timer::after(RECONNECT_DELAY).await;
            }
        }
    }
}

/// Why [`ntp_time`] failed.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum NtpError {
    /// The server name didn't resolve to an IPv4 address.
    Dns,
    /// The UDP socket couldn't be bound.
    Socket,
    /// No reply within 5 s.
    Timeout,
    /// The request failed or the reply was rejected.
    Ntp(sntpc::Error),
}

impl core::fmt::Display for NtpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NtpError::Dns => f.write_str("DNS lookup failed"),
            NtpError::Socket => f.write_str("UDP socket error"),
            NtpError::Timeout => f.write_str("no reply"),
            NtpError::Ntp(e) => write!(f, "{e:?}"),
        }
    }
}

impl core::error::Error for NtpError {}

/// Asks `server` for the time (SNTP, with the `sntpc` crate) and returns
/// Unix seconds.
pub async fn ntp_time(stack: Stack<'_>, server: &str) -> Result<u64, NtpError> {
    let addrs = stack
        .dns_query(server, DnsQueryType::A)
        .await
        .map_err(|_| NtpError::Dns)?;
    let Some(IpAddress::Ipv4(addr)) = addrs.first().copied() else {
        return Err(NtpError::Dns);
    };

    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx_buf = [0u8; 128];
    let mut tx_meta = [PacketMetadata::EMPTY; 2];
    let mut tx_buf = [0u8; 128];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket.bind(0).map_err(|_| NtpError::Socket)?;
    let socket = UdpSocketWrapper::from(socket);

    let context = NtpContext::new(EmbassyTimestampGenerator::default());
    let request = sntpc::get_time(SocketAddr::new(IpAddr::V4(addr), 123), &socket, context);
    match with_timeout(Duration::from_secs(5), request).await {
        Ok(Ok(time)) => Ok(time.sec()),
        Ok(Err(e)) => Err(NtpError::Ntp(e)),
        Err(_) => Err(NtpError::Timeout),
    }
}

pub use crate::time::DateTime;
