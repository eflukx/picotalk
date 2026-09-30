//! LToUDP transport: LLAP frames (without FCS) in UDP multicast datagrams
//! to 239.192.76.84:1954, each prefixed with a 4-byte sender ID. This is
//! what Mini vMac, Snow, AirTalk, TashRouter and the picotalk `ltoudp`
//! bridge speak.

use std::collections::VecDeque;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

pub use llap::ltoudp::{GROUP, HEADER_LEN, MAX_DATAGRAM, PORT};

pub struct LtoUdp {
    socket: UdpSocket,
    sender_id: [u8; 4],
    group: SocketAddrV4,
}

impl LtoUdp {
    /// Joins the LToUDP group on the interface with address `iface`
    /// (`0.0.0.0` lets the OS choose). Other programs on this host (an
    /// emulator, TashRouter) can share the port.
    pub fn open(iface: Ipv4Addr, sender_id: u32) -> io::Result<Self> {
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
        Ok(Self { socket, sender_id: sender_id.to_be_bytes(), group: SocketAddrV4::new(group_ip, PORT) })
    }

    pub fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut d = Vec::with_capacity(HEADER_LEN + frame.len());
        d.extend(self.sender_id);
        d.extend(frame);
        self.socket.send_to(&d, self.group).map(|_| ())
    }

    /// The next frame from someone else, or `None` after a short timeout.
    pub fn recv(&self) -> io::Result<Option<Vec<u8>>> {
        Ok(self.recv_raw()?.and_then(|(d, _)| {
            (d.len() > HEADER_LEN && d[..HEADER_LEN] != self.sender_id).then(|| d[HEADER_LEN..].to_vec())
        }))
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
}
