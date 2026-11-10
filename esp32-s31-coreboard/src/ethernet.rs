//! Gigabit Ethernet: the ESP32-S31 EMAC wired over RGMII to a Motorcomm
//! YT8531 PHY (address found on the MDIO bus), RJ45 with magnetics.
//!
//! esp-hal's `GenericPhy` already applies the YT8531's 2 ns RGMII clock
//! delays and retunes TX_CLK when the negotiated speed changes. [`BoardPhy`]
//! adds what esp-hal leaves to the application: polling the link every
//! 500 ms from embassy-time (instead of busy-polling) and logging changes.
//!
//! ```ignore
//! let eth = ethernet::new(p.ETH, p.GPIO7, esp32_s31_coreboard::rgmii_pins!(p)).await?;
//! let (stack, runner) = embassy_net::new(eth, config, resources, seed);
//! ```

use core::{future::Future, pin::Pin, task::Context};

use embassy_time::{Duration, Timer};
use esp_hal::{
    Async,
    efuse::{self, InterfaceMacAddress},
    ethernet::{
        Error, Ethernet, EthernetDmaStorage, RgmiiPinBundle,
        mac::{Duplex, LinkState, Speed},
        phy::{MdioBus, Phy, PhyError, generic::GenericPhy},
    },
    gpio::{Level, Output, OutputConfig},
    peripherals::{
        ETH, GPIO5, GPIO6, GPIO7, GPIO8, GPIO9, GPIO10, GPIO11, GPIO12, GPIO13, GPIO14, GPIO15,
        GPIO16, GPIO17, GPIO18, GPIO19,
    },
};
use log::info;
use static_cell::ConstStaticCell;

/// How often the PHY link state is read over MDIO.
const LINK_POLL: Duration = Duration::from_millis(500);

/// DMA descriptor rings: 10 receive and 10 transmit buffers of 1600 bytes.
type Storage = EthernetDmaStorage<10, 10>;

/// The async Ethernet driver; implements `embassy_net_driver::Driver`.
pub type Eth<'d> = Ethernet<'d, Async, BoardPhy>;

/// The RGMII and MDIO pins of the board; [`rgmii_pins!`](crate::rgmii_pins)
/// fills it in from the peripherals.
pub type RgmiiPins<'d> = RgmiiPinBundle<
    GPIO14<'d>, // RX_CLK
    GPIO13<'d>, // TX_CLK
    GPIO15<'d>, // RX_CTL
    GPIO12<'d>, // TX_CTL
    GPIO19<'d>, // RXD0
    GPIO18<'d>, // RXD1
    GPIO17<'d>, // RXD2
    GPIO16<'d>, // RXD3
    GPIO8<'d>,  // TXD0
    GPIO9<'d>,  // TXD1
    GPIO10<'d>, // TXD2
    GPIO11<'d>, // TXD3
    GPIO5<'d>,  // MDC
    GPIO6<'d>,  // MDIO
>;

/// [`RgmiiPins`] taken out of a `Peripherals` variable:
/// `ethernet::new(p.ETH, p.GPIO7, rgmii_pins!(p))`.
#[macro_export]
macro_rules! rgmii_pins {
    ($p:ident) => {
        esp_hal::ethernet::RgmiiPinBundle {
            rx_clk: $p.GPIO14,
            tx_clk: $p.GPIO13,
            rx_ctl: $p.GPIO15,
            tx_ctl: $p.GPIO12,
            rxd0: $p.GPIO19,
            rxd1: $p.GPIO18,
            rxd2: $p.GPIO17,
            rxd3: $p.GPIO16,
            txd0: $p.GPIO8,
            txd1: $p.GPIO9,
            txd2: $p.GPIO10,
            txd3: $p.GPIO11,
            mdc: $p.GPIO5,
            mdio: $p.GPIO6,
        }
    };
}

/// The Ethernet MAC address: the chip's base MAC + 3, as ESP-IDF derives it.
#[must_use]
pub fn mac_address() -> [u8; 6] {
    let base = efuse::interface_mac_address(InterfaceMacAddress::Station);
    let mut bytes = [0u8; 8];
    bytes[2..].copy_from_slice(base.as_bytes());
    let mac = u64::from_be_bytes(bytes).wrapping_add(3).to_be_bytes();
    [mac[2], mac[3], mac[4], mac[5], mac[6], mac[7]]
}

/// Resets the PHY (GPIO7) and starts the EMAC with [`mac_address`].
///
/// Call at most once; taking `ETH` by value enforces that.
///
/// # Errors
///
/// If no PHY answers on the MDIO bus or its reset times out.
pub async fn new(
    eth: ETH<'static>,
    reset: GPIO7<'static>,
    pins: RgmiiPins<'static>,
) -> Result<Eth<'static>, Error> {
    static STORAGE: ConstStaticCell<Storage> = ConstStaticCell::new(Storage::new());

    // Hold reset for 100 ms, then give the PHY 300 ms to come up. `BoardPhy`
    // keeps the pin driven high from then on.
    let mut reset = Output::new(reset, Level::Low, OutputConfig::default());
    Timer::after(Duration::from_millis(100)).await;
    reset.set_high();
    Timer::after(Duration::from_millis(300)).await;

    let phy = BoardPhy::new(reset);
    let eth = Ethernet::new(eth, STORAGE.take(), mac_address(), phy, pins)?;
    Ok(eth.into_async())
}

/// [`GenericPhy`] polled every 500 ms from embassy-time, logging link changes.
pub struct BoardPhy {
    phy: GenericPhy,
    next_poll: Timer,
    link: LinkState,
    /// PHY reset (active low), held high while the driver exists.
    _reset: Output<'static>,
}

impl BoardPhy {
    fn new(reset: Output<'static>) -> Self {
        Self {
            phy: GenericPhy::new_auto(),
            _reset: reset,
            next_poll: Timer::after_ticks(0),
            link: LinkState {
                up: false,
                speed: Speed::_100M,
                duplex: Duplex::Full,
            },
        }
    }

    /// The link state from the last poll.
    pub fn link(&self) -> LinkState {
        self.link
    }
}

impl Phy for BoardPhy {
    fn address(&self) -> u8 {
        self.phy.address()
    }

    fn init<M: MdioBus>(&mut self, mdio: &mut M, max_speed: Speed) -> Result<(), PhyError> {
        self.phy.init(mdio, max_speed)?;
        info!("Ethernet PHY at MDIO address {}", self.phy.address());
        Ok(())
    }

    fn poll_link<M: MdioBus>(&mut self, mdio: &mut M, cx: Option<&mut Context<'_>>) -> LinkState {
        if let Some(cx) = cx {
            if Pin::new(&mut self.next_poll).poll(cx).is_pending() {
                return self.link;
            }
            // Re-arm and register the waker for the next poll.
            self.next_poll = Timer::after(LINK_POLL);
            let _ = Pin::new(&mut self.next_poll).poll(cx);
        }

        let link = self.phy.poll_link(mdio, None);
        if link.up != self.link.up || link.speed != self.link.speed {
            if link.up {
                info!(
                    "Ethernet link up: {:?}, {:?} duplex",
                    link.speed, link.duplex
                );
            } else {
                info!("Ethernet link down");
            }
        }
        self.link = link;
        link
    }
}
