//! Pico 2 W mode: bridge LocalTalk straight to LToUDP over Wi-Fi.
//!
//! Wi-Fi credentials come from the `WIFI_SSID` and `WIFI_PASSWORD`
//! environment variables at build time.

use core::cell::RefCell;

use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, StackResources};
use embassy_rp::Peri;
use embassy_rp::dma;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO1};
use embassy_rp::pio::Pio;
use embassy_time::{Instant, Timer};
use llap::ltoudp::{self, Bridge};
use llap::tashtalk::{FEATURE_CHECK_CRC, RxStatus};
use static_cell::StaticCell;

use crate::Irqs;
use crate::shared::{FROM_LINK, TO_LINK, ToLink};

const WIFI_SSID: &str = env!("WIFI_SSID", "set WIFI_SSID to build the ltoudp mode");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD", "set WIFI_PASSWORD to build the ltoudp mode");

pub struct WifiPins {
    pub pio: Peri<'static, PIO1>,
    pub dma: Peri<'static, DMA_CH0>,
    pub pwr: Peri<'static, PIN_23>,
    pub dio: Peri<'static, PIN_24>,
    pub cs: Peri<'static, PIN_25>,
    pub clk: Peri<'static, PIN_29>,
}

type Spi = PioSpi<'static, PIO1, 0>;

#[embassy_executor::task]
async fn cyw43_task(runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, Spi>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

fn now_s() -> u32 {
    Instant::now().as_secs() as u32
}

pub async fn run(spawner: Spawner, pins: WifiPins, seed: u64) {
    let fw = cyw43::aligned_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = cyw43::aligned_bytes!("../cyw43-firmware/43439A0_clm.bin");
    let nvram = cyw43::aligned_bytes!("../cyw43-firmware/nvram_rp2040.bin");

    let pwr = Output::new(pins.pwr, Level::Low);
    let cs = Output::new(pins.cs, Level::High);
    let mut pio = Pio::new(pins.pio, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        pins.dio,
        pins.clk,
        dma::Channel::new(pins.dma, Irqs),
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let (net_device, mut control, runner) = cyw43::new(STATE.init(cyw43::State::new()), pwr, spi, fw, nvram).await;
    spawner.spawn(unwrap!(cyw43_task(runner)));

    control.init(clm).await;
    control.set_power_management(cyw43::PowerManagementMode::None).await;
    // The radio drops multicast frames unless their MAC address is listed.
    if control.add_multicast_address(ltoudp::GROUP_MAC).await.is_err() {
        warn!("could not add LToUDP multicast address to the radio filter");
    }

    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let config = embassy_net::Config::dhcpv4(Default::default());
    let (stack, net_runner) = embassy_net::new(net_device, config, RESOURCES.init(StackResources::new()), seed);
    spawner.spawn(unwrap!(net_task(net_runner)));

    loop {
        match control.join(WIFI_SSID, JoinOptions::new(WIFI_PASSWORD.as_bytes())).await {
            Ok(()) => break,
            Err(_) => {
                warn!("joining {} failed, retrying", WIFI_SSID);
                Timer::after_secs(2).await;
            }
        }
    }
    stack.wait_config_up().await;
    if let Some(cfg) = stack.config_v4() {
        info!("Wi-Fi up, address {}", cfg.address);
    }

    let [a, b, c, d] = ltoudp::GROUP;
    let group = Ipv4Address::new(a, b, c, d);
    unwrap!(stack.join_multicast_group(group));

    static RX_META: StaticCell<[PacketMetadata; 16]> = StaticCell::new();
    static TX_META: StaticCell<[PacketMetadata; 16]> = StaticCell::new();
    static RX_BUF: StaticCell<[u8; 8192]> = StaticCell::new();
    static TX_BUF: StaticCell<[u8; 8192]> = StaticCell::new();
    let mut socket = UdpSocket::new(
        stack,
        RX_META.init([PacketMetadata::EMPTY; 16]),
        RX_BUF.init([0; 8192]),
        TX_META.init([PacketMetadata::EMPTY; 16]),
        TX_BUF.init([0; 8192]),
    );
    unwrap!(socket.bind(ltoudp::PORT));
    let socket = &socket;
    let dest = IpEndpoint::new(IpAddress::Ipv4(group), ltoudp::PORT);

    // Only well-formed frames should cross the bridge.
    TO_LINK.send(ToLink::SetFeatures(FEATURE_CHECK_CRC)).await;

    let bridge = RefCell::new(Bridge::new(seed as u32));
    info!("bridging LocalTalk <-> LToUDP");

    let to_udp = async {
        let mut out = [0u8; ltoudp::MAX_DATAGRAM];
        loop {
            let msg = FROM_LINK.receive().await;
            if msg.status != RxStatus::FrameDone {
                continue;
            }
            let len = bridge.borrow_mut().from_localtalk(msg.frame.as_slice(), now_s(), &mut out);
            if let Some(len) = len
                && socket.send_to(&out[..len], dest).await.is_err() {
                    warn!("LToUDP send failed");
                }
        }
    };

    let from_udp = async {
        let mut buf = [0u8; ltoudp::MAX_DATAGRAM];
        loop {
            let Ok((n, _)) = socket.recv_from(&mut buf).await else { continue };
            let now = now_s();
            let (frame, proxy) = {
                let mut b = bridge.borrow_mut();
                (b.from_udp(&buf[..n], now), b.proxy_update(now))
            };
            if let Some(ids) = proxy {
                TO_LINK.send(ToLink::SetNodeIds(ids)).await;
            }
            if let Some(frame) = frame {
                TO_LINK.send(ToLink::Transmit(frame)).await;
            }
        }
    };

    // Let remote nodes age out of the proxy set even when nothing arrives.
    let refresh = async {
        loop {
            Timer::after_secs(60).await;
            let proxy = bridge.borrow_mut().proxy_update(now_s());
            if let Some(ids) = proxy {
                TO_LINK.send(ToLink::SetNodeIds(ids)).await;
            }
        }
    };

    join3(to_udp, from_udp, refresh).await;
}
