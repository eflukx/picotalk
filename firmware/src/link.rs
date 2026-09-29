//! The LocalTalk link, running alone on core 1 in a busy loop.
//!
//! Timing budget: the RX FIFO holds 8 words = 32 bit times (~139 µs) and
//! the TX FIFO 8 words = 64 bit times (~278 µs), so each loop iteration
//! must finish well within ~100 µs. Replies to RTS must start within the
//! 200 µs inter-frame gap.

use defmt::{debug, info, warn};
use embassy_rp::pio::{Instance, StateMachine};
use embassy_time::Instant;
use llap::fm0::{Decoder, LineEvent};
use llap::frame::{self, MAX_FRAME};
use llap::hdlc::{self, Deframer, RxEvent};
use llap::mac::{Action, Line, Mac};
use llap::tashtalk::{FEATURE_CALC_CRC, FEATURE_CHECK_CRC, RxStatus};
use llap::Frame;

use crate::shared::{FROM_LINK, FromLink, TO_LINK, ToLink};

const DATA_WORDS: usize = hdlc::max_words(MAX_FRAME);
const CTRL_WORDS: usize = hdlc::max_words(frame::CONTROL_FRAME_LEN);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Buf {
    Control,
    Data,
    Response,
}

struct TxJob {
    buf: Buf,
    pos: usize,
    /// All words are in the state machine; waiting for it to stall.
    draining: bool,
    /// Part of the MAC's pending transmission (as opposed to a reply).
    mac: bool,
}

/// Encoded transmit symbols and their length in words.
struct Symbols<const N: usize> {
    words: [u32; N],
    len: usize,
}

impl<const N: usize> Symbols<N> {
    const fn new() -> Self {
        Self { words: [0; N], len: 0 }
    }

    fn encode(&mut self, frame: &[u8], sync: bool) -> bool {
        match hdlc::encode_frame(&mut self.words, frame, sync) {
            Some(len) => {
                self.len = len;
                true
            }
            None => {
                self.len = 0;
                false
            }
        }
    }

    fn as_slice(&self) -> &[u32] {
        &self.words[..self.len]
    }
}

/// Large buffers, kept out of core 1's stack.
pub struct Buffers {
    data: Symbols<DATA_WORDS>,
    control: Symbols<CTRL_WORDS>,
    response: Symbols<CTRL_WORDS>,
}

impl Buffers {
    pub const fn new() -> Self {
        Self { data: Symbols::new(), control: Symbols::new(), response: Symbols::new() }
    }

    fn get(&self, buf: Buf) -> &[u32] {
        match buf {
            Buf::Control => self.control.as_slice(),
            Buf::Data => self.data.as_slice(),
            Buf::Response => self.response.as_slice(),
        }
    }
}

pub struct Link<'d, P: Instance, const RX: usize, const TX: usize> {
    rx_sm: StateMachine<'d, P, RX>,
    tx_sm: StateMachine<'d, P, TX>,
    bufs: &'d mut Buffers,
    decoder: Decoder,
    deframer: Deframer,
    mac: Mac,
    line: Line,
    features: u8,
    job: Option<TxJob>,
    dropped_to_host: u32,
}

fn now_us() -> u64 {
    Instant::now().as_micros()
}

impl<'d, P: Instance, const RX: usize, const TX: usize> Link<'d, P, RX, TX> {
    pub fn new(rx_sm: StateMachine<'d, P, RX>, tx_sm: StateMachine<'d, P, TX>, bufs: &'d mut Buffers, seed: u32) -> Self {
        Self {
            rx_sm,
            tx_sm,
            bufs,
            decoder: Decoder::new(),
            deframer: Deframer::new(),
            mac: Mac::new(seed),
            line: Line::default(),
            features: 0,
            job: None,
            dropped_to_host: 0,
        }
    }

    pub fn run(mut self) -> ! {
        info!("link running on core 1");
        loop {
            let now = now_us();
            if let Some(action) = self.service_rx(now) {
                self.perform(action);
            }
            if self.job.is_some() {
                self.service_tx();
                continue;
            }
            if self.mac.is_idle()
                && let Ok(cmd) = TO_LINK.try_receive() {
                    self.command(cmd);
                }
            if let Some(action) = self.mac.poll(now, self.line) {
                self.perform(action);
            }
        }
    }

    /// Drains the RX FIFO through the decoder. Returns a reply to send, if
    /// a received frame calls for one.
    fn service_rx(&mut self, now: u64) -> Option<Action> {
        let mut action = None;
        while let Some(word) = self.rx_sm.rx().try_pull() {
            if self.job.is_some() {
                // Our own transmission (or the floating receiver output).
                continue;
            }
            let Self { decoder, deframer, mac, line, features, dropped_to_host, .. } = self;
            decoder.push_word(word, |ev| {
                match ev {
                    LineEvent::Carrier => line.busy = true,
                    LineEvent::Idle => {
                        line.busy = false;
                        line.idle_since = now;
                    }
                    _ => {}
                }
                deframer.line(ev, |rx| {
                    let (data, status) = match rx {
                        RxEvent::Frame { data, crc_ok } => {
                            if let Some(a) = mac.on_frame(data, crc_ok) {
                                action = Some(a);
                            }
                            let status = if !crc_ok && *features & FEATURE_CHECK_CRC != 0 {
                                RxStatus::CrcError
                            } else {
                                RxStatus::FrameDone
                            };
                            (data, status)
                        }
                        RxEvent::FramingError => (&[][..], RxStatus::FramingError),
                        RxEvent::Aborted => (&[][..], RxStatus::Aborted),
                    };
                    let frame = Frame::from_slice(data).unwrap_or_default();
                    if FROM_LINK.try_send(FromLink { frame, status }).is_err() {
                        *dropped_to_host += 1;
                    }
                });
            });
        }
        action
    }

    fn command(&mut self, cmd: ToLink) {
        match cmd {
            ToLink::SetNodeIds(ids) => self.mac.set_node_ids(ids),
            ToLink::SetFeatures(f) => self.features = f,
            ToLink::Transmit(mut frame) => {
                if frame.len() < frame::CONTROL_FRAME_LEN {
                    warn!("dropping short frame from host");
                    return;
                }
                if self.features & FEATURE_CALC_CRC != 0 {
                    frame.fill_fcs();
                }
                // Control frames start a dialog and get the sync pulse; data
                // frames follow a CTS (or broadcast RTS) and do not.
                let control = frame.is_control();
                let symbols_ok = if control {
                    self.bufs.control.encode(frame.as_slice(), true)
                } else {
                    self.bufs.data.encode(frame.as_slice(), false)
                };
                if !symbols_ok {
                    warn!("frame too long to encode");
                    return;
                }
                self.mac.start(frame.dest(), frame.src(), control);
            }
        }
    }

    fn perform(&mut self, action: Action) {
        let (buf, mac) = match action {
            Action::Respond(reply) => {
                debug!("reply {=[u8]:02x}", reply);
                self.bufs.response.encode(&reply, false);
                (Buf::Response, false)
            }
            Action::SendControl => (Buf::Control, true),
            Action::SendRts(rts) => {
                self.bufs.control.encode(&rts, true);
                (Buf::Control, true)
            }
            Action::SendData => (Buf::Data, true),
        };
        self.job = Some(TxJob { buf, pos: 0, draining: false, mac });
        self.service_tx();
    }

    fn service_tx(&mut self) {
        let Some(job) = &mut self.job else { return };
        let words = self.bufs.get(job.buf);
        while job.pos < words.len() && self.tx_sm.tx().try_push(words[job.pos]) {
            job.pos += 1;
        }
        if job.pos < words.len() || !self.tx_sm.tx().empty() {
            return;
        }
        if !job.draining {
            // The last word is in the output shift register. The stall flag is
            // sticky and was set while we were idle: clear it now, and the
            // state machine sets it again once that word is shifted out.
            let _ = self.tx_sm.tx().stalled();
            job.draining = true;
            return;
        }
        if !self.tx_sm.tx().stalled() {
            return;
        }

        // Transmission finished: discard what we heard of ourselves.
        let was_mac = job.mac;
        self.job = None;
        while self.rx_sm.rx().try_pull().is_some() {}
        self.decoder.reset();
        self.deframer.reset();
        let now = now_us();
        self.line = Line { busy: false, idle_since: now };
        if was_mac {
            self.mac.on_tx_done(now);
            if let Some(outcome) = self.mac.take_outcome() {
                debug!("tx {} ({})", outcome, self.mac.stats);
            }
        }
    }
}
