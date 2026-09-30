//! rchat: chat between a terminal and classic Macs over AppleTalk.
//!
//! `rchat` runs a hub: it registers `NICK:RChat` and the Macs' RChat
//! application (and `rchat --join` on other PCs) connect to it. Whatever
//! is typed on the hub's terminal goes to everyone.

mod hub;
mod proto;

use std::collections::VecDeque;
use std::io::BufRead;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use appletalk::node::OPTIONS_HELP;
use appletalk::{Addr, Config, Entity, Event, Node, Role};

use hub::Hub;
use proto::Message;

const HUB_SOCKET: u8 = 251;

fn usage() -> String {
    format!(
        "usage: rchat [--nick NICK] [options]          run a hub; type lines to chat
       rchat --join [--nick NICK] [options]   join a hub from this PC

In the chat: /who lists who is connected (hub), /quit leaves.

options:
  --nick NICK     your name, also the hub's NBP name (default: host name)
{OPTIONS_HELP}"
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut join = false;
    let mut nick = hostname();
    let mut cfg = Config::new(Role::Server);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let r = match a.as_str() {
            "--join" => {
                join = true;
                Ok(())
            }
            "--nick" => it.next().map(|n| nick = n.clone()).ok_or("--nick needs a value".to_string()),
            "-h" | "--help" => Err(usage()),
            _ => match cfg.parse_arg(a, &mut it) {
                Ok(true) => Ok(()),
                Ok(false) => Err(format!("unknown option {a}\n\n{}", usage())),
                Err(e) => Err(e),
            },
        };
        if let Err(e) = r {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    }
    let nick = proto::to_mac(&nick);
    if nick.is_empty() || nick.len() > proto::MAX_NICK {
        eprintln!("--nick must be 1 to {} characters", proto::MAX_NICK);
        return ExitCode::FAILURE;
    }
    if join {
        cfg.role = Role::Workstation;
    }
    let lines = stdin_lines();
    let result = if join { run_client(&cfg, nick, lines) } else { run_hub(&cfg, nick, lines) };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().chars().take(proto::MAX_NICK).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "PC".into())
}

/// Lines typed on the terminal, read on their own thread.
fn stdin_lines() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn show(m: &Message) {
    if m.nick == b"*" {
        println!("* {}", proto::from_mac(&m.text));
    } else {
        println!("<{}> {}", proto::from_mac(&m.nick), proto::from_mac(&m.text));
    }
}

fn run_hub(cfg: &Config, nick: Vec<u8>, lines: Receiver<String>) -> std::io::Result<()> {
    let mut node = Node::open(cfg)?;
    let entity = Entity { object: nick.clone(), kind: proto::TYPE.as_bytes().to_vec(), zone: b"*".to_vec() };
    node.stack.register(entity.clone(), HUB_SOCKET);
    let mut hub = Hub::new(nick.clone());
    eprintln!("joined LToUDP, acquiring a node address…");
    loop {
        let now = Instant::now();
        let mut answers = Vec::new();
        for ev in node.step()? {
            match ev {
                Event::Ready(n) => eprintln!("node {n}: hub {entity} is up; waiting for Macs"),
                Event::Request { socket, req } => {
                    let (answer, posted) = hub.handle(socket, req, now);
                    answers.extend(answer);
                    posted.iter().for_each(show);
                }
                _ => {}
            }
        }
        let (due, posted) = hub.due(now);
        answers.extend(due);
        posted.iter().for_each(show);
        for (socket, req, response) in answers {
            node.stack.respond(socket, &req, response, now);
        }

        match lines.try_recv() {
            Ok(line) if line.trim() == "/quit" => return Ok(()),
            Ok(line) if line.trim() == "/who" => println!("* connected: {}", hub.nicks().join(", ")),
            Ok(line) if !line.trim().is_empty() => {
                hub.post(&nick, &proto::to_mac(&line));
            }
            Ok(_) | Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return Ok(()),
        }
        node.flush()?;
    }
}

/// A PC client: the same protocol the Mac's RChat speaks.
fn run_client(cfg: &Config, nick: Vec<u8>, lines: Receiver<String>) -> std::io::Result<()> {
    let mut node = Node::open(cfg)?;
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
            let id = node.stack.lookup("=", proto::TYPE).unwrap();
            lookup = Some((id, now + Duration::from_secs(1)));
        }
        if let (Some(h), None, None, None) = (hub, join, poll, quitting) {
            let mut d = vec![proto::JOIN, 0];
            proto::pstr(&mut d, &nick);
            join = node.stack.request(h, &d, 0, 1, now);
        }
        if let (Some(h), None) = (hub, say)
            && let Some(text) = outbox.front() {
                let mut d = vec![proto::SAY, 0];
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
                    let status = d.get(1).copied().unwrap_or(proto::NOT_JOINED);
                    if Some(tid) == join {
                        join = None;
                        if status == proto::OK && d.len() >= 4 {
                            after = u16::from_be_bytes([d[2], d[3]]);
                            let hub_nick = proto::read_pstr(&d[4..]).map(|(n, _)| proto::from_mac(n));
                            eprintln!("joined {}", hub_nick.unwrap_or_default());
                            poll = node.stack.request(hub.unwrap(), &poll_request(after), 0, 1, now);
                        }
                    } else if Some(tid) == poll {
                        poll = None;
                        if status == proto::OK {
                            for m in proto::parse_messages(&d[2..]).unwrap_or_default() {
                                if m.nick != nick {
                                    show(&m);
                                }
                                after = m.id;
                            }
                            poll = node.stack.request(hub.unwrap(), &poll_request(after), 0, 1, now);
                        } // NOT_JOINED: poll stays None, so we join again.
                    } else if Some(tid) == say {
                        say = None;
                        if status == proto::OK {
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
                    node.stack.request(h, &[proto::LEAVE, 0], 0, 1, now);
                }
                quitting = Some(now + Duration::from_millis(300));
            }
            Ok(line) if !line.trim().is_empty() => outbox.push_back(proto::to_mac(&line)[..].to_vec()),
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
    let mut d = vec![proto::POLL, 0];
    d.extend(after.to_be_bytes());
    d
}
