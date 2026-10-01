//! `lftest tanks`: the Mac's Tanks client, in a terminal. Prints the
//! dashboard each time a new snapshot arrives.

use std::time::{Duration, Instant};

use appletalk::{Addr, Event, Node};

use crate::chat::{from_mac, read_pstr};
use crate::tanks::{self, POLL, POLL_PACKETS};
use crate::{Options, stdin_lines};

pub fn run(o: Options) -> std::io::Result<()> {
    let mut node = Node::open(&o.cfg)?;
    let lines = stdin_lines();
    let mut server: Option<Addr> = None;
    let mut lookup: Option<(u8, Instant)> = None;
    let mut poll: Option<u16> = None;
    let mut have = 0u16;

    loop {
        let now = Instant::now();
        if node.stack.node().is_some() && server.is_none() && lookup.is_none_or(|(_, t)| now >= t) {
            lookup = Some((node.stack.lookup("=", tanks::TYPE).unwrap(), now + Duration::from_secs(1)));
        }
        if let (Some(s), None) = (server, poll) {
            let mut d = vec![POLL, 0];
            d.extend(have.to_be_bytes());
            poll = node.stack.request(s, &d, 0, POLL_PACKETS, now);
        }
        for ev in node.step()? {
            match ev {
                Event::Found { id, addr, entity } if server.is_none() && lookup.is_some_and(|(l, _)| l == id) => {
                    eprintln!("found {entity} at {addr}");
                    server = Some(addr);
                }
                Event::Response { tid, packets } if Some(tid) == poll => {
                    poll = None;
                    let d: Vec<u8> = packets.iter().flat_map(|p| p.data.iter().copied()).collect();
                    if d.len() >= 4 && d[1] == tanks::OK {
                        let version = u16::from_be_bytes([d[2], d[3]]);
                        if version != have && d.len() > 4 {
                            have = version;
                            match show(&d[4..]) {
                                Some(text) => print!("{text}"),
                                None => eprintln!("snapshot {version}: cannot parse it"),
                            }
                        }
                    }
                }
                Event::RequestFailed { tid } if Some(tid) == poll => {
                    eprintln!("server not answering; looking for it again");
                    (server, poll, lookup) = (None, None, None);
                }
                _ => {}
            }
        }
        if let Ok(line) = lines.try_recv()
            && line.trim() == "/quit"
        {
            return Ok(());
        }
    }
}

/// The snapshot as text, or None if it is malformed.
fn show(b: &[u8]) -> Option<String> {
    fn s(b: &mut &[u8]) -> Option<String> {
        let (v, rest) = read_pstr(b)?;
        *b = rest;
        Some(from_mac(v))
    }
    fn byte(b: &mut &[u8]) -> Option<u8> {
        let (&v, rest) = b.split_first()?;
        *b = rest;
        Some(v)
    }
    let mut b = b;
    let mut out = String::from("\n");
    let n = byte(&mut b)?;
    for _ in 0..n {
        let name = s(&mut b)?;
        let arrow = ["=", "▲", "▼", "?"].get(byte(&mut b)? as usize).copied().unwrap_or("?");
        let status = s(&mut b)?;
        let level = u16::from_be_bytes([byte(&mut b)?, byte(&mut b)?]);
        let v: Vec<String> = (0..6).map(|_| s(&mut b)).collect::<Option<_>>()?;
        out += &format!(
            "{name:<18} {:>5.1}%  {arrow} {status:<15} vandaag +{} -{} = {:<8} gisteren +{} -{} = {}\n",
            level as f64 / 10.0,
            v[0],
            v[1],
            v[2],
            v[3],
            v[4],
            v[5]
        );
    }
    if byte(&mut b)? == 1 {
        out += &format!("weer: {} {} — {}\n", s(&mut b)?, s(&mut b)?, s(&mut b)?);
    }
    out += &format!("laatste update: {}\n", s(&mut b)?);
    let problem = s(&mut b)?;
    if !problem.is_empty() {
        out += &format!("!! {problem}\n");
    }
    Some(out)
}
