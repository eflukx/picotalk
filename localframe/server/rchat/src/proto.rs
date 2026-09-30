//! The RChat wire protocol. Every message is an exactly-once ATP request
//! from a client to the hub (NBP type `RChat`); the hub never initiates.
//! Clients long-poll: the hub holds a POLL until there is something new or
//! [`HOLD`] has passed, so messages arrive as soon as they are posted.
//!
//! ```text
//! JOIN   01 00 nick(pstr)   → 01 st after(2) hub_nick(pstr)
//! SAY    02 00 text…        → 02 st
//! POLL   03 00 after(2)     → 03 st count 00 { id(2) nick(pstr) text(pstr) } × count
//! LEAVE  04 00              → 04 st
//! ```
//!
//! `st` is 0, or [`NOT_JOINED`] if the hub does not know the client (it
//! restarted, or dropped the client after [`CLIENT_TIMEOUT`]): join again.
//! Message IDs count up from 1 and wrap. JOIN returns the `after` to poll
//! with first, which replays the last few messages. Text is Mac Roman,
//! nick at most [`MAX_NICK`] and text at most [`MAX_TEXT`] bytes.

use std::time::Duration;

pub const TYPE: &str = "RChat";

pub const JOIN: u8 = 1;
pub const SAY: u8 = 2;
pub const POLL: u8 = 3;
pub const LEAVE: u8 = 4;

pub const OK: u8 = 0;
pub const NOT_JOINED: u8 = 1;

pub const MAX_NICK: usize = 31;
pub const MAX_TEXT: usize = 200;

/// How long the hub holds a POLL with nothing to say. Well under the Mac
/// client's ATP retry timeout.
pub const HOLD: Duration = Duration::from_secs(2);
/// A client that has not polled for this long has left.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: u16,
    pub nick: Vec<u8>,
    pub text: Vec<u8>,
}

impl Message {
    pub fn wire_len(&self) -> usize {
        2 + 1 + self.nick.len() + 1 + self.text.len()
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend(self.id.to_be_bytes());
        pstr(out, &self.nick);
        pstr(out, &self.text);
    }
}

pub fn pstr(out: &mut Vec<u8>, s: &[u8]) {
    let s = &s[..s.len().min(255)];
    out.push(s.len() as u8);
    out.extend(s);
}

/// Reads a Pascal string at the start of `b`: (string, rest).
pub fn read_pstr(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&n, rest) = b.split_first()?;
    Some((rest.get(..n as usize)?, &rest[n as usize..]))
}

/// Parses a POLL response body (after `03 st`).
pub fn parse_messages(b: &[u8]) -> Option<Vec<Message>> {
    let count = *b.first()?;
    let mut rest = b.get(2..)?;
    let mut out = Vec::new();
    for _ in 0..count {
        let id = u16::from_be_bytes([*rest.first()?, *rest.get(1)?]);
        let (nick, r) = read_pstr(&rest[2..])?;
        let (text, r) = read_pstr(r)?;
        out.push(Message { id, nick: nick.to_vec(), text: text.to_vec() });
        rest = r;
    }
    Some(out)
}

/// UTF-8 → Mac Roman, `?` for what Mac Roman lacks, control characters
/// dropped.
pub fn to_mac(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in s.chars().filter(|c| !c.is_control()) {
        let mut buf = [0u8; 4];
        let (bytes, _, unmappable) = encoding_rs::MACINTOSH.encode(c.encode_utf8(&mut buf));
        if unmappable {
            out.push(b'?');
        } else {
            out.extend(bytes.iter());
        }
    }
    out
}

pub fn from_mac(b: &[u8]) -> String {
    encoding_rs::MACINTOSH.decode_without_bom_handling(b).0.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_roundtrip() {
        let msgs = vec![
            Message { id: 7, nick: b"mac".to_vec(), text: b"hello".to_vec() },
            Message { id: 8, nick: b"pc".to_vec(), text: vec![] },
        ];
        let mut body = vec![2, 0];
        for m in &msgs {
            m.write(&mut body);
        }
        assert_eq!(body.len(), 2 + msgs.iter().map(Message::wire_len).sum::<usize>());
        assert_eq!(parse_messages(&body), Some(msgs));
        assert_eq!(parse_messages(&body[..body.len() - 1]), None);
    }

    #[test]
    fn mac_roman() {
        assert_eq!(to_mac("café ✓\n"), b"caf\x8E ?");
        assert_eq!(from_mac(b"caf\x8E\x07"), "café");
    }
}
