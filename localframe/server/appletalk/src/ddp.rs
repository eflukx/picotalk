//! LLAP frames and the DDP datagrams inside them.
//!
//! ```text
//! LLAP:        dest src type payload…                (no FCS on LToUDP)
//! short DDP:   type 1: len(10 bits) dst_skt src_skt ddp_type data…
//! long DDP:    type 2: hops/len cksum dst_net src_net dst_node src_node
//!                      dst_skt src_skt ddp_type data…
//! ```
//!
//! A short header is only valid between nodes on the same LocalTalk
//! network; nodes on a network without a router use it for everything but
//! broadcasts to other networks.

use llap::frame::{BROADCAST, LLAP_ACK, LLAP_ENQ};

pub const LLAP_DDP_SHORT: u8 = 1;
pub const LLAP_DDP_LONG: u8 = 2;

pub const SHORT_HEADER: usize = 5;
pub const LONG_HEADER: usize = 13;
/// Largest DDP data field.
pub const MAX_DATA: usize = 586;

pub const TYPE_NBP: u8 = 2;
pub const TYPE_ATP: u8 = 3;

/// The NBP names information socket, present on every node.
pub const SOCKET_NIS: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Addr {
    pub net: u16,
    pub node: u8,
    pub socket: u8,
}

impl core::fmt::Display for Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}:{}", self.net, self.node, self.socket)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub src: Addr,
    pub dst: Addr,
    pub ddp_type: u8,
    pub data: Vec<u8>,
    /// Arrived with (or should be sent with) a long header.
    pub long: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Llap {
    Enq { node: u8 },
    Ack { node: u8 },
    /// RTS, CTS and anything else we do not act on.
    OtherControl,
    Ddp(Datagram),
}

/// Parses an LLAP frame without FCS. Returns `None` for malformed frames.
pub fn parse(frame: &[u8]) -> Option<Llap> {
    let (&dst, &src, &llap_type) = (frame.first()?, frame.get(1)?, frame.get(2)?);
    let body = &frame[3..];
    match llap_type {
        LLAP_ENQ => Some(Llap::Enq { node: dst }),
        LLAP_ACK => Some(Llap::Ack { node: src }),
        t if t & 0x80 != 0 => Some(Llap::OtherControl),
        LLAP_DDP_SHORT => {
            let len = ddp_len(body)?;
            if len < SHORT_HEADER {
                return None;
            }
            Some(Llap::Ddp(Datagram {
                src: Addr { net: 0, node: src, socket: body[3] },
                dst: Addr { net: 0, node: dst, socket: body[2] },
                ddp_type: body[4],
                data: body[SHORT_HEADER..len].to_vec(),
                long: false,
            }))
        }
        LLAP_DDP_LONG => {
            let len = ddp_len(body)?;
            if len < LONG_HEADER {
                return None;
            }
            let be16 = |i: usize| u16::from_be_bytes([body[i], body[i + 1]]);
            Some(Llap::Ddp(Datagram {
                src: Addr { net: be16(6), node: body[9], socket: body[11] },
                dst: Addr { net: be16(4), node: body[8], socket: body[10] },
                ddp_type: body[12],
                data: body[LONG_HEADER..len].to_vec(),
                long: true,
            }))
        }
        _ => None,
    }
}

/// The 10-bit DDP length, checked against the bytes present.
fn ddp_len(body: &[u8]) -> Option<usize> {
    let len = ((*body.first()? as usize & 0x03) << 8) | *body.get(1)? as usize;
    (len <= body.len()).then_some(len)
}

pub fn enq(node: u8) -> Vec<u8> {
    vec![node, node, LLAP_ENQ]
}

pub fn ack(node: u8) -> Vec<u8> {
    vec![node, node, LLAP_ACK]
}

/// Builds the LLAP frame (without FCS) for a datagram from `src_node`.
/// The LLAP destination is the datagram's destination node, so the peer
/// must be on this LocalTalk network (no routing).
pub fn build(d: &Datagram) -> Vec<u8> {
    assert!(d.data.len() <= MAX_DATA);
    let mut f = Vec::with_capacity(3 + LONG_HEADER + d.data.len());
    f.extend([d.dst.node, d.src.node]);
    if d.long {
        let len = (LONG_HEADER + d.data.len()) as u16;
        f.push(LLAP_DDP_LONG);
        f.extend(len.to_be_bytes());
        f.extend([0, 0]); // no checksum
        f.extend(d.dst.net.to_be_bytes());
        f.extend(d.src.net.to_be_bytes());
        f.extend([d.dst.node, d.src.node, d.dst.socket, d.src.socket, d.ddp_type]);
    } else {
        let len = (SHORT_HEADER + d.data.len()) as u16;
        f.push(LLAP_DDP_SHORT);
        f.extend(len.to_be_bytes());
        f.extend([d.dst.socket, d.src.socket, d.ddp_type]);
    }
    f.extend(&d.data);
    f
}

/// True if a datagram with this destination is for `node`.
pub fn is_for(dst: &Addr, node: u8) -> bool {
    dst.node == node || dst.node == BROADCAST
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datagram(long: bool) -> Datagram {
        Datagram {
            src: Addr { net: if long { 7 } else { 0 }, node: 200, socket: 250 },
            dst: Addr { net: if long { 7 } else { 0 }, node: 12, socket: 0xFC },
            ddp_type: TYPE_ATP,
            data: vec![1, 2, 3],
            long,
        }
    }

    #[test]
    fn short_roundtrip() {
        let d = datagram(false);
        let f = build(&d);
        assert_eq!(f, [12, 200, 1, 0, 8, 0xFC, 250, 3, 1, 2, 3]);
        assert_eq!(parse(&f), Some(Llap::Ddp(d)));
    }

    #[test]
    fn long_roundtrip() {
        let d = datagram(true);
        let f = build(&d);
        assert_eq!(f.len(), 3 + LONG_HEADER + 3);
        assert_eq!(parse(&f), Some(Llap::Ddp(d)));
    }

    #[test]
    fn rejects_truncated() {
        assert_eq!(parse(&[12, 200, 1, 0, 20, 0xFC, 250, 3]), None);
        assert_eq!(parse(&[12, 200]), None);
    }

    #[test]
    fn control() {
        assert_eq!(parse(&enq(130)), Some(Llap::Enq { node: 130 }));
        assert_eq!(parse(&ack(130)), Some(Llap::Ack { node: 130 }));
        assert_eq!(parse(&[1, 2, 0x84]), Some(Llap::OtherControl));
    }
}
