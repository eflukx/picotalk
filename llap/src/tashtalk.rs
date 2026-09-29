//! The TashTalk UART protocol, so existing hosts (AirTalk, tashtalkd,
//! TashRouter, MultiTalk) can use this implementation unchanged.
//!
//! Host to device: command bytes
//! * `0x00` no-op
//! * `0x01` transmit frame: followed by the frame including FCS; its length
//!   is inferred from the header
//! * `0x02` set node IDs: followed by a 32-byte bitmap
//! * `0x03` set features: followed by one byte
//!
//! Device to host: every received frame, with `0x00` escaped as `0x00 0xFF`,
//! followed by `0x00` and a status byte.

use crate::frame::{self, Frame, MAX_FRAME};

pub const CMD_NOP: u8 = 0x00;
pub const CMD_TRANSMIT: u8 = 0x01;
pub const CMD_SET_NODE_IDS: u8 = 0x02;
pub const CMD_SET_FEATURES: u8 = 0x03;

/// Feature bit: compute the FCS of outgoing frames ourselves.
pub const FEATURE_CALC_CRC: u8 = 0x80;
/// Feature bit: report frames with a bad FCS as [`RxStatus::CrcError`].
pub const FEATURE_CHECK_CRC: u8 = 0x40;

pub const ESCAPE: u8 = 0x00;
const LITERAL_ZERO: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RxStatus {
    FrameDone = 0xFD,
    FramingError = 0xFE,
    Aborted = 0xFA,
    CrcError = 0xFC,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Transmit(&'a Frame),
    SetNodeIds([u8; 32]),
    SetFeatures(u8),
}

enum State {
    Command,
    Frame { expected: usize },
    NodeIds { n: usize },
    Features,
}

/// Incremental parser for the host-to-device byte stream.
pub struct CommandParser {
    state: State,
    frame: Frame,
    node_ids: [u8; 32],
    /// Frames longer than we can buffer are consumed and dropped.
    overlong: usize,
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandParser {
    pub const fn new() -> Self {
        Self { state: State::Command, frame: Frame::new(), node_ids: [0; 32], overlong: 0 }
    }

    fn push(&mut self, byte: u8) -> Option<Command<'_>> {
        match self.state {
            State::Command => {
                match byte {
                    CMD_TRANSMIT => {
                        self.frame.clear();
                        self.state = State::Frame { expected: frame::CONTROL_FRAME_LEN };
                    }
                    CMD_SET_NODE_IDS => self.state = State::NodeIds { n: 0 },
                    CMD_SET_FEATURES => self.state = State::Features,
                    _ => {}
                }
                None
            }
            State::Frame { mut expected } => {
                self.frame.push(byte);
                let len = self.frame.len();
                if len == frame::CONTROL_FRAME_LEN {
                    expected = frame::total_len(self.frame.as_slice());
                    if expected > MAX_FRAME {
                        self.overlong = expected - len;
                        self.state = State::Command;
                        return None;
                    }
                }
                if len >= expected {
                    self.state = State::Command;
                    return Some(Command::Transmit(&self.frame));
                }
                self.state = State::Frame { expected };
                None
            }
            State::NodeIds { n } => {
                self.node_ids[n] = byte;
                if n + 1 == 32 {
                    self.state = State::Command;
                    Some(Command::SetNodeIds(self.node_ids))
                } else {
                    self.state = State::NodeIds { n: n + 1 };
                    None
                }
            }
            State::Features => {
                self.state = State::Command;
                Some(Command::SetFeatures(byte))
            }
        }
    }

    /// Feeds one byte from the host; returns a command once it is complete.
    pub fn feed(&mut self, byte: u8) -> Option<Command<'_>> {
        if self.overlong > 0 {
            self.overlong -= 1;
            return None;
        }
        self.push(byte)
    }
}

/// Encodes a received frame (or error) for the host.
pub fn encode_rx(data: &[u8], status: RxStatus, mut out: impl FnMut(u8)) {
    for &b in data {
        out(b);
        if b == ESCAPE {
            out(LITERAL_ZERO);
        }
    }
    out(ESCAPE);
    out(status as u8);
}

/// Worst-case encoded size of a frame of `len` bytes.
pub const fn max_encoded_len(len: usize) -> usize {
    len * 2 + 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(bytes: &[u8]) -> Vec<String> {
        let mut p = CommandParser::new();
        let mut out = Vec::new();
        for &b in bytes {
            if let Some(cmd) = p.feed(b) {
                out.push(format!("{cmd:?}"));
            }
        }
        out
    }

    #[test]
    fn parses_commands() {
        let mut bytes = vec![0x00, 0x00, CMD_SET_FEATURES, 0xC0, CMD_TRANSMIT, 1, 2, 0x84, 0xAA, 0xBB];
        bytes.push(CMD_TRANSMIT);
        bytes.extend([1, 2, 0x01, 0x00, 0x04, 0xDE, 0xAD, 0x11, 0x22]);
        bytes.push(CMD_SET_NODE_IDS);
        bytes.extend([0x55; 32]);
        let cmds = parse_all(&bytes);
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[0], "SetFeatures(192)");
        assert!(cmds[1].contains("01, 02, 84, aa, bb"));
        assert!(cmds[2].contains("01, 02, 01, 00, 04, de, ad, 11, 22"));
        assert!(cmds[3].starts_with("SetNodeIds([85, 85"));
    }

    #[test]
    fn escapes_zero() {
        let mut out = Vec::new();
        encode_rx(&[1, 0, 2], RxStatus::FrameDone, |b| out.push(b));
        assert_eq!(out, [1, 0, 0xFF, 2, 0, 0xFD]);
    }
}
