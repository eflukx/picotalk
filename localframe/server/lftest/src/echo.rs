//! The `LFEcho` test service: proves the whole chain (NBP lookup, ATP
//! requests and multi-packet responses) before the remote desktop exists.
//!
//! Requests are ATP data, big endian:
//!
//! ```text
//! PING: 01 00 seq(2) payload…
//!   → one packet: 01 00 seq(2) your_net(2) your_node your_socket
//!                 server_node 00 count(2) payload…   (payload echoed,
//!                 truncated to fit 578 bytes); user bytes echoed
//! BULK: 02 00 seq(2)
//!   → as many 578-byte packets as the request's bitmap allows. Packet i:
//!     02 i seq(2), then byte k (k = 4..577) = (seq + 17·i + k) mod 256;
//!     user bytes = i
//! ```
//!
//! Anything else gets one packet `cmd FF`.

use appletalk::atp::{self, Request, ResponsePacket};

pub const TYPE: &str = "LFEcho";

pub const PING: u8 = 1;
pub const BULK: u8 = 2;

const PING_HEADER: usize = 12;

#[derive(Default)]
pub struct Echo {
    count: u16,
}

impl Echo {
    pub fn handle(&mut self, req: &Request, server_node: u8) -> Vec<ResponsePacket> {
        let d = &req.data;
        let seq = [d.get(2).copied().unwrap_or(0), d.get(3).copied().unwrap_or(0)];
        match d.first() {
            Some(&PING) => {
                self.count = self.count.wrapping_add(1);
                let mut p = vec![PING, 0, seq[0], seq[1]];
                p.extend(req.from.net.to_be_bytes());
                p.extend([req.from.node, req.from.socket, server_node, 0]);
                p.extend(self.count.to_be_bytes());
                let payload = d.get(4..).unwrap_or(&[]);
                p.extend(&payload[..payload.len().min(atp::MAX_DATA - PING_HEADER)]);
                vec![ResponsePacket { user: req.user, data: p }]
            }
            Some(&BULK) => (0..req.max_packets())
                .map(|i| ResponsePacket { user: i as u32, data: bulk_packet(u16::from_be_bytes(seq), i as u8) })
                .collect(),
            other => vec![ResponsePacket { user: 0, data: vec![other.copied().unwrap_or(0), 0xFF] }],
        }
    }
}

pub fn bulk_packet(seq: u16, index: u8) -> Vec<u8> {
    let s = seq.to_be_bytes();
    let mut p = vec![BULK, index, s[0], s[1]];
    p.extend((4..atp::MAX_DATA).map(|k| (seq as usize + 17 * index as usize + k) as u8));
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use appletalk::Addr;

    fn req(bitmap: u8, data: &[u8]) -> Request {
        Request {
            from: Addr { net: 0, node: 12, socket: 0xFC },
            tid: 1,
            xo: true,
            bitmap,
            user: 0xCAFEBABE,
            data: data.to_vec(),
        }
    }

    #[test]
    fn ping() {
        let mut e = Echo::default();
        let r = e.handle(&req(1, &[PING, 0, 0, 5, b'h', b'i']), 200);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].user, 0xCAFEBABE);
        assert_eq!(r[0].data, [PING, 0, 0, 5, 0, 0, 12, 0xFC, 200, 0, 0, 1, b'h', b'i']);
        let big = e.handle(&req(1, &[PING, 0, 0, 6].iter().copied().chain([7; 574]).collect::<Vec<_>>()), 200);
        assert_eq!(big[0].data.len(), atp::MAX_DATA);
    }

    #[test]
    fn bulk() {
        let r = Echo::default().handle(&req(0x0F, &[BULK, 0, 1, 2]), 200);
        assert_eq!(r.len(), 4);
        assert!(r.iter().all(|p| p.data.len() == atp::MAX_DATA));
        assert_eq!(&r[3].data[..4], &[BULK, 3, 1, 2]);
        assert_eq!(r[3].data[4], (0x0102 + 17 * 3 + 4) as u8);
    }
}
