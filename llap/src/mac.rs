//! LLAP medium access control, following the algorithm in *Inside AppleTalk*
//! as implemented by TashTalk:
//!
//! * Wait for the inter-dialog gap (400 µs) plus a random backoff (multiples
//!   of 100 µs, masked by an adaptive backoff mask) of idle line. Activity
//!   during the wait is a *deferral*.
//! * Control frames are then sent directly. Data frames are preceded by an
//!   RTS. Directed data frames are sent when the destination answers with a
//!   CTS within the inter-frame gap (200 µs); broadcast data frames are sent
//!   after 200 µs of silence. No CTS, or activity during the silence, is a
//!   *collision*.
//! * Deferrals and collisions widen the local backoff mask; their history
//!   over the last eight frames adapts the global one. Give up after 32
//!   attempts.
//!
//! The MAC also answers ENQ (with ACK) and RTS (with CTS) addressed to any
//! node ID in its node bitmap, which lets a bridge proxy remote nodes whose
//! replies could never arrive within the inter-frame gap.
//!
//! The MAC is purely logical: the driver tells it about received frames,
//! line state and the end of its own transmissions, and performs the
//! [`Action`]s it returns.

use crate::frame::{self, BROADCAST, LLAP_ACK, LLAP_CTS, LLAP_ENQ, LLAP_RTS};

pub const IDG_US: u32 = 400;
pub const IFG_US: u32 = 200;
pub const BACKOFF_UNIT_US: u32 = 100;
pub const MAX_ATTEMPTS: u8 = 32;

/// Something the driver must transmit now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Action {
    /// Reply to a received frame, without sync pulse. Not part of the
    /// pending transmission: do not report its completion via
    /// [`Mac::on_tx_done`].
    Respond([u8; 5]),
    /// Send the pending control frame, with sync pulse.
    SendControl,
    /// Send an RTS for the pending data frame, with sync pulse.
    SendRts([u8; 5]),
    /// Send the pending data frame, without sync pulse.
    SendData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Outcome {
    Sent,
    GaveUp,
}

/// Line state as seen by the receiver.
#[derive(Clone, Copy, Debug, Default)]
pub struct Line {
    /// Transitions are currently being seen.
    pub busy: bool,
    /// When the line last became idle (µs).
    pub idle_since: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// Waiting for `wait_us` of idle line. `busy_seen` is set while the
    /// deferral for the current burst of activity has been counted.
    Waiting { wait_us: u32, busy_seen: bool },
    SendingRts,
    AwaitCts { until: u64 },
    AwaitSilence { until: u64 },
    SendingFinal,
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    dest: u8,
    src: u8,
    control: bool,
}

#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Stats {
    pub sent: u32,
    pub gave_up: u32,
    pub deferrals: u32,
    pub collisions: u32,
    pub responses: u32,
}

pub struct Mac {
    nodes: [u8; 32],
    state: State,
    pending: Option<Pending>,
    global_backoff: u8,
    local_backoff: u8,
    collision_history: u8,
    deferral_history: u8,
    attempts: u8,
    rng: u32,
    outcome: Option<Outcome>,
    pub stats: Stats,
}

impl Mac {
    pub fn new(seed: u32) -> Self {
        let mut mac = Self {
            nodes: [0; 32],
            state: State::Idle,
            pending: None,
            global_backoff: 0,
            local_backoff: 0,
            collision_history: 0,
            deferral_history: 0,
            attempts: 0,
            rng: seed | 1,
            outcome: None,
            stats: Stats::default(),
        };
        mac.prepare_next();
        mac
    }

    /// Sets the node IDs to answer ENQ and RTS for (bit n of byte n/8, LSB
    /// first, as in the TashTalk protocol).
    pub fn set_node_ids(&mut self, bitmap: [u8; 32]) {
        self.nodes = bitmap;
    }

    pub fn represents(&self, node: u8) -> bool {
        self.nodes[node as usize / 8] & (1 << (node % 8)) != 0
    }

    /// True when a new frame can be handed to [`Mac::start`].
    pub fn is_idle(&self) -> bool {
        self.pending.is_none()
    }

    /// The result of the last finished transmission, once.
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// Begins transmitting a frame. The driver keeps the frame; the MAC only
    /// needs its addresses and kind.
    pub fn start(&mut self, dest: u8, src: u8, control: bool) {
        self.pending = Some(Pending { dest, src, control });
        self.local_backoff = self.global_backoff;
        self.attempts = MAX_ATTEMPTS;
        self.state = State::Waiting { wait_us: self.backoff_wait(), busy_seen: false };
    }

    /// A frame was received (reported once the line went idle after it).
    pub fn on_frame(&mut self, data: &[u8], crc_ok: bool) -> Option<Action> {
        if !crc_ok || data.len() < 3 || !frame::is_control(data[2]) {
            if let State::AwaitCts { .. } = self.state {
                self.collided();
            }
            return None;
        }
        let (dest, src, ty) = (data[0], data[1], data[2]);

        if let (State::AwaitCts { .. }, Some(p)) = (self.state, self.pending) {
            if ty == LLAP_CTS && src == p.dest && dest == p.src {
                self.state = State::SendingFinal;
                return Some(Action::SendData);
            }
            // Someone else's dialog got in the way of ours.
            self.collided();
        }

        if !self.represents(dest) {
            return None;
        }
        let reply = match ty {
            LLAP_ENQ => frame::control_frame(dest, dest, LLAP_ACK),
            LLAP_RTS => frame::control_frame(src, dest, LLAP_CTS),
            _ => return None,
        };
        self.stats.responses += 1;
        Some(Action::Respond(reply))
    }

    /// Advances timers. Call often, whenever no transmission is in progress.
    pub fn poll(&mut self, now: u64, line: Line) -> Option<Action> {
        match self.state {
            State::Waiting { wait_us, busy_seen } => {
                if line.busy {
                    if !busy_seen {
                        self.deferred();
                        if let State::Waiting { busy_seen, .. } = &mut self.state {
                            *busy_seen = true;
                        }
                    }
                    return None;
                }
                if busy_seen {
                    self.state = State::Waiting { wait_us, busy_seen: false };
                }
                if now.saturating_sub(line.idle_since) < wait_us as u64 {
                    return None;
                }
                let p = self.pending?;
                if p.control {
                    self.state = State::SendingFinal;
                    Some(Action::SendControl)
                } else {
                    self.state = State::SendingRts;
                    Some(Action::SendRts(frame::control_frame(p.dest, p.src, LLAP_RTS)))
                }
            }
            State::AwaitCts { until } => {
                if !line.busy && now >= until {
                    self.collided();
                }
                None
            }
            State::AwaitSilence { until } => {
                if line.busy {
                    self.collided();
                    None
                } else if now >= until {
                    self.state = State::SendingFinal;
                    Some(Action::SendData)
                } else {
                    None
                }
            }
            State::Idle | State::SendingRts | State::SendingFinal => None,
        }
    }

    /// Our transmission of an RTS, control or data frame has finished.
    pub fn on_tx_done(&mut self, now: u64) {
        match (self.state, self.pending) {
            (State::SendingRts, Some(p)) => {
                let until = now + IFG_US as u64;
                self.state = if p.dest == BROADCAST {
                    State::AwaitSilence { until }
                } else {
                    State::AwaitCts { until }
                };
            }
            (State::SendingFinal, _) => {
                self.stats.sent += 1;
                self.finish(Outcome::Sent);
            }
            _ => {}
        }
    }

    fn deferred(&mut self) {
        self.stats.deferrals += 1;
        self.deferral_history |= 1;
        self.local_backoff |= 1;
        self.try_again();
    }

    fn collided(&mut self) {
        self.stats.collisions += 1;
        self.collision_history |= 1;
        self.local_backoff = ((self.local_backoff << 1) | 1) & 0x0F;
        self.try_again();
    }

    fn try_again(&mut self) {
        self.attempts -= 1;
        if self.attempts == 0 {
            self.stats.gave_up += 1;
            self.finish(Outcome::GaveUp);
        } else {
            self.state = State::Waiting { wait_us: self.backoff_wait(), busy_seen: false };
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        self.pending = None;
        self.state = State::Idle;
        self.outcome = Some(outcome);
        self.prepare_next();
    }

    /// Adapts the global backoff to recent history (TashTalk's
    /// `PrepForNextFrame`).
    fn prepare_next(&mut self) {
        if self.collision_history.count_ones() > 2 {
            self.global_backoff = ((self.global_backoff << 1) | 1) & 0x0F;
            self.collision_history = 0;
        }
        if self.deferral_history.count_ones() < 2 {
            self.global_backoff >>= 1;
            self.deferral_history = 0xFF;
        }
        self.collision_history <<= 1;
        self.deferral_history <<= 1;
    }

    fn backoff_wait(&mut self) -> u32 {
        // xorshift32
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        let slots = (self.rng as u8) & self.local_backoff;
        IDG_US + slots as u32 * BACKOFF_UNIT_US
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: Line = Line { busy: false, idle_since: 0 };

    #[test]
    fn answers_rts_and_enq_for_represented_nodes() {
        let mut mac = Mac::new(1);
        let mut nodes = [0u8; 32];
        nodes[10 / 8] |= 1 << (10 % 8);
        mac.set_node_ids(nodes);

        let rts = frame::control_frame(10, 3, LLAP_RTS);
        assert_eq!(mac.on_frame(&rts, true), Some(Action::Respond(frame::control_frame(3, 10, LLAP_CTS))));

        let enq = frame::control_frame(10, 10, LLAP_ENQ);
        assert_eq!(mac.on_frame(&enq, true), Some(Action::Respond(frame::control_frame(10, 10, LLAP_ACK))));

        let other = frame::control_frame(11, 3, LLAP_RTS);
        assert_eq!(mac.on_frame(&other, true), None);
        assert_eq!(mac.on_frame(&rts, false), None);
    }

    #[test]
    fn directed_data_frame_dialog() {
        let mut mac = Mac::new(7);
        mac.start(2, 5, false);
        // Line idle for a long time: RTS goes out on the first poll.
        let now = 10_000;
        let rts = frame::control_frame(2, 5, LLAP_RTS);
        assert_eq!(mac.poll(now, IDLE), Some(Action::SendRts(rts)));
        mac.on_tx_done(now + 100);
        assert_eq!(mac.poll(now + 150, IDLE), None);
        let cts = frame::control_frame(5, 2, LLAP_CTS);
        assert_eq!(mac.on_frame(&cts, true), Some(Action::SendData));
        mac.on_tx_done(now + 2000);
        assert_eq!(mac.take_outcome(), Some(Outcome::Sent));
        assert!(mac.is_idle());
    }

    #[test]
    fn missing_cts_is_a_collision_and_retries() {
        let mut mac = Mac::new(7);
        mac.start(2, 5, false);
        let mut now = 10_000;
        assert!(matches!(mac.poll(now, IDLE), Some(Action::SendRts(_))));
        mac.on_tx_done(now);
        now += IFG_US as u64;
        assert_eq!(mac.poll(now, IDLE), None);
        assert_eq!(mac.stats.collisions, 1);
        // Retry needs at least the IDG of idle line again.
        let line = Line { busy: false, idle_since: now };
        assert_eq!(mac.poll(now + 100, line), None);
        assert!(matches!(mac.poll(now + 2000, line), Some(Action::SendRts(_))));
    }

    #[test]
    fn broadcast_waits_for_silence() {
        let mut mac = Mac::new(3);
        mac.start(BROADCAST, 5, false);
        assert!(matches!(mac.poll(5_000, IDLE), Some(Action::SendRts(_))));
        mac.on_tx_done(5_000);
        assert_eq!(mac.poll(5_100, IDLE), None);
        assert_eq!(mac.poll(5_200, IDLE), Some(Action::SendData));
    }

    #[test]
    fn activity_while_waiting_is_a_deferral() {
        let mut mac = Mac::new(3);
        mac.start(1, 5, true);
        assert_eq!(mac.poll(100, Line { busy: true, idle_since: 0 }), None);
        assert_eq!(mac.poll(150, Line { busy: true, idle_since: 0 }), None);
        assert_eq!(mac.stats.deferrals, 1);
        let line = Line { busy: false, idle_since: 200 };
        assert_eq!(mac.poll(300, line), None);
        assert_eq!(mac.poll(200 + 400 + 15 * 100, line), Some(Action::SendControl));
    }

    #[test]
    fn gives_up_after_32_attempts() {
        let mut mac = Mac::new(3);
        mac.start(2, 5, false);
        let mut now = 0;
        let mut rts = 0;
        while !mac.is_idle() {
            now += 10_000;
            let line = Line { busy: false, idle_since: now - 5_000 };
            if let Some(Action::SendRts(_)) = mac.poll(now, line) {
                rts += 1;
                mac.on_tx_done(now);
                mac.poll(now + 1_000, Line { busy: false, idle_since: now });
            }
        }
        assert_eq!(rts, 32);
        assert_eq!(mac.take_outcome(), Some(Outcome::GaveUp));
    }
}
