//! AppleTalk Transaction Protocol, responder side.
//!
//! ```text
//! header: control bitmap/seq TID(2) user_bytes(4), then up to 578 data bytes
//! control: bits 7-6 function (01 TReq, 10 TResp, 11 TRel), bit 5 XO,
//!          bit 4 EOM, bit 3 STS, bits 2-0 TRel timeout (XO requests)
//! ```
//!
//! A request carries a bitmap of the response packets (up to 8) the
//! requester still wants. For exactly-once (XO) transactions the responder
//! keeps its response until the requester sends TRel, so a retried request
//! gets the same answer instead of being executed twice.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::ddp::Addr;

pub const HEADER: usize = 8;
pub const MAX_DATA: usize = 578;
pub const MAX_PACKETS: usize = 8;

const TREQ: u8 = 0x40;
const TRESP: u8 = 0x80;
const TREL: u8 = 0xC0;
const FUNCTION: u8 = 0xC0;
const XO: u8 = 0x20;
const EOM: u8 = 0x10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub control: u8,
    pub bitmap: u8,
    pub tid: u16,
    pub user: u32,
    pub data: Vec<u8>,
}

impl Packet {
    pub fn parse(b: &[u8]) -> Option<Self> {
        let h = b.get(..HEADER)?;
        Some(Self {
            control: h[0],
            bitmap: h[1],
            tid: u16::from_be_bytes([h[2], h[3]]),
            user: u32::from_be_bytes([h[4], h[5], h[6], h[7]]),
            data: b[HEADER..].to_vec(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = vec![self.control, self.bitmap];
        b.extend(self.tid.to_be_bytes());
        b.extend(self.user.to_be_bytes());
        b.extend(&self.data);
        b
    }
}

/// A transaction request handed to the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub from: Addr,
    pub tid: u16,
    pub xo: bool,
    /// Response packets wanted: bit n = packet n.
    pub bitmap: u8,
    pub user: u32,
    pub data: Vec<u8>,
}

impl Request {
    /// How many response packets the requester has buffers for.
    pub fn max_packets(&self) -> usize {
        (self.bitmap.trailing_ones() as usize).min(MAX_PACKETS)
    }
}

/// One response packet: user bytes and up to [`MAX_DATA`] bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponsePacket {
    pub user: u32,
    pub data: Vec<u8>,
}

struct Cached {
    packets: Vec<ResponsePacket>,
    expires: Instant,
}

/// Duplicate filtering and XO response cache for one responding socket.
#[derive(Default)]
pub struct Responder {
    /// Requests handed to the application and not yet answered.
    in_progress: HashMap<(Addr, u16), Instant>,
    cache: HashMap<(Addr, u16), Cached>,
}

/// What to do with an incoming ATP packet.
#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    /// A new request for the application.
    Request(Request),
    /// A retried XO request we already answered: send these packets again.
    Resend(Vec<Packet>),
    Ignore,
}

impl Responder {
    pub fn receive(&mut self, from: Addr, bytes: &[u8], now: Instant) -> Incoming {
        let Some(p) = Packet::parse(bytes) else { return Incoming::Ignore };
        let key = (from, p.tid);
        match p.control & FUNCTION {
            TREL => {
                self.cache.remove(&key);
                Incoming::Ignore
            }
            TREQ => {
                if let Some(c) = self.cache.get(&key) {
                    return Incoming::Resend(packets(p.tid, &c.packets, p.bitmap));
                }
                if self.in_progress.contains_key(&key) {
                    return Incoming::Ignore;
                }
                let xo = p.control & XO != 0;
                if xo {
                    self.in_progress.insert(key, now + trel_timeout(p.control));
                }
                Incoming::Request(Request { from, tid: p.tid, xo, bitmap: p.bitmap, user: p.user, data: p.data })
            }
            _ => Incoming::Ignore,
        }
    }

    /// Answers `req`. Returns the ATP packets to send. At most
    /// `req.max_packets()` response packets are used; the rest are dropped.
    pub fn respond(&mut self, req: &Request, mut response: Vec<ResponsePacket>, now: Instant) -> Vec<Packet> {
        response.truncate(req.max_packets().max(1));
        if response.is_empty() {
            response.push(ResponsePacket { user: 0, data: Vec::new() });
        }
        let key = (req.from, req.tid);
        let out = packets(req.tid, &response, req.bitmap);
        if req.xo {
            let expires = self.in_progress.remove(&key).unwrap_or(now + Duration::from_secs(30));
            self.cache.insert(key, Cached { packets: response, expires });
        }
        out
    }

    pub fn expire(&mut self, now: Instant) {
        self.cache.retain(|_, c| c.expires > now);
        self.in_progress.retain(|_, &mut t| t > now);
    }

    pub fn cached(&self) -> usize {
        self.cache.len()
    }
}

/// The TResp packets selected by `bitmap`; EOM marks the last packet of the
/// whole response.
fn packets(tid: u16, response: &[ResponsePacket], bitmap: u8) -> Vec<Packet> {
    let last = response.len() - 1;
    response
        .iter()
        .enumerate()
        .filter(|&(i, _)| bitmap & (1 << i) != 0)
        .map(|(i, r)| Packet {
            control: TRESP | if i == last { EOM } else { 0 },
            bitmap: i as u8,
            tid,
            user: r.user,
            data: r.data.clone(),
        })
        .collect()
}

/// How long an XO response is kept, from the TRel timer indicator.
fn trel_timeout(control: u8) -> Duration {
    Duration::from_secs(30 << (control & 0x07).min(4))
}

/// Builds a TReq (used by tests and tools that act as requester).
pub fn request(tid: u16, xo: bool, bitmap: u8, user: u32, data: &[u8]) -> Vec<u8> {
    Packet { control: TREQ | if xo { XO } else { 0 }, bitmap, tid, user, data: data.to_vec() }.to_bytes()
}

pub fn release(tid: u16) -> Vec<u8> {
    Packet { control: TREL, bitmap: 0, tid, user: 0, data: vec![] }.to_bytes()
}

pub fn is_eom(p: &Packet) -> bool {
    p.control & EOM != 0
}

pub fn is_response(p: &Packet) -> bool {
    p.control & FUNCTION == TRESP
}

/// A requester's view of one transaction: which response packets are
/// still missing, and the ones that arrived.
#[derive(Clone, Debug)]
pub struct Collector {
    /// Packets still wanted, as a TReq bitmap.
    pub want: u8,
    got: Vec<Option<ResponsePacket>>,
}

impl Collector {
    pub fn new(packets: usize) -> Self {
        let n = packets.clamp(1, MAX_PACKETS);
        Self { want: ((1u16 << n) - 1) as u8, got: vec![None; n] }
    }

    /// Takes a TResp. Returns the whole response once complete.
    pub fn add(&mut self, p: &Packet) -> Option<Vec<ResponsePacket>> {
        let seq = p.bitmap as usize;
        if seq >= self.got.len() {
            return None;
        }
        if is_eom(p) {
            // Nothing after this packet will come.
            self.got.truncate(seq + 1);
            self.want &= ((2u16 << seq) - 1) as u8;
        }
        self.want &= !(1 << seq);
        self.got[seq] = Some(ResponsePacket { user: p.user, data: p.data.clone() });
        (self.want == 0).then(|| self.got.iter().cloned().collect::<Option<Vec<_>>>()).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: Addr = Addr { net: 0, node: 12, socket: 0xFC };

    fn resp(n: usize) -> Vec<ResponsePacket> {
        (0..n).map(|i| ResponsePacket { user: i as u32, data: vec![i as u8; 4] }).collect()
    }

    #[test]
    fn xo_request_is_answered_once_and_resent() {
        let now = Instant::now();
        let mut r = Responder::default();
        let Incoming::Request(req) = r.receive(MAC, &request(5, true, 0xFF, 0, b"hi"), now) else { panic!() };
        assert_eq!(req.max_packets(), 8);
        // A retry while the application is still working is dropped.
        assert_eq!(r.receive(MAC, &request(5, true, 0xFF, 0, b"hi"), now), Incoming::Ignore);

        let out = r.respond(&req, resp(3), now);
        assert_eq!(out.len(), 3);
        assert_eq!(out.iter().map(|p| p.bitmap).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(!is_eom(&out[1]) && is_eom(&out[2]));

        // Packet 1 was lost: the retry asks only for it.
        let Incoming::Resend(again) = r.receive(MAC, &request(5, true, 0b010, 0, b"hi"), now) else { panic!() };
        assert_eq!(again, vec![out[1].clone()]);

        r.receive(MAC, &release(5), now);
        assert_eq!(r.cached(), 0);
    }

    #[test]
    fn response_limited_by_bitmap() {
        let now = Instant::now();
        let mut r = Responder::default();
        let Incoming::Request(req) = r.receive(MAC, &request(1, false, 0b11, 0, b""), now) else { panic!() };
        let out = r.respond(&req, resp(5), now);
        assert_eq!(out.len(), 2);
        assert!(is_eom(&out[1]));
        // ALO: nothing is kept.
        assert_eq!(r.cached(), 0);
        assert!(matches!(r.receive(MAC, &request(1, false, 0b11, 0, b""), now), Incoming::Request(_)));
    }

    #[test]
    fn collector_handles_eom_and_reordering() {
        let pkt = |seq: u8, eom: bool| Packet {
            control: TRESP | if eom { EOM } else { 0 },
            bitmap: seq,
            tid: 1,
            user: seq as u32,
            data: vec![seq],
        };
        let mut c = Collector::new(8);
        assert_eq!(c.want, 0xFF);
        assert_eq!(c.add(&pkt(2, true)), None);
        assert_eq!(c.want, 0b011);
        assert_eq!(c.add(&pkt(0, false)), None);
        let all = c.add(&pkt(1, false)).unwrap();
        assert_eq!(all.iter().map(|p| p.data[0]).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn cache_expires() {
        let now = Instant::now();
        let mut r = Responder::default();
        let Incoming::Request(req) = r.receive(MAC, &request(2, true, 1, 0, b""), now) else { panic!() };
        r.respond(&req, resp(1), now);
        r.expire(now + Duration::from_secs(29));
        assert_eq!(r.cached(), 1);
        r.expire(now + Duration::from_secs(31));
        assert_eq!(r.cached(), 0);
    }
}
