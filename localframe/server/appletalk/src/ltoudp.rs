//! LToUDP transport: LLAP frames (without FCS) in UDP multicast datagrams
//! to 239.192.76.84:1954, each prefixed with a 4-byte sender ID. This is
//! what Mini vMac, Snow, AirTalk, TashRouter and the picotalk `ltoudp`
//! bridge speak.
//!
//! With `unicast` on, data frames for a node we have heard from are sent
//! straight to that node's address instead of to the group. Peers bound to
//! `0.0.0.0:1954` (Snow, Mini vMac, lftest) receive them all the same, and
//! on Wi-Fi a unicast frame is acknowledged and retried by the radio where
//! a multicast one is sent once at the lowest rate.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use llap::frame::{BROADCAST, is_control};
use socket2::{Domain, Protocol, Socket, Type};

pub use llap::ltoudp::{GROUP, HEADER_LEN, MAX_DATAGRAM, PORT};

pub struct LtoUdp {
    socket: UdpSocket,
    sender_id: [u8; 4],
    group: SocketAddrV4,
    /// `None` sends everything to the group.
    peers: Option<Peers>,
}

/// Where each LLAP node's data frames come from, to send it unicast.
#[derive(Default)]
struct Peers {
    by_node: HashMap<u8, SocketAddr>,
    /// Addresses our own looped-back datagrams arrive from: a peer there
    /// shares this host's port, and Windows hands a unicast datagram to only
    /// one of the sockets bound to it.
    own_ips: HashSet<IpAddr>,
}

impl Peers {
    fn learn(&mut self, frame: &[u8], from: SocketAddr) {
        // Control frames are left out: an ENQ's source is only a candidate.
        if frame.len() >= 3 && !is_control(frame[2]) && frame[1] != BROADCAST {
            self.by_node.insert(frame[1], from);
        }
    }

    /// The address to send `frame` to, or `None` for the group: broadcasts,
    /// control frames (address acquisition must reach everyone), unknown
    /// nodes, and nodes that share an address with another node or with us.
    fn route(&self, frame: &[u8]) -> Option<SocketAddr> {
        if frame.len() < 3 || frame[0] == BROADCAST || is_control(frame[2]) {
            return None;
        }
        let addr = *self.by_node.get(&frame[0])?;
        let shared = self.by_node.iter().any(|(&n, &a)| a == addr && n != frame[0]);
        (!shared && !self.own_ips.contains(&addr.ip())).then_some(addr)
    }
}

impl LtoUdp {
    /// Joins the LToUDP group on the interface with address `iface`
    /// (`0.0.0.0` lets the OS choose). Other programs on this host (an
    /// emulator, TashRouter) can share the port. `unicast` sends data frames
    /// for known nodes straight to them (see the module docs).
    pub fn open(iface: Ipv4Addr, sender_id: u32, unicast: bool) -> io::Result<Self> {
        let group_ip = Ipv4Addr::from(GROUP);
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_reuse_address(true)?;
        #[cfg(unix)]
        s.set_reuse_port(true)?;
        s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT).into())?;
        s.join_multicast_v4(&group_ip, &iface)?;
        s.set_multicast_if_v4(&iface)?;
        // Emulators on this host must hear us.
        s.set_multicast_loop_v4(true)?;
        s.set_multicast_ttl_v4(1)?;
        let socket: UdpSocket = s.into();
        socket.set_read_timeout(Some(Duration::from_millis(2)))?;
        Ok(Self {
            socket,
            sender_id: sender_id.to_be_bytes(),
            group: SocketAddrV4::new(group_ip, PORT),
            peers: unicast.then(Peers::default),
        })
    }

    pub fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut d = Vec::with_capacity(HEADER_LEN + frame.len());
        d.extend(self.sender_id);
        d.extend(frame);
        let to = self.peers.as_ref().and_then(|p| p.route(frame)).unwrap_or(self.group.into());
        self.socket.send_to(&d, to).map(|_| ())
    }

    /// The next frame from someone else, or `None` after a short timeout.
    pub fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        let Some((d, from)) = self.recv_raw()? else { return Ok(None) };
        if d.len() <= HEADER_LEN {
            return Ok(None);
        }
        let own = d[..HEADER_LEN] == self.sender_id;
        if let Some(p) = &mut self.peers {
            if own {
                p.own_ips.insert(from.ip());
            } else {
                p.learn(&d[HEADER_LEN..], from);
            }
        }
        Ok((!own).then(|| d[HEADER_LEN..].to_vec()))
    }

    /// The next datagram as received, own ones included, with the sender's
    /// address; `None` after a short timeout. For monitoring.
    pub fn recv_raw(&self) -> io::Result<Option<(Vec<u8>, std::net::SocketAddr)>> {
        let mut buf = [0u8; MAX_DATAGRAM + 16];
        match self.socket.recv_from(&mut buf) {
            Ok((n, from)) => Ok(Some((buf[..n].to_vec(), from))),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Spaces outgoing frames so a burst (an 8-packet ATP response) does not
/// overrun a bridge's queue. LocalTalk carries at most 28.8 KB/s.
pub struct Pacer {
    /// Bytes per second; 0 sends immediately.
    rate: u32,
    next: Instant,
    queue: VecDeque<Vec<u8>>,
}

/// Per-frame cost in bytes on top of the frame: LLAP RTS/CTS, flags,
/// sync pulse and gaps, roughly.
const FRAME_OVERHEAD: u32 = 24;

impl Pacer {
    pub fn new(rate: u32, now: Instant) -> Self {
        Self { rate, next: now, queue: VecDeque::new() }
    }

    pub fn push(&mut self, frame: Vec<u8>) {
        self.queue.push_back(frame);
    }

    /// Frames that may be sent now.
    pub fn ready(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while !self.queue.is_empty() && (self.rate == 0 || now >= self.next) {
            let f = self.queue.pop_front().unwrap();
            if self.rate != 0 {
                let secs = (f.len() as u32 + FRAME_OVERHEAD) as f64 / self.rate as f64;
                self.next = self.next.max(now) + Duration::from_secs_f64(secs);
            }
            out.push(f);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_spaces_frames() {
        let t0 = Instant::now();
        let mut p = Pacer::new(1000, t0);
        p.push(vec![0; 76]);
        p.push(vec![0; 76]);
        assert_eq!(p.ready(t0).len(), 1);
        assert!(p.ready(t0 + Duration::from_millis(50)).is_empty());
        assert_eq!(p.ready(t0 + Duration::from_millis(100)).len(), 1);

        let mut unpaced = Pacer::new(0, t0);
        unpaced.push(vec![1]);
        unpaced.push(vec![2]);
        assert_eq!(unpaced.ready(t0).len(), 2);
    }

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn peers_route_data_frames_to_known_nodes() {
        let mut p = Peers::default();
        let mac = addr("192.168.1.26:1954");
        p.learn(&[0xFF, 7, 2, 0, 5], mac); // a DDP broadcast from node 7
        assert_eq!(p.route(&[7, 200, 1, 0, 5]), Some(mac));
        assert_eq!(p.route(&[8, 200, 1, 0, 5]), None, "unknown node");
        assert_eq!(p.route(&[0xFF, 200, 1, 0, 5]), None, "broadcast");
        assert_eq!(p.route(&[7, 7, 0x81]), None, "control frame");
    }

    #[test]
    fn peers_ignore_enq_sources() {
        let mut p = Peers::default();
        p.learn(&[9, 9, 0x81], addr("10.0.0.2:1954"));
        assert_eq!(p.route(&[9, 200, 1, 0, 5]), None);
    }

    #[test]
    fn peers_skip_shared_and_own_addresses() {
        let mut p = Peers::default();
        let host = addr("192.168.1.26:1954");
        p.learn(&[0xFF, 7, 2, 0, 5], host);
        p.learn(&[0xFF, 8, 2, 0, 5], host);
        assert_eq!(p.route(&[7, 200, 1, 0, 5]), None, "two nodes behind one socket");

        let mut p = Peers::default();
        p.learn(&[0xFF, 7, 2, 0, 5], host);
        p.own_ips.insert(host.ip());
        assert_eq!(p.route(&[7, 200, 1, 0, 5]), None, "a peer on our own host");
    }

    #[test]
    fn peers_follow_a_node_that_moves() {
        let mut p = Peers::default();
        p.learn(&[0xFF, 7, 2, 0, 5], addr("10.0.0.2:1954"));
        p.learn(&[0xFF, 7, 2, 0, 5], addr("10.0.0.3:1954"));
        assert_eq!(p.route(&[7, 200, 1, 0, 5]), Some(addr("10.0.0.3:1954")));
    }
}
