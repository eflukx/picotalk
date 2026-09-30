//! `lftest serve`: one node offering both test services.
//!
//! * `NAME:LFEcho` on socket 250, answered at once (see `echo`);
//! * `NAME:RChat` on socket 251, a chat hub that holds polls (see `hub`).
//!
//! Lines typed on the terminal are posted to the chat as NAME.

use std::sync::mpsc::TryRecvError;
use std::time::Instant;

use appletalk::{Entity, Event, Node, Role};

use crate::chat::{self, Message};
use crate::echo::{self, Echo};
use crate::hub::Hub;
use crate::{Options, stdin_lines};

pub const ECHO_SOCKET: u8 = 250;
pub const CHAT_SOCKET: u8 = 251;

pub fn show(m: &Message) {
    if m.nick == b"*" {
        println!("* {}", chat::from_mac(&m.text));
    } else {
        println!("<{}> {}", chat::from_mac(&m.nick), chat::from_mac(&m.text));
    }
}

pub fn run(mut o: Options) -> std::io::Result<()> {
    // Same name, same address after a restart, so clients need not look again.
    o.cfg.node = o.cfg.node.or(Some(Role::Server.node_for(&o.name)));
    let mut node = Node::open(&o.cfg)?;
    let entity = |kind: &str| Entity { object: o.name.clone(), kind: kind.as_bytes().to_vec(), zone: b"*".to_vec() };
    node.stack.register(entity(echo::TYPE), ECHO_SOCKET);
    node.stack.register(entity(chat::TYPE), CHAT_SOCKET);
    let mut echo = Echo::default();
    let mut hub = Hub::new(o.name.clone());
    let lines = stdin_lines();
    eprintln!("joined LToUDP, acquiring a node address…");

    loop {
        let now = Instant::now();
        let mut answers = Vec::new();
        for ev in node.step()? {
            match ev {
                Event::Ready(n) => eprintln!(
                    "node {n}: serving {} on socket {ECHO_SOCKET} and {} on socket {CHAT_SOCKET}\n\
                     type to chat; /who lists who is connected, /quit stops",
                    entity(echo::TYPE),
                    entity(chat::TYPE)
                ),
                Event::LookedUp { from, pattern } if o.verbose => eprintln!("lookup for {pattern} from {from}"),
                Event::Request { socket: ECHO_SOCKET, req } => {
                    let response = echo.handle(&req, node.stack.node().unwrap_or(0));
                    if o.verbose || req.data.first() != Some(&echo::PING) {
                        eprintln!(
                            "echo: {} cmd {:02x}, {} bytes in, {} packets out",
                            req.from,
                            req.data.first().copied().unwrap_or(0),
                            req.data.len(),
                            response.len()
                        );
                    }
                    answers.push((ECHO_SOCKET, req, response));
                }
                Event::Request { socket, req } => {
                    if o.verbose {
                        eprintln!("chat: {} cmd {:02x}", req.from, req.data.first().copied().unwrap_or(0));
                    }
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
                hub.post(&o.name, &chat::to_mac(&line));
            }
            Ok(_) | Err(TryRecvError::Empty) => {}
            // No terminal (stdin closed, e.g. run as a service): keep serving.
            Err(TryRecvError::Disconnected) => {}
        }
        node.flush()?;
    }
}
