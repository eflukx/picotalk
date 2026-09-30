//! The hub: the message log, who has joined, and the POLLs being held.
//! Pure logic; `main` connects it to the AppleTalk node and the terminal.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use appletalk::atp::{MAX_DATA, Request, ResponsePacket};

use crate::proto::{self, Message};

/// Messages kept for clients that fall behind, and for replay on JOIN.
const LOG_LEN: usize = 64;
const REPLAY: usize = 10;

struct Client {
    nick: Vec<u8>,
    last_seen: Instant,
}

struct Held {
    socket: u8,
    req: Request,
    after: u16,
    deadline: Instant,
}

pub struct Hub {
    pub nick: Vec<u8>,
    log: VecDeque<Message>,
    next_id: u16,
    /// Keyed by LLAP node: one client per node.
    clients: HashMap<u8, Client>,
    held: Vec<Held>,
}

/// An answer to send: (socket, request, response).
pub type Answer = (u8, Request, Vec<ResponsePacket>);

impl Hub {
    pub fn new(nick: Vec<u8>) -> Self {
        Self { nick, log: VecDeque::new(), next_id: 1, clients: HashMap::new(), held: Vec::new() }
    }

    /// Adds a message to the log and returns it.
    pub fn post(&mut self, nick: &[u8], text: &[u8]) -> Message {
        let m = Message {
            id: self.next_id,
            nick: nick[..nick.len().min(proto::MAX_NICK)].to_vec(),
            text: text[..text.len().min(proto::MAX_TEXT)].to_vec(),
        };
        self.next_id = self.next_id.wrapping_add(1).max(1);
        if self.log.len() == LOG_LEN {
            self.log.pop_front();
        }
        self.log.push_back(m.clone());
        m
    }

    fn system(&mut self, text: String) -> Message {
        self.post(b"*", &proto::to_mac(&text))
    }

    pub fn nicks(&self) -> Vec<String> {
        self.clients.values().map(|c| proto::from_mac(&c.nick)).collect()
    }

    /// Handles a request. Returns the answers to send now (none if it is a
    /// POLL being held) and the messages the request posted.
    pub fn handle(&mut self, socket: u8, req: Request, now: Instant) -> (Vec<Answer>, Vec<Message>) {
        let node = req.from.node;
        let cmd = req.data.first().copied().unwrap_or(0);
        let known = self.clients.contains_key(&node);
        if let Some(c) = self.clients.get_mut(&node) {
            c.last_seen = now;
        }
        let mut posted = Vec::new();
        let mut answers = Vec::new();
        let reply = match cmd {
            proto::JOIN => {
                let nick = proto::read_pstr(req.data.get(2..).unwrap_or(&[]))
                    .map(|(n, _)| n)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(b"Mac");
                let nick = nick[..nick.len().min(proto::MAX_NICK)].to_vec();
                let joined = format!("{} joined", proto::from_mac(&nick));
                self.clients.insert(node, Client { nick, last_seen: now });
                posted.push(self.system(joined));
                let skip = self.log.len().saturating_sub(REPLAY);
                let after = self.log.get(skip).map_or(self.next_id, |m| m.id).wrapping_sub(1);
                let mut r = vec![proto::JOIN, proto::OK];
                r.extend(after.to_be_bytes());
                proto::pstr(&mut r, &self.nick);
                r
            }
            proto::SAY if known => {
                let nick = self.clients[&node].nick.clone();
                posted.push(self.post(&nick, req.data.get(2..).unwrap_or(&[])));
                vec![proto::SAY, proto::OK]
            }
            proto::POLL if known => {
                let after = u16::from_be_bytes([req.data.get(2).copied().unwrap_or(0), req.data.get(3).copied().unwrap_or(0)]);
                if self.newer(after).next().is_none() {
                    self.held.push(Held { socket, req, after, deadline: now + proto::HOLD });
                    return (answers, posted);
                }
                self.poll_response(after)
            }
            proto::LEAVE if known => {
                let nick = self.clients.remove(&node).unwrap().nick;
                posted.push(self.system(format!("{} left", proto::from_mac(&nick))));
                // Release the client's held POLL, so it can quit at once.
                let (theirs, others) = std::mem::take(&mut self.held).into_iter().partition(|h| h.req.from.node == node);
                self.held = others;
                for h in theirs {
                    answers.push((h.socket, h.req, vec![ResponsePacket { user: 0, data: vec![proto::POLL, proto::NOT_JOINED] }]));
                }
                vec![proto::LEAVE, proto::OK]
            }
            _ => vec![cmd, proto::NOT_JOINED],
        };
        answers.push((socket, req, vec![ResponsePacket { user: 0, data: reply }]));
        (answers, posted)
    }

    /// Held POLLs that can be answered now (new messages, or held long
    /// enough), and clients that timed out.
    pub fn due(&mut self, now: Instant) -> (Vec<Answer>, Vec<Message>) {
        let mut posted = Vec::new();
        let gone: Vec<u8> = self
            .clients
            .iter()
            .filter(|(_, c)| now.duration_since(c.last_seen) > proto::CLIENT_TIMEOUT)
            .map(|(&n, _)| n)
            .collect();
        for node in gone {
            let nick = self.clients.remove(&node).unwrap().nick;
            posted.push(self.system(format!("{} timed out", proto::from_mac(&nick))));
        }

        let mut answers = Vec::new();
        let held = std::mem::take(&mut self.held);
        for h in held {
            if now >= h.deadline || self.newer(h.after).next().is_some() {
                let data = self.poll_response(h.after);
                answers.push((h.socket, h.req, vec![ResponsePacket { user: 0, data }]));
            } else {
                self.held.push(h);
            }
        }
        (answers, posted)
    }

    /// Log entries after `after`. If `after` has already dropped out of the
    /// log (or is from before a hub restart), everything in the log.
    fn newer(&self, after: u16) -> impl Iterator<Item = &Message> {
        let known = self.log.iter().any(|m| m.id == after);
        self.log.iter().filter(move |m| !known || (m.id.wrapping_sub(after) as i16) > 0)
    }

    fn poll_response(&self, after: u16) -> Vec<u8> {
        let mut r = vec![proto::POLL, proto::OK, 0, 0];
        let mut count = 0u8;
        for m in self.newer(after) {
            if r.len() + m.wire_len() > MAX_DATA || count == u8::MAX {
                break;
            }
            m.write(&mut r);
            count += 1;
        }
        r[2] = count;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use appletalk::Addr;
    use std::time::Duration;

    fn req(node: u8, data: &[u8]) -> Request {
        Request { from: Addr { net: 0, node, socket: 254 }, tid: 1, xo: true, bitmap: 1, user: 0, data: data.to_vec() }
    }

    fn body(a: &[Answer]) -> Vec<u8> {
        assert_eq!(a.len(), 1);
        a[0].2[0].data.clone()
    }

    fn join(hub: &mut Hub, node: u8, nick: &[u8], now: Instant) -> u16 {
        let mut d = vec![proto::JOIN, 0];
        proto::pstr(&mut d, nick);
        let b = body(&hub.handle(250, req(node, &d), now).0);
        assert_eq!(&b[..2], &[proto::JOIN, proto::OK]);
        u16::from_be_bytes([b[2], b[3]])
    }

    fn poll(after: u16) -> Vec<u8> {
        let mut d = vec![proto::POLL, 0];
        d.extend(after.to_be_bytes());
        d
    }

    #[test]
    fn unknown_clients_must_join() {
        let now = Instant::now();
        let mut hub = Hub::new(b"pc".to_vec());
        assert_eq!(body(&hub.handle(250, req(5, &poll(0)), now).0), [proto::POLL, proto::NOT_JOINED]);
        assert_eq!(body(&hub.handle(250, req(5, b"\x02\x00hi"), now).0), [proto::SAY, proto::NOT_JOINED]);
    }

    #[test]
    fn join_replays_and_poll_is_held_until_a_message() {
        let now = Instant::now();
        let mut hub = Hub::new(b"pc".to_vec());
        hub.post(b"pc", b"earlier");
        let after = join(&mut hub, 5, b"mac", now);

        // The replay: "earlier" and "mac joined".
        let b = body(&hub.handle(250, req(5, &poll(after)), now).0);
        let msgs = proto::parse_messages(&b[2..]).unwrap();
        assert_eq!(msgs.iter().map(|m| m.text.as_slice()).collect::<Vec<_>>(), [&b"earlier"[..], b"mac joined"]);
        let last = msgs.last().unwrap().id;

        // Nothing new: held.
        assert!(hub.handle(250, req(5, &poll(last)), now).0.is_empty());
        assert!(hub.due(now).0.is_empty());
        let posted = hub.post(b"pc", b"hello");
        let (answers, _) = hub.due(now);
        assert_eq!(answers.len(), 1);
        let msgs = proto::parse_messages(&answers[0].2[0].data[2..]).unwrap();
        assert_eq!(msgs, vec![posted]);
    }

    #[test]
    fn held_poll_times_out_empty_and_clients_expire() {
        let now = Instant::now();
        let mut hub = Hub::new(b"pc".to_vec());
        let after = join(&mut hub, 5, b"mac", now);
        let b = body(&hub.handle(250, req(5, &poll(after)), now).0);
        let last = proto::parse_messages(&b[2..]).unwrap().last().unwrap().id;
        assert!(hub.handle(250, req(5, &poll(last)), now).0.is_empty());
        let (answers, _) = hub.due(now + proto::HOLD);
        assert_eq!(answers[0].2[0].data, [proto::POLL, proto::OK, 0, 0]);

        let (_, posted) = hub.due(now + proto::CLIENT_TIMEOUT + Duration::from_secs(1));
        assert_eq!(posted[0].text, b"mac timed out");
        assert!(hub.nicks().is_empty());
    }

    #[test]
    fn say_and_leave() {
        let now = Instant::now();
        let mut hub = Hub::new(b"pc".to_vec());
        join(&mut hub, 5, b"mac", now);
        let (a, posted) = hub.handle(250, req(5, b"\x02\x00hi there"), now);
        assert_eq!(body(&a), [proto::SAY, proto::OK]);
        assert_eq!((posted[0].nick.as_slice(), posted[0].text.as_slice()), (&b"mac"[..], &b"hi there"[..]));
        // A held POLL is released by LEAVE.
        let last = hub.post(b"pc", b"x").id;
        assert!(hub.handle(250, req(5, &poll(last)), now).0.is_empty());
        let (answers, posted) = hub.handle(250, req(5, &[proto::LEAVE, 0]), now);
        assert_eq!(posted[0].text, b"mac left");
        let bodies: Vec<_> = answers.iter().map(|a| a.2[0].data.clone()).collect();
        assert_eq!(bodies, [vec![proto::POLL, proto::NOT_JOINED], vec![proto::LEAVE, proto::OK]]);
    }

    #[test]
    fn poll_response_fits_one_packet() {
        let now = Instant::now();
        let mut hub = Hub::new(b"pc".to_vec());
        for _ in 0..10 {
            hub.post(b"pc", &[b'x'; proto::MAX_TEXT]);
        }
        let after = join(&mut hub, 5, b"mac", now);
        let b = body(&hub.handle(250, req(5, &poll(after)), now).0);
        assert!(b.len() <= MAX_DATA);
        assert_eq!(proto::parse_messages(&b[2..]).unwrap().len(), 2);
    }

    #[test]
    fn stale_after_gets_the_whole_log() {
        let now = Instant::now();
        let mut hub = Hub::new(b"pc".to_vec());
        join(&mut hub, 5, b"mac", now);
        hub.post(b"pc", b"a");
        let b = body(&hub.handle(250, req(5, &poll(40_000)), now).0);
        assert_eq!(proto::parse_messages(&b[2..]).unwrap().len(), 2);
    }
}
