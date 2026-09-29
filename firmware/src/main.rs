//! picotalk: a TashTalk-compatible LocalTalk interface on the RP2350.
//!
//! Core 1 runs the LocalTalk link (PIO line coding, framing, MAC) in a busy
//! loop. Core 0 connects it to one of (pick with a cargo feature):
//!
//! * `host-uart` (default): the TashTalk protocol on UART0, as a drop-in
//!   replacement for a TashTalk chip (e.g. in an AirTalk or on a Pi);
//! * `host-usb`: the TashTalk protocol over USB CDC-ACM;
//! * `ltoudp`: a standalone LocalTalk <-> LToUDP bridge over Wi-Fi (Pico 2 W).
//!
//! Pins (Pico 2 / Pico 2 W):
//!
//! | GPIO | Function                                               |
//! |------|--------------------------------------------------------|
//! | GP0  | UART0 TX  -> host RX                                   |
//! | GP1  | UART0 RX  <- host TX                                   |
//! | GP2  | UART0 CTS <- host RTS (optional, pulled low)           |
//! | GP3  | UART0 RTS -> host CTS (low = host may send)            |
//! | GP6  | LocalTalk RX  <- transceiver RO                        |
//! | GP7  | LocalTalk TX  -> transceiver DI                        |
//! | GP8  | Driver enable -> transceiver DE and /RE                |

#![no_std]
#![no_main]

#[cfg(not(feature = "ltoudp"))]
mod host;
mod line;
mod link;
#[cfg(feature = "ltoudp")]
mod ltoudp;
mod shared;

use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_rp::multicore::{Stack, spawn_core1};
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::{self, Pio};
use embassy_rp::bind_interrupts;
use panic_probe as _;
use static_cell::StaticCell;

#[unsafe(link_section = ".bi_entries")]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 4] = [
    embassy_rp::binary_info::rp_program_name!(c"picotalk"),
    embassy_rp::binary_info::rp_program_description!(c"TashTalk-compatible LocalTalk interface"),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

#[cfg(feature = "host-uart")]
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    UART0_IRQ => embassy_rp::uart::BufferedInterruptHandler<embassy_rp::peripherals::UART0>;
});

#[cfg(feature = "host-usb")]
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<embassy_rp::peripherals::USB>;
});

#[cfg(feature = "ltoudp")]
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    PIO1_IRQ_0 => pio::InterruptHandler<embassy_rp::peripherals::PIO1>;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<embassy_rp::peripherals::DMA_CH0>;
});

#[cfg(not(any(
    all(feature = "host-uart", not(feature = "host-usb"), not(feature = "ltoudp")),
    all(feature = "host-usb", not(feature = "host-uart"), not(feature = "ltoudp")),
    all(feature = "ltoudp", not(feature = "host-uart"), not(feature = "host-usb")),
)))]
compile_error!("enable exactly one of the host-uart, host-usb and ltoudp features");

static mut CORE1_STACK: Stack<8192> = Stack::new();
static LINK_BUFFERS: StaticCell<link::Buffers> = StaticCell::new();

#[embassy_executor::main(executor = "embassy_rp::executor::Executor", entry = "cortex_m_rt::entry")]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("picotalk starting");

    let Pio { mut common, mut sm0, mut sm1, .. } = Pio::new(p.PIO0, Irqs);
    line::setup_rx(&mut common, &mut sm0, p.PIN_6);
    line::setup_tx(&mut common, &mut sm1, p.PIN_7, p.PIN_8);
    // `common` owns the loaded programs; it must outlive the state machines.
    core::mem::forget(common);

    let bufs = LINK_BUFFERS.init(link::Buffers::new());
    let seed = random_seed();
    spawn_core1(p.CORE1, unsafe { &mut *core::ptr::addr_of_mut!(CORE1_STACK) }, move || {
        link::Link::new(sm0, sm1, bufs, seed).run()
    });

    #[cfg(feature = "host-uart")]
    {
        use embassy_rp::uart::{BufferedUart, Config};

        static TX_BUF: StaticCell<[u8; 4096]> = StaticCell::new();
        static RX_BUF: StaticCell<[u8; 2048]> = StaticCell::new();
        let mut config = Config::default();
        config.baudrate = 1_000_000;
        let uart = BufferedUart::new_with_rtscts(
            p.UART0,
            p.PIN_0,
            p.PIN_1,
            p.PIN_3,
            p.PIN_2,
            Irqs,
            TX_BUF.init([0; 4096]),
            RX_BUF.init([0; 2048]),
            config,
        );
        // Hosts without an RTS output leave CTS unconnected: pull it low
        // (asserted) so we may always send, as TashTalk does.
        embassy_rp::pac::PADS_BANK0.gpio(2).modify(|w| w.set_pde(true));

        let (tx, rx) = uart.split();
        spawner.spawn(defmt::unwrap!(uart_rx_task(rx)));
        spawner.spawn(defmt::unwrap!(uart_tx_task(tx)));
    }

    #[cfg(feature = "host-usb")]
    spawner.spawn(defmt::unwrap!(usb::run(p.USB)));

    #[cfg(feature = "ltoudp")]
    {
        let pins = ltoudp::WifiPins {
            pio: p.PIO1,
            dma: p.DMA_CH0,
            pwr: p.PIN_23,
            dio: p.PIN_24,
            cs: p.PIN_25,
            clk: p.PIN_29,
        };
        let net_seed = (random_seed() as u64) << 32 | random_seed() as u64;
        ltoudp::run(spawner, pins, net_seed).await;
    }
}

/// Seeds the backoff generator from the ring oscillator's jitter.
fn random_seed() -> u32 {
    let mut seed = 0u32;
    for _ in 0..32 {
        seed = (seed << 1) | embassy_rp::pac::ROSC.randombit().read().randombit() as u32;
    }
    seed ^ embassy_time::Instant::now().as_ticks() as u32
}

#[cfg(feature = "host-uart")]
#[embassy_executor::task]
async fn uart_rx_task(mut rx: embassy_rp::uart::BufferedUartRx) {
    loop {
        let _ = host::host_to_link(&mut rx).await;
        defmt::warn!("UART receive error");
    }
}

#[cfg(feature = "host-uart")]
#[embassy_executor::task]
async fn uart_tx_task(mut tx: embassy_rp::uart::BufferedUartTx) {
    loop {
        let _ = host::link_to_host(&mut tx).await;
    }
}

#[cfg(feature = "host-usb")]
mod usb {
    use defmt::info;
    use embassy_futures::join::join3;
    use embassy_rp::Peri;
    use embassy_rp::peripherals::USB;
    use embassy_rp::usb::Driver;
    use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
    use embassy_usb::{Builder, Config};
    use static_cell::StaticCell;

    use crate::{Irqs, host};

    #[embassy_executor::task]
    pub async fn run(usb: Peri<'static, USB>) {
        let driver = Driver::new(usb, Irqs);
        let mut config = Config::new(0x1209, 0x0001); // pid.codes test VID/PID
        config.manufacturer = Some("picotalk");
        config.product = Some("LocalTalk interface");
        config.max_power = 100;
        config.max_packet_size_0 = 64;

        static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        static STATE: StaticCell<State> = StaticCell::new();
        static RX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        let mut builder = Builder::new(
            driver,
            config,
            CONFIG_DESC.init([0; 256]),
            BOS_DESC.init([0; 256]),
            &mut [],
            CONTROL_BUF.init([0; 64]),
        );
        let class = CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64);
        let mut device = builder.build();

        let (mut tx, rx) = class.split();
        let mut rx = rx.into_buffered(RX_BUF.init([0; 64]));

        let host_rx = async {
            loop {
                rx.wait_connection().await;
                info!("USB host connected");
                let _ = host::host_to_link(&mut rx).await;
            }
        };
        let host_tx = async {
            loop {
                tx.wait_connection().await;
                let _ = host::link_to_host(&mut tx).await;
            }
        };
        join3(device.run(), host_rx, host_tx).await;
    }
}
