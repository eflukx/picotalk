//! HDLC-style framing used by LLAP: flag bytes (0x7E), zero-bit stuffing
//! after five consecutive ones, bytes sent least significant bit first.

use crate::crc::Crc;
use crate::fm0::{LineEvent, SymbolWriter};
use crate::frame::Frame;

const FLAG: u8 = 0x7E;

/// Consecutive `1` bits sent before the opening flags (clock run-in).
const PREAMBLE_ONES: usize = 4;
/// Opening flags. LLAP needs two; the Mac sends three and the Apple IIgs
/// depends on that.
const OPENING_FLAGS: usize = 3;
/// `1` bits after the closing flag: LLAP's 12 to 18 bit abort sequence.
const TRAILING_ONES: usize = 13;
/// Half bits the line is driven low for the synchronisation pulse.
const SYNC_DRIVE: usize = 3;
/// Half bits the line is then released for (at least two bit times).
const SYNC_RELEASE: usize = 7;

fn raw_byte(w: &mut SymbolWriter<'_>, byte: u8) {
    for i in 0..8 {
        w.bit(byte >> i & 1 != 0);
    }
}

/// Encodes `frame` (which must already carry its FCS) into transmit symbols.
///
/// `sync` prepends the synchronisation pulse LLAP requires at the start of a
/// dialog (before an RTS, ENQ or lone control frame, but not before a CTS or
/// the data frame that follows a CTS).
///
/// Returns the number of words used, or `None` if `buf` is too small.
pub fn encode_frame(buf: &mut [u32], frame: &[u8], sync: bool) -> Option<usize> {
    let mut w = SymbolWriter::new(buf);
    if sync {
        for _ in 0..SYNC_DRIVE {
            w.drive(false);
        }
        for _ in 0..SYNC_RELEASE {
            w.release();
        }
    }
    for _ in 0..PREAMBLE_ONES {
        w.bit(true);
    }
    for _ in 0..OPENING_FLAGS {
        raw_byte(&mut w, FLAG);
    }
    let mut ones = 0;
    for &byte in frame {
        for i in 0..8 {
            let bit = byte >> i & 1 != 0;
            w.bit(bit);
            if bit {
                ones += 1;
                if ones == 5 {
                    w.bit(false);
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
    }
    raw_byte(&mut w, FLAG);
    for _ in 0..TRAILING_ONES {
        w.bit(true);
    }
    // Leave the bus driven low before letting go, as TashTalk does.
    if w.level() {
        w.drive(false);
        w.drive(false);
    }
    w.finish()
}

/// Worst-case number of transmit words for a frame of `len` bytes.
pub const fn max_words(len: usize) -> usize {
    let bits = PREAMBLE_ONES + OPENING_FLAGS * 8 + len * 8 * 6 / 5 + 1 + 8 + TRAILING_ONES + 1;
    let symbols = SYNC_DRIVE + SYNC_RELEASE + bits * 2 + 1;
    symbols.div_ceil(crate::fm0::SYMBOLS_PER_WORD) + 1
}

/// Result of receiving a frame.
#[derive(Debug, PartialEq, Eq)]
pub enum RxEvent<'a> {
    /// A complete frame, including its FCS. Reported once the line has gone
    /// idle after the closing flag.
    Frame { data: &'a [u8], crc_ok: bool },
    /// Seven or more ones, a misaligned flag or lost bit sync mid-frame.
    FramingError,
    /// The sender stopped (line went idle) without a closing flag.
    Aborted,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Waiting for a flag.
    Hunt,
    /// Seen a flag, no data yet.
    Open,
    /// Receiving frame bytes.
    Data,
    /// Closing flag seen; waiting for the line to go idle.
    Done,
}

pub struct Deframer {
    state: State,
    ones: u8,
    byte: u8,
    nbits: u8,
    crc: Crc,
    frame: Frame,
}

impl Default for Deframer {
    fn default() -> Self {
        Self::new()
    }
}

impl Deframer {
    pub const fn new() -> Self {
        Self { state: State::Hunt, ones: 0, byte: 0, nbits: 0, crc: Crc::new(), frame: Frame::new() }
    }

    pub fn reset(&mut self) {
        self.state = State::Hunt;
        self.ones = 0;
    }

    /// True between the first byte of a frame and the line going idle.
    pub fn in_frame(&self) -> bool {
        matches!(self.state, State::Data | State::Done)
    }

    pub fn line(&mut self, ev: LineEvent, mut sink: impl FnMut(RxEvent<'_>)) {
        match ev {
            LineEvent::Carrier => self.reset(),
            LineEvent::Bit(b) => self.bit(b, &mut sink),
            LineEvent::Desync => {
                if self.state == State::Data {
                    sink(RxEvent::FramingError);
                }
                if self.state != State::Done {
                    self.reset();
                }
            }
            LineEvent::Idle => {
                match self.state {
                    State::Data => sink(RxEvent::Aborted),
                    State::Done => sink(RxEvent::Frame {
                        data: self.frame.as_slice(),
                        crc_ok: self.crc.residue_ok(),
                    }),
                    _ => {}
                }
                self.reset();
            }
        }
    }

    fn bit(&mut self, bit: bool, sink: &mut impl FnMut(RxEvent<'_>)) {
        if self.state == State::Done {
            return;
        }
        if bit {
            self.ones = self.ones.saturating_add(1);
            if self.ones >= 7 {
                if self.state == State::Data {
                    sink(RxEvent::FramingError);
                }
                self.state = State::Hunt;
                return;
            }
            self.data_bit(true, sink);
        } else {
            let ones = self.ones;
            self.ones = 0;
            match ones {
                6 => self.flag(sink),
                5 => {} // stuffed zero
                _ => self.data_bit(false, sink),
            }
        }
    }

    fn data_bit(&mut self, bit: bool, sink: &mut impl FnMut(RxEvent<'_>)) {
        if self.state == State::Hunt {
            return;
        }
        self.byte = (self.byte >> 1) | ((bit as u8) << 7);
        self.nbits += 1;
        if self.nbits == 8 {
            self.nbits = 0;
            if self.state == State::Open {
                self.state = State::Data;
                self.frame.clear();
                self.crc = Crc::new();
            }
            if !self.frame.push(self.byte) {
                sink(RxEvent::FramingError);
                self.state = State::Hunt;
                return;
            }
            self.crc.update(self.byte);
        }
    }

    fn flag(&mut self, sink: &mut impl FnMut(RxEvent<'_>)) {
        // The flag's leading zero and six ones went in as data bits.
        if self.state == State::Data {
            if self.nbits == 7 {
                self.state = State::Done;
                return;
            }
            sink(RxEvent::FramingError);
        }
        self.state = State::Open;
        self.nbits = 0;
        self.byte = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fm0::LineEvent;

    /// Recovers the FM0 bit stream directly from the driven symbols.
    fn bits_of(words: &[u32]) -> Vec<bool> {
        let syms: Vec<u32> = words
            .iter()
            .flat_map(|w| (0..16).map(move |i| (w >> (2 * i)) & 3))
            .take_while(|s| s & 2 != 0)
            .collect();
        syms.as_chunks::<2>().0.iter().map(|c| (c[0] & 1) == (c[1] & 1)).collect()
    }

    #[test]
    fn deframes_encoded_bits() {
        let frame = [0x7Eu8, 0xFF, 0x1F, 0x3E, 0x00, 0xF8];
        let mut buf = [0u32; 64];
        let n = encode_frame(&mut buf, &frame, false).unwrap();
        let bits = bits_of(&buf[..n]);

        let mut d = Deframer::new();
        let mut got = None;
        d.line(LineEvent::Carrier, |_| {});
        for b in bits {
            d.line(LineEvent::Bit(b), |e| panic!("unexpected {e:?}"));
        }
        d.line(LineEvent::Idle, |e| {
            if let RxEvent::Frame { data, .. } = e {
                got = Some(data.to_vec());
            }
        });
        assert_eq!(got.as_deref(), Some(&frame[..]));
    }

    #[test]
    fn seven_ones_mid_frame_is_framing_error() {
        let mut d = Deframer::new();
        let mut events = Vec::new();
        let flag = [false, true, true, true, true, true, true, false];
        let feed = |d: &mut Deframer, b: bool, ev: &mut Vec<String>| {
            d.line(LineEvent::Bit(b), |e| ev.push(format!("{e:?}")));
        };
        for b in flag {
            feed(&mut d, b, &mut events);
        }
        for _ in 0..8 {
            feed(&mut d, false, &mut events);
        }
        for _ in 0..7 {
            feed(&mut d, true, &mut events);
        }
        assert_eq!(events, vec!["FramingError".to_string()]);
    }
}
