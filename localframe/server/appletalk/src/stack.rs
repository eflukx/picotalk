//! A minimal AppleTalk node: LLAP address acquisition, NBP (answering
//! lookups for registered names, and looking names up), ATP responders on
//! registered sockets, and an ATP requester.
//!
//! The stack is transport free. Feed it LLAP frames (without FCS) with
//! [`Stack::receive`], call [`Stack::poll`] regularly, and send whatever
//! [`Stack::pop_frame`] returns. Over LToUDP there is no RTS/CTS and no
//! FCS; a picotalk bridge adds both on the LocalTalk side.

use std::collections::{HashMap, VecDeque};
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use crate::atp::{self, Collector, Incoming, Request, ResponsePacket};
use crate::ddp::{self, Addr, Datagram, Llap};
use crate::nbp::{self, Entity};

/// ENQs sent for a candidate address before it is taken.
const ENQ_COUNT: u32 = 4;
const ENQ_INTERVAL: Duration = Duration::from_millis(50);

/// Socket our requests are sent from and answered to.
pub const REQUESTER_SOCKET: u8 = 254;
/// Retries of an unanswered request, and the interval between them.
const REQUEST_TRIES: u32 = 4;
const REQUEST_RETRY: Duration = Duration::from_secs(2);

/// Which LLAP node range to pick an address from (Inside AppleTalk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// 1–127
    Workstation,
    /// 128–254
    Server,
}

impl Role {
    fn nodes(self) -> RangeInclusive<u8> {
        match self {
            Role::Workstation => 1..=127,
            Role::Server => 128..=254,
        }
    }

    /// A node ID in this role's range derived from `name`, to try first.
    /// A server that restarts under the same name then usually gets the
    /// same address back, so clients holding it keep working (Macs keep
    /// their last node ID in PRAM for the same reason).
    pub fn node_for(self, name: &[u8]) -> u8 {
        // FNV-1a
        let h = name.iter().fold(0x811C_9DC5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193));
        let range = self.nodes();
        range.start() + (h % (*range.end() as u32 - *range.start() as u32 + 1)) as u8
    }
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Acquiring { candidate: u8, sent: u32, next: Instant },
    Ready,
}

/// Something the application should know about.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// The node address is ours to use.
    Ready(u8),
    /// A new ATP request arrived on one of our sockets. Answer it with
    /// [`Stack::respond`], now or later.
    Request { socket: u8, req: Request },
    /// We answered an NBP lookup.
    LookedUp { from: Addr, pattern: Entity },
    /// A reply to our [`Stack::lookup`].
    Found { id: u8, addr: Addr, entity: Entity },
    /// Our request `tid` was answered.
    Response { tid: u16, packets: Vec<ResponsePacket> },
    /// Our request `tid` got no (complete) answer.
    RequestFailed { tid: u16 },
}

struct Transaction {
    to: Addr,
    data: Vec<u8>,
    user: u32,
    collector: Collector,
    tries: u32,
    next: Instant,
}

pub struct Stack {
    role: Role,
    node: u8,
    state: State,
    rng: u32,
    names: Vec<(Entity, u8)>,
    responders: HashMap<u8, atp::Responder>,
    transactions: HashMap<u16, Transaction>,
    next_tid: u16,
    next_nbp_id: u8,
    /// Whether each peer last talked to us with a long DDP header, so we
    /// talk to it the same way.
    long_peers: HashMap<u8, bool>,
    out: VecDeque<Vec<u8>>,
}

impl Stack {
    /// `node` is the address to try first; otherwise one is picked at
    /// random from the role's range.
    pub fn new(role: Role, node: Option<u8>, seed: u32, now: Instant) -> Self {
        let mut s = Self {
            role,
            node: 0,
            state: State::Ready,
            rng: seed | 1,
            names: Vec::new(),
            responders: HashMap::new(),
            transactions: HashMap::new(),
            next_tid: seed as u16,
            next_nbp_id: (seed >> 16) as u8,
            long_peers: HashMap::new(),
            out: VecDeque::new(),
        };
        let candidate = node.unwrap_or_else(|| s.random_node(None));
        s.state = State::Acquiring { candidate, sent: 0, next: now };
        s
    }

    pub fn node(&self) -> Option<u8> {
        (self.state == State::Ready).then_some(self.node)
    }

    /// Opens an ATP responding socket and registers `entity` on it.
    pub fn register(&mut self, entity: Entity, socket: u8) {
        self.responders.entry(socket).or_default();
        self.names.push((entity, socket));
    }

    pub fn pop_frame(&mut self) -> Option<Vec<u8>> {
        self.out.pop_front()
    }

    /// Timers: address acquisition, request retries, ATP cache expiry.
    pub fn poll(&mut self, now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        for r in self.responders.values_mut() {
            r.expire(now);
        }
        if let State::Acquiring { candidate, sent, next } = self.state
            && now >= next {
                if sent == ENQ_COUNT {
                    self.node = candidate;
                    self.state = State::Ready;
                    events.push(Event::Ready(candidate));
                } else {
                    self.out.push_back(ddp::enq(candidate));
                    self.state = State::Acquiring { candidate, sent: sent + 1, next: now + ENQ_INTERVAL };
                }
            }
        let due: Vec<u16> = self.transactions.iter().filter(|(_, t)| now >= t.next).map(|(&tid, _)| tid).collect();
        for tid in due {
            let t = self.transactions.get_mut(&tid).unwrap();
            if t.tries == 0 {
                self.transactions.remove(&tid);
                events.push(Event::RequestFailed { tid });
            } else {
                t.tries -= 1;
                t.next = now + REQUEST_RETRY;
                let treq = atp::request(tid, true, t.collector.want, t.user, &t.data);
                let to = t.to;
                self.send(REQUESTER_SOCKET, to, ddp::TYPE_ATP, treq);
            }
        }
        events
    }

    pub fn receive(&mut self, frame: &[u8], now: Instant) -> Option<Event> {
        let llap = ddp::parse(frame)?;
        if let State::Acquiring { candidate, .. } = self.state {
            // Someone else has (or wants) our candidate: start over.
            let taken = match &llap {
                Llap::Ack { node } | Llap::Enq { node } => *node == candidate,
                Llap::Ddp(d) => d.src.node == candidate,
                Llap::OtherControl => false,
            };
            if taken {
                let candidate = self.random_node(Some(candidate));
                self.state = State::Acquiring { candidate, sent: 0, next: now };
            }
            return None;
        }
        match llap {
            Llap::Enq { node } if node == self.node => {
                self.out.push_back(ddp::ack(self.node));
                None
            }
            Llap::Ddp(d) if ddp::is_for(&d.dst, self.node) => {
                self.long_peers.insert(d.src.node, d.long);
                match d.ddp_type {
                    ddp::TYPE_NBP if d.dst.socket == ddp::SOCKET_NIS => self.nbp(&d),
                    ddp::TYPE_ATP if d.dst.node == self.node => self.atp(&d, now),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn nbp(&mut self, d: &Datagram) -> Option<Event> {
        if let Some((id, tuples)) = nbp::parse_reply(&d.data) {
            // One event per call; LkUp-Replies from servers carry one tuple.
            let (addr, entity) = tuples.into_iter().next()?;
            return Some(Event::Found { id, addr, entity });
        }
        let lookup = nbp::parse_lookup(&d.data)?;
        let mut answered = false;
        for (entity, socket) in &self.names {
            if !nbp::matches(&lookup.pattern, entity) {
                continue;
            }
            let src = Addr { net: reply_net(d), node: self.node, socket: *socket };
            let dgram = Datagram {
                src: Addr { socket: ddp::SOCKET_NIS, ..src },
                dst: lookup.reply_to,
                ddp_type: ddp::TYPE_NBP,
                data: nbp::reply(lookup.id, src, entity),
                long: d.long,
            };
            self.out.push_back(ddp::build(&dgram));
            answered = true;
        }
        answered.then_some(Event::LookedUp { from: lookup.reply_to, pattern: lookup.pattern })
    }

    fn atp(&mut self, d: &Datagram, now: Instant) -> Option<Event> {
        if d.dst.socket == REQUESTER_SOCKET {
            let p = atp::Packet::parse(&d.data).filter(atp::is_response)?;
            let t = self.transactions.get_mut(&p.tid).filter(|t| t.to.node == d.src.node)?;
            let packets = t.collector.add(&p)?;
            let to = t.to;
            self.transactions.remove(&p.tid);
            self.send(REQUESTER_SOCKET, to, ddp::TYPE_ATP, atp::release(p.tid));
            return Some(Event::Response { tid: p.tid, packets });
        }
        let responder = self.responders.get_mut(&d.dst.socket)?;
        match responder.receive(d.src, &d.data, now) {
            Incoming::Request(req) => Some(Event::Request { socket: d.dst.socket, req }),
            Incoming::Resend(packets) => {
                for p in packets {
                    self.send(d.dst.socket, d.src, ddp::TYPE_ATP, p.to_bytes());
                }
                None
            }
            Incoming::Ignore => None,
        }
    }

    /// Answers a request previously returned as [`Event::Request`].
    pub fn respond(&mut self, socket: u8, req: &Request, response: Vec<ResponsePacket>, now: Instant) {
        let Some(responder) = self.responders.get_mut(&socket) else { return };
        for p in responder.respond(req, response, now) {
            self.send(socket, req.from, ddp::TYPE_ATP, p.to_bytes());
        }
    }

    /// Sends an exactly-once request for up to `packets` response packets.
    /// The answer arrives as [`Event::Response`] or [`Event::RequestFailed`]
    /// with the returned transaction ID. `None` until the node is ready.
    pub fn request(&mut self, to: Addr, data: &[u8], user: u32, packets: usize, now: Instant) -> Option<u16> {
        self.node()?;
        self.next_tid = self.next_tid.wrapping_add(1);
        let tid = self.next_tid;
        let t = Transaction {
            to,
            data: data.to_vec(),
            user,
            collector: Collector::new(packets),
            tries: REQUEST_TRIES + 1,
            next: now,
        };
        self.transactions.insert(tid, t);
        // Sent by the next poll().
        Some(tid)
    }

    /// Broadcasts an NBP lookup for `object:kind@*`. Replies arrive as
    /// [`Event::Found`] with the returned ID. Call again to retry.
    pub fn lookup(&mut self, object: &str, kind: &str) -> Option<u8> {
        let node = self.node()?;
        self.next_nbp_id = self.next_nbp_id.wrapping_add(1);
        let me = Addr { net: 0, node, socket: ddp::SOCKET_NIS };
        let data = nbp::lookup(self.next_nbp_id, me, &Entity::new(object, kind));
        let broadcast = Addr { net: 0, node: llap::frame::BROADCAST, socket: ddp::SOCKET_NIS };
        self.send(ddp::SOCKET_NIS, broadcast, ddp::TYPE_NBP, data);
        Some(self.next_nbp_id)
    }

    fn send(&mut self, socket: u8, to: Addr, ddp_type: u8, data: Vec<u8>) {
        let long = self.long_peers.get(&to.node).copied().unwrap_or(false);
        let dgram = Datagram { src: Addr { net: to.net, node: self.node, socket }, dst: to, ddp_type, data, long };
        self.out.push_back(ddp::build(&dgram));
    }

    fn random_node(&mut self, avoid: Option<u8>) -> u8 {
        let range = self.role.nodes();
        let span = (range.end() - range.start()) as u32 + 1;
        loop {
            // xorshift32
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 17;
            self.rng ^= self.rng << 5;
            let n = range.start() + (self.rng % span) as u8;
            if Some(n) != avoid {
                return n;
            }
        }
    }
}

/// Our network number as seen by the sender: without a router every node
/// on the cable shares it, and a long header tells us what it is.
fn reply_net(d: &Datagram) -> u16 {
    if d.dst.net != 0 { d.dst.net } else { d.src.net }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(role: Role, node: u8, now: Instant) -> Stack {
        let mut s = Stack::new(role, Some(node), 1, now);
        let mut t = now;
        while !s.poll(t).contains(&Event::Ready(node)) {
            t += ENQ_INTERVAL;
        }
        let enqs: Vec<_> = std::iter::from_fn(|| s.pop_frame()).collect();
        assert_eq!(enqs, vec![ddp::enq(node); ENQ_COUNT as usize]);
        s
    }

    /// Delivers every frame `from` has queued to `to`, returning `to`'s events.
    fn deliver(from: &mut Stack, to: &mut Stack, now: Instant) -> Vec<Event> {
        std::iter::from_fn(|| from.pop_frame()).filter_map(|f| to.receive(&f, now)).collect()
    }

    #[test]
    fn acquisition_restarts_on_conflict() {
        let now = Instant::now();
        let mut s = Stack::new(Role::Server, Some(200), 7, now);
        s.poll(now);
        assert_eq!(s.receive(&ddp::ack(200), now), None);
        let State::Acquiring { candidate, sent: 0, .. } = s.state else { panic!() };
        assert_ne!(candidate, 200);
        assert!(Role::Server.nodes().contains(&candidate));
    }

    #[test]
    fn node_for_is_stable_and_in_range() {
        assert_eq!(Role::Server.node_for(b"crunchy"), Role::Server.node_for(b"crunchy"));
        for name in [&b""[..], b"a", b"crunchy", b"a much longer server name"] {
            assert!(Role::Server.nodes().contains(&Role::Server.node_for(name)));
            assert!(Role::Workstation.nodes().contains(&Role::Workstation.node_for(name)));
        }
    }

    #[test]
    fn answers_enq() {
        let now = Instant::now();
        let mut s = ready(Role::Server, 200, now);
        s.receive(&ddp::enq(200), now);
        assert_eq!(s.pop_frame(), Some(ddp::ack(200)));
        s.receive(&ddp::enq(201), now);
        assert_eq!(s.pop_frame(), None);
    }

    #[test]
    fn lookup_and_transaction_between_two_stacks() {
        let now = Instant::now();
        let mut server = ready(Role::Server, 200, now);
        server.register(Entity::new("pc", "LFEcho"), 250);
        let mut client = ready(Role::Workstation, 12, now);

        let id = client.lookup("=", "lfecho").unwrap();
        let ev = deliver(&mut client, &mut server, now);
        assert!(matches!(ev[..], [Event::LookedUp { .. }]));
        let ev = deliver(&mut server, &mut client, now);
        let [Event::Found { id: got, addr, ref entity }] = ev[..] else { panic!("{ev:?}") };
        assert_eq!((got, addr, entity), (id, Addr { net: 0, node: 200, socket: 250 }, &Entity::new("pc", "LFEcho")));

        let tid = client.request(addr, b"ping", 7, 2, now).unwrap();
        client.poll(now);
        let ev = deliver(&mut client, &mut server, now);
        let [Event::Request { socket: 250, ref req }] = ev[..] else { panic!("{ev:?}") };
        assert_eq!((req.data.as_slice(), req.user, req.bitmap), (&b"ping"[..], 7, 0b11));
        let resp = vec![ResponsePacket { user: 1, data: b"po".to_vec() }, ResponsePacket { user: 2, data: b"ng".to_vec() }];
        server.respond(250, req, resp.clone(), now);

        // Lose packet 0; the retry asks only for it.
        let lost = server.pop_frame().unwrap();
        assert!(deliver(&mut server, &mut client, now).is_empty());
        drop(lost);
        client.poll(now + REQUEST_RETRY);
        assert!(deliver(&mut client, &mut server, now).is_empty()); // answered from the XO cache
        let ev = deliver(&mut server, &mut client, now);
        assert_eq!(ev, vec![Event::Response { tid, packets: resp }]);
        // The client released the transaction.
        deliver(&mut client, &mut server, now);
        assert_eq!(server.responders[&250].cached(), 0);
    }

    #[test]
    fn request_gives_up() {
        let now = Instant::now();
        let mut client = ready(Role::Workstation, 12, now);
        let to = Addr { net: 0, node: 200, socket: 250 };
        let tid = client.request(to, b"x", 0, 1, now).unwrap();
        let mut t = now;
        let mut sent = 0;
        loop {
            let ev = client.poll(t);
            sent += std::iter::from_fn(|| client.pop_frame()).count();
            if ev == vec![Event::RequestFailed { tid }] {
                break;
            }
            t += REQUEST_RETRY;
        }
        assert_eq!(sent, REQUEST_TRIES as usize + 1);
    }

    #[test]
    fn requests_to_unopened_sockets_are_ignored() {
        let now = Instant::now();
        let mut server = ready(Role::Server, 200, now);
        let d = Datagram {
            src: Addr { net: 0, node: 12, socket: 0xFD },
            dst: Addr { net: 0, node: 200, socket: 99 },
            ddp_type: ddp::TYPE_ATP,
            data: atp::request(1, false, 1, 0, b""),
            long: false,
        };
        assert_eq!(server.receive(&ddp::build(&d), now), None);
    }
}
