//! FM0 (bi-phase space) line coding as used by LocalTalk at 230.4 kbit/s.
//!
//! Every bit cell starts with a transition. A `0` has an extra transition in
//! the middle of the cell, a `1` does not. The idle line has no transitions.
//!
//! Transmit side: [`SymbolWriter`] produces half-bit *symbols* for a PIO
//! program that shifts out two pins per symbol (bit 0 = line level, bit 1 =
//! driver enable), 16 symbols per 32-bit word, least significant first.
//!
//! Receive side: [`Decoder`] consumes 32-bit words of line samples taken at
//! [`SAMPLES_PER_BIT`] times the bit rate (earliest sample in the MSB) and
//! classifies the intervals between edges.

/// Receive oversampling factor.
pub const SAMPLES_PER_BIT: u32 = 8;

/// Nominal LocalTalk bit rate.
pub const BIT_RATE: u32 = 230_400;

/// Symbols (half bits) per 32-bit transmit word.
pub const SYMBOLS_PER_WORD: usize = 16;

const SYM_DRIVE: u32 = 0b10;

/// Writes transmit symbols into a word buffer.
pub struct SymbolWriter<'a> {
    buf: &'a mut [u32],
    word: usize,
    shift: u32,
    level: bool,
    overflow: bool,
}

impl<'a> SymbolWriter<'a> {
    pub fn new(buf: &'a mut [u32]) -> Self {
        if let Some(first) = buf.first_mut() {
            *first = 0;
        }
        // Start "high" so the first bit cell transition drives the line low,
        // as TashTalk does.
        Self { buf, word: 0, shift: 0, level: true, overflow: false }
    }

    fn push(&mut self, sym: u32) {
        if self.word >= self.buf.len() {
            self.overflow = true;
            return;
        }
        self.buf[self.word] |= sym << self.shift;
        self.shift += 2;
        if self.shift == 32 {
            self.shift = 0;
            self.word += 1;
            if let Some(w) = self.buf.get_mut(self.word) {
                *w = 0;
            }
        }
    }

    /// Drive the line at `level` for one half bit.
    pub fn drive(&mut self, level: bool) {
        self.level = level;
        self.push(SYM_DRIVE | level as u32);
    }

    /// Release the line (driver disabled) for one half bit.
    pub fn release(&mut self) {
        self.push(0);
    }

    /// One FM0-coded bit cell.
    pub fn bit(&mut self, bit: bool) {
        let first = !self.level;
        let second = if bit { first } else { !first };
        self.drive(first);
        self.drive(second);
    }

    pub fn level(&self) -> bool {
        self.level
    }

    /// Pads the last word with released symbols and returns the number of
    /// words written, or `None` if the buffer was too small. Always ends with
    /// at least one released symbol so the driver is left disabled.
    pub fn finish(mut self) -> Option<usize> {
        self.release();
        while self.shift != 0 {
            self.release();
        }
        (!self.overflow).then_some(self.word)
    }
}

/// Something the receiver saw on the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LineEvent {
    /// First transition after the line was idle.
    Carrier,
    Bit(bool),
    /// A full-cell interval where the second half of a `0` was expected.
    Desync,
    /// No transition for more than 1.5 bit times: the line is idle.
    Idle,
}

// Interval classification thresholds, in samples.
const HALF_MAX: u32 = SAMPLES_PER_BIT * 3 / 4 - 1; // 5 at 8x
const FULL_MAX: u32 = SAMPLES_PER_BIT * 3 / 2; // 12 at 8x

/// FM0 decoder working on oversampled line data.
#[derive(Default)]
pub struct Decoder {
    last_sample: bool,
    since_edge: u32,
    carrier: bool,
    mid: bool,
}

impl Decoder {
    pub const fn new() -> Self {
        Self { last_sample: false, since_edge: u32::MAX / 2, carrier: false, mid: false }
    }

    /// True while transitions are being seen.
    pub fn carrier(&self) -> bool {
        self.carrier
    }

    /// Forget the current bit sync (e.g. after our own transmission).
    pub fn reset(&mut self) {
        self.carrier = false;
        self.mid = false;
        self.since_edge = u32::MAX / 2;
    }

    /// Feeds 32 samples, earliest in the MSB.
    #[inline]
    pub fn push_word(&mut self, word: u32, mut sink: impl FnMut(LineEvent)) {
        // Bit i (from the MSB) of `edges` is set when sample i differs from
        // the sample before it.
        let prev = (self.last_sample as u32) << 31;
        let mut edges = word ^ ((word >> 1) | prev);
        self.last_sample = word & 1 != 0;

        let mut pos = 0;
        while edges != 0 {
            let k = edges.leading_zeros();
            let interval = self.since_edge + (k - pos);
            self.edge(interval, &mut sink);
            self.since_edge = 0;
            pos = k;
            edges &= !(0x8000_0000 >> k);
        }
        self.since_edge = self.since_edge.saturating_add(32 - pos).min(u32::MAX / 2);

        if self.carrier && self.since_edge > FULL_MAX {
            self.carrier = false;
            self.mid = false;
            sink(LineEvent::Idle);
        }
    }

    #[inline]
    fn edge(&mut self, interval: u32, sink: &mut impl FnMut(LineEvent)) {
        if !self.carrier || interval > FULL_MAX {
            if self.carrier {
                sink(LineEvent::Idle);
            }
            self.carrier = true;
            self.mid = false;
            sink(LineEvent::Carrier);
        } else if interval <= HALF_MAX {
            if self.mid {
                self.mid = false;
                sink(LineEvent::Bit(false));
            } else {
                self.mid = true;
            }
        } else if self.mid {
            self.mid = false;
            sink(LineEvent::Desync);
        } else {
            sink(LineEvent::Bit(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders transmit symbols into line samples at `SAMPLES_PER_BIT`,
    /// holding the last driven level while the driver is off.
    fn symbols_to_samples(words: &[u32], idle_level: bool) -> Vec<bool> {
        let mut out = Vec::new();
        let mut line = idle_level;
        for w in words {
            for i in 0..SYMBOLS_PER_WORD {
                let sym = (w >> (2 * i)) & 3;
                if sym & SYM_DRIVE != 0 {
                    line = sym & 1 != 0;
                }
                for _ in 0..SAMPLES_PER_BIT / 2 {
                    out.push(line);
                }
            }
        }
        out
    }

    fn pack(samples: &[bool]) -> Vec<u32> {
        samples
            .chunks(32)
            .map(|c| {
                let mut w = 0u32;
                for (i, &s) in c.iter().enumerate() {
                    w |= (s as u32) << (31 - i);
                }
                // Short final chunk: repeat the last sample.
                for i in c.len()..32 {
                    w |= (*c.last().unwrap() as u32) << (31 - i);
                }
                w
            })
            .collect()
    }

    #[test]
    fn bits_round_trip() {
        let bits = [true, false, false, true, true, true, false, true, false, false, true];
        let mut buf = [0u32; 8];
        let mut w = SymbolWriter::new(&mut buf);
        for &b in &bits {
            w.bit(b);
        }
        let n = w.finish().unwrap();
        let mut samples = vec![true; 16]; // idle before
        samples.extend(symbols_to_samples(&buf[..n], true));
        samples.extend(std::iter::repeat_n(*samples.last().unwrap(), 64));

        let mut dec = Decoder::new();
        let mut events = Vec::new();
        for word in pack(&samples) {
            dec.push_word(word, |e| events.push(e));
        }
        assert_eq!(events[0], LineEvent::Carrier);
        let got: Vec<bool> = events
            .iter()
            .filter_map(|e| if let LineEvent::Bit(b) = e { Some(*b) } else { None })
            .collect();
        // The first cell's leading edge marks carrier start. The last bit has
        // no closing transition before the line is released, so it is lost
        // (on the wire this is one of the trailing abort ones).
        assert_eq!(&got[..], &bits[..bits.len() - 1]);
        assert_eq!(*events.last().unwrap(), LineEvent::Idle);
    }
}
