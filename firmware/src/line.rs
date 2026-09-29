//! PIO programs for the LocalTalk line.
//!
//! * RX: sample the receive pin at 8x the bit rate, 32 samples per word,
//!   earliest sample in the MSB. All decoding happens in software.
//! * TX: shift out two pins (line level, driver enable) per half-bit symbol,
//!   16 symbols per word. When the FIFO runs dry the state machine stalls on
//!   `out` and the pins keep the last symbol, which is always "released".

use embassy_rp::Peri;
use embassy_rp::gpio::{Level, Pull};
use embassy_rp::pio::{Common, Config, Direction, FifoJoin, Instance, PioPin, ShiftConfig, ShiftDirection, StateMachine};
use embassy_rp::pio_programs::clock_divider::calculate_pio_clock_divider;
use llap::fm0::{BIT_RATE, SAMPLES_PER_BIT};

pub fn setup_rx<'d, P: Instance, const SM: usize>(
    common: &mut Common<'d, P>,
    sm: &mut StateMachine<'d, P, SM>,
    rx_pin: Peri<'d, impl PioPin>,
) {
    let prg = embassy_rp::pio::program::pio_asm!(
        ".wrap_target",
        "    in pins, 1",
        ".wrap",
    );
    let loaded = common.load_program(&prg.program);

    let mut rx = common.make_pio_pin(rx_pin);
    // The transceiver's receiver output may float while we transmit.
    rx.set_pull(Pull::Up);
    sm.set_pin_dirs(Direction::In, &[&rx]);

    let mut cfg = Config::default();
    cfg.use_program(&loaded, &[]);
    cfg.set_in_pins(&[&rx]);
    cfg.shift_in = ShiftConfig { threshold: 32, direction: ShiftDirection::Left, auto_fill: true };
    cfg.fifo_join = FifoJoin::RxOnly;
    cfg.clock_divider = calculate_pio_clock_divider(BIT_RATE * SAMPLES_PER_BIT);
    sm.set_config(&cfg);
    sm.set_enable(true);
}

/// `txd_pin` and `de_pin` must be consecutive GPIOs (DE = TXD + 1).
pub fn setup_tx<'d, P: Instance, const SM: usize>(
    common: &mut Common<'d, P>,
    sm: &mut StateMachine<'d, P, SM>,
    txd_pin: Peri<'d, impl PioPin>,
    de_pin: Peri<'d, impl PioPin>,
) {
    let prg = embassy_rp::pio::program::pio_asm!(
        ".wrap_target",
        "    out pins, 2",
        ".wrap",
    );
    let loaded = common.load_program(&prg.program);

    let txd = common.make_pio_pin(txd_pin);
    let de = common.make_pio_pin(de_pin);
    assert_eq!(de.pin(), txd.pin() + 1, "DE must be the GPIO after TXD");
    sm.set_pins(Level::Low, &[&txd, &de]);
    sm.set_pin_dirs(Direction::Out, &[&txd, &de]);

    let mut cfg = Config::default();
    cfg.use_program(&loaded, &[]);
    cfg.set_out_pins(&[&txd, &de]);
    cfg.shift_out = ShiftConfig { threshold: 32, direction: ShiftDirection::Right, auto_fill: true };
    cfg.fifo_join = FifoJoin::TxOnly;
    cfg.clock_divider = calculate_pio_clock_divider(BIT_RATE * 2);
    sm.set_config(&cfg);
    sm.set_enable(true);
}
