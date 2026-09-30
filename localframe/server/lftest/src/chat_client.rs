//! `lftest chat`: join a chat hub from a PC, speaking the same protocol as
//! the Mac's LFTest in its Chat mode.

use std::collections::VecDeque;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use appletalk::{Addr, Event, Node};

use crate::chat;
use crate::serve::show;
use crate::{Options, stdin_lines};

pub fn run(o: Options) -> std::io::Result<()> {
    let nick = o.name;
    let mut node = Node::open(&o.cfg)?;
    let lines = stdin_lines();
    let mut hub: Option<Addr> = None;
    let mut lookup: Option<(u8, Instant)> = None;
    let mut join: Option<u16> = None;
    let mut poll: Option<u16> = None;
    let mut say: Option<u16> = None;
    let mut after = 0u16;
    let mut outbox: VecDeque<Vec<u8>> = VecDeque::new();
    let mut quitting: Option<Instant> = None;

    loop {
        let now = Instant::now();
        let ready = node.stack.node().is_some();

        // Find the hub, then join it.
        if ready && hub.is_none() && lookup.is_none_or(|(_, t)| now >= t) {
            let id = node.stack.lookup("=", chat::TYPE).unwrap();
            lookup = Some((id, now + Duration::from_secs(1)));
        }
        if let (Some(h), None, None, None) = (hub, join, poll, quitting) {
            let mut d = vec![chat::JOIN, 0];
            chat::pstr(&mut d, &nick);
            join = node.stack.request(h, &d, 0, 1, now);
        }
        if let (Some(h), None) = (hub, say)
            && let Some(text) = outbox.front()
        {
            let mut d = vec![chat::SAY, 0];
            d.extend(text);
            say = node.stack.request(h, &d, 0, 1, now);
        }

        for ev in node.step()? {
            match ev {
                Event::Ready(n) => eprintln!("we are node {n}; looking for a hub…"),
                Event::Found { id, addr, entity } if hub.is_none() && lookup.is_some_and(|(l, _)| l == id) => {
                    eprintln!("found hub {entity} at {addr}");
                    hub = Some(addr);
                }
                Event::Response { tid, packets } => {
                    let d = &packets[0].data;
                    let status = d.get(1).copied().unwrap_or(chat::NOT_JOINED);
                    if Some(tid) == join {
                        join = None;
                        if status == chat::OK && d.len() >= 4 {
                            after = u16::from_be_bytes([d[2], d[3]]);
                            let hub_nick = chat::read_pstr(&d[4..]).map(|(n, _)| chat::from_mac(n));
                            eprintln!("joined {}", hub_nick.unwrap_or_default());
                            poll = node.stack.request(hub.unwrap(), &poll_request(after), 0, 1, now);
                        }
                    } else if Some(tid) == poll {
                        poll = None;
                        if status == chat::OK {
                            for m in chat::parse_messages(&d[2..]).unwrap_or_default() {
                                if m.nick != nick {
                                    show(&m);
                                }
                                after = m.id;
                            }
                            poll = node.stack.request(hub.unwrap(), &poll_request(after), 0, 1, now);
                        } // NOT_JOINED: poll stays None, so we join again.
                    } else if Some(tid) == say {
                        say = None;
                        if status == chat::OK {
                            outbox.pop_front();
                        }
                    }
                }
                Event::RequestFailed { tid } if [join, poll, say].contains(&Some(tid)) => {
                    eprintln!("hub not answering; looking for it again");
                    (hub, lookup, join, poll, say) = (None, None, None, None, None);
                }
                _ => {}
            }
        }

        match lines.try_recv() {
            Ok(line) if line.trim() == "/quit" && quitting.is_none() => {
                if let Some(h) = hub {
                    node.stack.request(h, &[chat::LEAVE, 0], 0, 1, now);
                }
                quitting = Some(now + Duration::from_millis(300));
            }
            Ok(line) if !line.trim().is_empty() => outbox.push_back(chat::to_mac(&line)),
            Ok(_) | Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) if quitting.is_none() => quitting = Some(now + Duration::from_millis(300)),
            Err(TryRecvError::Disconnected) => {}
        }
        if quitting.is_some_and(|t| now >= t) {
            return Ok(());
        }
    }
}

fn poll_request(after: u16) -> Vec<u8> {
    let mut d = vec![chat::POLL, 0];
    d.extend(after.to_be_bytes());
    d
}
