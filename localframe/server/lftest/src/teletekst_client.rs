//! `lftest teletekst`: the Mac's Teletekst client, in a terminal. Shows the
//! page as the Mac gets it (mosaic cells redrawn as block characters, in
//! the 8 colours); type a page number or keys and Enter.
//!
//! With `LFTEST_TELETEKST_DUMP=FILE` set, each screen is also written to
//! FILE as it arrives: 26 × 40 × (char, attr) bytes, for tools that render
//! it as the Mac does.

use std::time::{Duration, Instant};

use appletalk::{Addr, Event, Node};

use crate::teletekst::{self, COLS, MOSAIC, POLL_PACKETS, ROWS, bg, fg};
use crate::{Options, stdin_lines};

pub fn run(o: Options) -> std::io::Result<()> {
    let mut node = Node::open(&o.cfg)?;
    let lines = stdin_lines();
    let mut server: Option<Addr> = None;
    let mut lookup: Option<(u8, Instant)> = None;
    let mut poll: Option<u16> = None;
    let mut have = 0u16;
    let mut screen = vec![[(b' ', teletekst::WHITE); COLS]; ROWS];

    loop {
        let now = Instant::now();
        if node.stack.node().is_some() && server.is_none() && lookup.is_none_or(|(_, t)| now >= t) {
            lookup = Some((node.stack.lookup("=", teletekst::TYPE).unwrap(), now + Duration::from_secs(1)));
        }
        if let (Some(s), None) = (server, poll) {
            let mut d = vec![teletekst::POLL, 0];
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
                    if d.len() >= 6 && d[1] == teletekst::OK {
                        have = u16::from_be_bytes([d[2], d[3]]);
                        let n = d[4] as usize;
                        for r in d[6..].chunks(1 + 2 * COLS).take(n) {
                            let row = r[0] as usize;
                            for c in 0..COLS.min((r.len() - 1) / 2) {
                                screen[row][c] = (r[1 + 2 * c], r[2 + 2 * c]);
                            }
                        }
                        if n > 0 {
                            show(&screen);
                            if let Ok(path) = std::env::var("LFTEST_TELETEKST_DUMP") {
                                let raw: Vec<u8> = screen.iter().flatten().flat_map(|&(c, a)| [c, a]).collect();
                                let _ = std::fs::write(path, raw);
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
        if let Ok(line) = lines.try_recv() {
            if line.trim() == "/quit" {
                return Ok(());
            }
            if let Some(s) = server {
                let mut d = vec![teletekst::KEY, 0];
                d.extend(line.bytes());
                node.stack.request(s, &d, 0, 1, now);
            }
        }
    }
}

fn show(screen: &[[(u8, u8); COLS]]) {
    let mut out = String::from("\x1b[H\x1b[2J");
    for row in screen {
        for &(ch, attr) in row {
            out.push_str(&format!("\x1b[{};{}m", 30 + fg(attr), 40 + bg(attr)));
            if attr & MOSAIC != 0 {
                out.push(sextant(ch));
            } else {
                out.push_str(&crate::chat::from_mac(&[ch]).chars().next().map_or(" ".into(), |c| c.to_string()));
            }
        }
        out.push_str("\x1b[0m\n");
    }
    print!("{out}");
}

/// A 2×3 pattern back as a block character.
fn sextant(p: u8) -> char {
    match p {
        0 => ' ',
        21 => '▌',
        42 => '▐',
        63 => '█',
        p => {
            let mut i = p as u32 - 1;
            if p > 42 {
                i -= 1;
            }
            if p > 21 {
                i -= 1;
            }
            char::from_u32(0x1FB00 + i).unwrap_or('?')
        }
    }
}
