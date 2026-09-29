//! LToUDP bridging: LLAP frames carried in UDP multicast datagrams to
//! 239.192.76.84:1954, each prefixed with a 4-byte sender ID and without FCS.
//! This is what Mini vMac, AirTalk, TashRouter and MultiTalk speak.
//!
//! [`Bridge`] holds the forwarding policy (from AirTalk): RTS/CTS stay on
//! LocalTalk, LToUDP frames only go onto LocalTalk if they are broadcast or
//! for a node recently seen there, and nodes heard over LToUDP are proxied
//! so the link answers RTS/ENQ for them.

use crate::frame::{Frame, LLAP_CTS, LLAP_RTS};
use crate::nodes::NodeTable;

pub const GROUP: [u8; 4] = [239, 192, 76, 84];
pub const PORT: u16 = 1954;
/// Ethernet multicast address for [`GROUP`] (01:00:5e + low 23 bits).
pub const GROUP_MAC: [u8; 6] = [0x01, 0x00, 0x5E, GROUP[1] & 0x7F, GROUP[2], GROUP[3]];

pub const HEADER_LEN: usize = 4;
/// Largest datagram: header plus the largest LLAP frame without FCS.
pub const MAX_DATAGRAM: usize = HEADER_LEN + crate::frame::MAX_FRAME - 2;

/// How long a LocalTalk node stays worth forwarding to (seconds).
pub const LOCAL_MAX_AGE: u32 = 3600;
/// How long a remote node stays proxied (seconds).
pub const REMOTE_MAX_AGE: u32 = 1800;

pub struct Bridge {
    sender_id: [u8; 4],
    local: NodeTable,
    remote: NodeTable,
    proxied: [u8; 32],
}

impl Bridge {
    pub fn new(sender_id: u32) -> Self {
        Self {
            sender_id: sender_id.to_be_bytes(),
            local: NodeTable::new(),
            remote: NodeTable::new(),
            proxied: [0; 32],
        }
    }

    /// A good frame arrived from LocalTalk (`frame` includes the FCS).
    /// Writes the datagram to send into `out` and returns its length, or
    /// `None` if the frame stays local.
    pub fn from_localtalk(&mut self, frame: &[u8], now: u32, out: &mut [u8]) -> Option<usize> {
        if frame.len() < 5 {
            return None;
        }
        self.local.touch(frame[1], now);
        if frame.len() == 5 && (frame[2] == LLAP_RTS || frame[2] == LLAP_CTS) {
            return None;
        }
        let body = &frame[..frame.len() - 2];
        let len = HEADER_LEN + body.len();
        out.get_mut(..len)?;
        out[..HEADER_LEN].copy_from_slice(&self.sender_id);
        out[HEADER_LEN..len].copy_from_slice(body);
        Some(len)
    }

    /// A datagram arrived from LToUDP. Returns the frame (with FCS) to put on
    /// LocalTalk, if any.
    pub fn from_udp(&mut self, datagram: &[u8], now: u32) -> Option<Frame> {
        if datagram.len() < HEADER_LEN + 3 || datagram[..HEADER_LEN] == self.sender_id {
            return None;
        }
        let body = &datagram[HEADER_LEN..];
        let dest = body[0];
        if dest != 0xFF && !self.local.fresh(dest, now, LOCAL_MAX_AGE) {
            return None;
        }
        self.remote.touch(body[1], now);
        let mut frame = Frame::new();
        for &b in body.iter().chain(&[0, 0]) {
            if !frame.push(b) {
                return None;
            }
        }
        frame.fill_fcs();
        Some(frame)
    }

    /// The node bitmap the link should proxy for, when it has changed.
    pub fn proxy_update(&mut self, now: u32) -> Option<[u8; 32]> {
        let bits = self.remote.bitmap(now, REMOTE_MAX_AGE);
        (bits != self.proxied).then(|| {
            self.proxied = bits;
            bits
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc;
    use crate::frame::control_frame;

    #[test]
    fn group_mac() {
        assert_eq!(GROUP_MAC, [0x01, 0x00, 0x5E, 0x40, 0x4C, 0x54]);
    }

    #[test]
    fn localtalk_to_udp() {
        let mut b = Bridge::new(0xAABBCCDD);
        let mut out = [0u8; MAX_DATAGRAM];
        assert_eq!(b.from_localtalk(&control_frame(1, 2, LLAP_RTS), 0, &mut out), None);

        let mut f = vec![0xFF, 2, 0x01, 0x00, 0x03, 0x42];
        f.extend(crc::fcs(&f));
        let n = b.from_localtalk(&f, 0, &mut out).unwrap();
        assert_eq!(&out[..n], &[0xAA, 0xBB, 0xCC, 0xDD, 0xFF, 2, 0x01, 0x00, 0x03, 0x42]);
    }

    #[test]
    fn udp_to_localtalk_only_for_local_or_broadcast() {
        let mut b = Bridge::new(1);
        let dg = [0, 0, 0, 9, 7, 20, 0x01, 0x00, 0x02];
        // Node 7 has not been seen on LocalTalk yet.
        assert!(b.from_udp(&dg, 10).is_none());
        let mut out = [0u8; MAX_DATAGRAM];
        b.from_localtalk(&control_frame(0xFF, 7, 0x81), 10, &mut out);
        let f = b.from_udp(&dg, 20).unwrap();
        assert_eq!(&f.as_slice()[..5], &[7, 20, 0x01, 0x00, 0x02]);
        assert!(crc::check(f.as_slice()));
        // Node 20 now needs proxying.
        let bits = b.proxy_update(20).unwrap();
        assert_eq!(bits[20 / 8], 1 << (20 % 8));
        assert_eq!(b.proxy_update(21), None);
        // Our own datagrams are ignored.
        assert!(b.from_udp(&[0, 0, 0, 1, 0xFF, 20, 1, 0, 2], 20).is_none());
    }
}
