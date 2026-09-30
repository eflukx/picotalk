//! The `Teletekst` service: NOS Teletekst for classic Macs.
//!
//! Each Mac gets its own `ssh teletekst.nl` session in a 40×26 pseudo-
//! terminal. A terminal emulator (`vt100`) keeps the screen, and the Mac
//! long-polls for the rows that changed since the screen it has, as the
//! chat does for messages. Keys the Mac sends go to the session.
//!
//! ```text
//! POLL  03 00 have(2)  → 03 st seq(2) nrows 00 { row  40 × (char attr) } × nrows
//! KEY   02 00 keys…    → 02 st
//! ```
//!
//! `seq` numbers the screens this client was sent; `have` is the last one
//! it applied (0 at first). If `have` is the last one sent, only rows that
//! differ are sent; otherwise all rows. A POLL is held until the screen
//! changes, or for [`HOLD`].
//!
//! A cell is a character (Mac Roman) and an attribute byte:
//!
//! * bits 0–2: foreground colour, bits 3–5: background colour, as the 8
//!   Teletekst colours: bit 0 red, bit 1 green, bit 2 blue (0 black,
//!   1 red, 2 green, 3 yellow, 4 blue, 5 magenta, 6 cyan, 7 white). The
//!   Mac shows them as dither patterns.
//! * bit 6 [`MOSAIC`]: the character is a 2×3 block-graphics pattern, bit
//!   0 top left, 1 top right, 2 middle left, … 5 bottom right.
//!
//! Keys: bytes as typed; Mac arrow keys (1C–1F) become ANSI arrow keys.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use appletalk::atp::{MAX_DATA, Request, ResponsePacket};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

pub const TYPE: &str = "Teletekst";

pub const KEY: u8 = 2;
pub const POLL: u8 = 3;
pub const OK: u8 = 0;

pub const COLS: usize = 40;
/// A Teletekst page is 25 rows; the service adds a status line below it.
pub const ROWS: usize = 26;

pub const MOSAIC: u8 = 0x40;
pub const BLACK: u8 = 0;
pub const WHITE: u8 = 7;

pub fn fg(attr: u8) -> u8 {
    attr & 7
}

pub fn bg(attr: u8) -> u8 {
    (attr >> 3) & 7
}

/// How long a POLL waits for the screen to change.
pub const HOLD: Duration = Duration::from_secs(2);
/// A session nobody polled for this long is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Response packets a client should allow a POLL: a full screen is about
/// 2 KB.
pub const POLL_PACKETS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: u8,
    pub attr: u8,
}

pub type Row = [Cell; COLS];
pub type Screen = Vec<Row>;

/// One ssh session in a pseudo-terminal.
struct Session {
    parser: Arc<Mutex<vt100::Parser>>,
    /// Bumped whenever output arrives.
    changed: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// The last screen sent to the client, and its number.
    sent: Option<(u16, Screen)>,
    last_seen: Instant,
}

impl Session {
    fn start(host: &str) -> std::io::Result<Self> {
        fn err(e: impl std::fmt::Display) -> std::io::Error {
            std::io::Error::other(e.to_string())
        }
        let pair = native_pty_system()
            .openpty(PtySize { rows: ROWS as u16, cols: COLS as u16, pixel_width: 0, pixel_height: 0 })
            .map_err(err)?;
        let mut cmd = CommandBuilder::new("ssh");
        // No keys or passwords: the service needs no login, and our keys
        // are none of its business.
        for a in [
            "-tt",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "PubkeyAuthentication=no",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            host,
        ] {
            cmd.arg(a);
        }
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).map_err(err)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(err)?;
        let writer = pair.master.take_writer().map_err(err)?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(ROWS as u16, COLS as u16, 0)));
        let changed = Arc::new(AtomicU64::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        {
            let (parser, changed, alive) = (parser.clone(), changed.clone(), alive.clone());
            // The master must outlive the reader thread's use of it.
            let master = pair.master;
            std::thread::spawn(move || {
                let _master = master;
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    parser.lock().unwrap().process(&buf[..n]);
                    changed.fetch_add(1, Ordering::Relaxed);
                }
                alive.store(false, Ordering::Relaxed);
                changed.fetch_add(1, Ordering::Relaxed);
            });
        }
        Ok(Self { parser, changed, alive, writer, child, sent: None, last_seen: Instant::now() })
    }

    fn screen(&self) -> Screen {
        if !self.alive.load(Ordering::Relaxed) {
            return message_screen(&["Verbinding met teletekst.nl", "verbroken.", "", "Druk op een toets om", "opnieuw te verbinden."]);
        }
        let parser = self.parser.lock().unwrap();
        extract(parser.screen())
    }

    fn keys(&mut self, keys: &[u8]) {
        let mut out = Vec::new();
        for &k in keys {
            match k {
                0x1C => out.extend(b"\x1b[D"),
                0x1D => out.extend(b"\x1b[C"),
                0x1E => out.extend(b"\x1b[A"),
                0x1F => out.extend(b"\x1b[B"),
                0x08 => out.push(0x7F),
                k => out.push(k),
            }
        }
        let _ = self.writer.write_all(&out).and_then(|_| self.writer.flush());
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// The screen as the Mac shows it.
fn extract(s: &vt100::Screen) -> Screen {
    (0..ROWS)
        .map(|r| {
            let mut row = [Cell { ch: b' ', attr: WHITE }; COLS];
            for (c, cell) in row.iter_mut().enumerate() {
                if let Some(vc) = s.cell(r as u16, c as u16) {
                    *cell = convert(vc);
                }
            }
            row
        })
        .collect()
}

fn convert(vc: &vt100::Cell) -> Cell {
    let (mut fg, mut bg) = (colour(vc.fgcolor(), WHITE), colour(vc.bgcolor(), BLACK));
    if vc.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }
    let attr = fg | bg << 3;
    let ch = vc.contents().chars().next().unwrap_or(' ');
    match mosaic(ch) {
        Some(p) => Cell { ch: p, attr: attr | MOSAIC },
        None => Cell { ch: to_mac(ch), attr },
    }
}

/// The nearest of the 8 Teletekst colours; `default` for the terminal's
/// default colour.
fn colour(c: vt100::Color, default: u8) -> u8 {
    let rgb = |r: bool, g: bool, b: bool| r as u8 | (g as u8) << 1 | (b as u8) << 2;
    match c {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => rgb(r >= 128, g >= 128, b >= 128),
        // The basic ANSI colours use the same bit order.
        vt100::Color::Idx(i) if i < 16 => i % 8,
        // The 6×6×6 cube and the grey ramp.
        vt100::Color::Idx(i) if i < 232 => {
            let n = i - 16;
            rgb(n / 36 >= 3, (n / 6) % 6 >= 3, n % 6 >= 3)
        }
        vt100::Color::Idx(i) => if i >= 244 { WHITE } else { BLACK },
    }
}

/// A block-graphics character as a 2×3 pattern.
fn mosaic(ch: char) -> Option<u8> {
    let p = match ch {
        // Symbols for Legacy Computing: sextants for patterns 1..62,
        // except 21 and 42, which are the half blocks below.
        '\u{1FB00}'..='\u{1FB3B}' => {
            let mut p = ch as u32 - 0x1FB00 + 1;
            if p >= 21 {
                p += 1;
            }
            if p >= 42 {
                p += 1;
            }
            p as u8
        }
        '█' => 63,
        '▌' => 21,
        '▐' => 42,
        '▀' => 15,
        '▄' => 60,
        '▘' => 1,
        '▝' => 2,
        '▖' => 16,
        '▗' => 32,
        _ => return None,
    };
    Some(p)
}

fn to_mac(ch: char) -> u8 {
    match ch {
        '←' => b'<',
        '→' => b'>',
        '↑' => b'^',
        '↓' => b'v',
        c if c.is_ascii_graphic() || c == ' ' => c as u8,
        c => {
            let mut buf = [0u8; 4];
            let (b, _, bad) = encoding_rs::MACINTOSH.encode(c.encode_utf8(&mut buf));
            if bad || b.len() != 1 { b'?' } else { b[0] }
        }
    }
}

/// A screen with some lines of text in the middle, for status messages.
fn message_screen(lines: &[&str]) -> Screen {
    let mut s = vec![[Cell { ch: b' ', attr: WHITE }; COLS]; ROWS];
    let top = (ROWS - lines.len()) / 2;
    for (i, line) in lines.iter().enumerate() {
        let start = (COLS.saturating_sub(line.chars().count())) / 2;
        for (j, ch) in line.chars().take(COLS).enumerate() {
            s[top + i][start + j].ch = to_mac(ch);
        }
    }
    s
}

struct Held {
    socket: u8,
    req: Request,
    have: u16,
    deadline: Instant,
    /// The session's output counter when last compared.
    mark: u64,
}

/// An answer to send: (socket, request, response).
pub type Answer = (u8, Request, Vec<ResponsePacket>);

pub struct Teletekst {
    host: String,
    sessions: HashMap<u8, Session>,
    held: Vec<Held>,
}

impl Teletekst {
    pub fn new(host: String) -> Self {
        Self { host, sessions: HashMap::new(), held: Vec::new() }
    }

    /// The session for a node, started (or restarted) as needed.
    fn session(&mut self, node: u8, now: Instant) -> Option<&mut Session> {
        if !self.sessions.contains_key(&node) {
            match Session::start(&self.host) {
                Ok(s) => {
                    eprintln!("teletekst: node {node}: connecting to {}", self.host);
                    self.sessions.insert(node, s);
                }
                Err(e) => {
                    eprintln!("teletekst: cannot start ssh: {e}");
                    return None;
                }
            }
        }
        let s = self.sessions.get_mut(&node)?;
        s.last_seen = now;
        Some(s)
    }

    pub fn handle(&mut self, socket: u8, req: Request, now: Instant) -> Vec<Answer> {
        let node = req.from.node;
        let cmd = req.data.first().copied().unwrap_or(0);
        match cmd {
            KEY => {
                // After the session ended, a key starts a new one.
                if self.sessions.get(&node).is_some_and(|s| !s.alive.load(Ordering::Relaxed)) {
                    self.sessions.remove(&node);
                }
                if let Some(s) = self.session(node, now) {
                    s.keys(req.data.get(2..).unwrap_or(&[]));
                }
                vec![(socket, req, vec![ResponsePacket { user: 0, data: vec![KEY, OK] }])]
            }
            POLL => {
                let have = u16::from_be_bytes([req.data.get(2).copied().unwrap_or(0), req.data.get(3).copied().unwrap_or(0)]);
                let Some(s) = self.session(node, now) else {
                    let screen = message_screen(&["Kan ssh niet starten", "op de server."]);
                    let data = encode(1, &changed_rows(None, &screen));
                    return vec![(socket, req, packets(data))];
                };
                let screen = s.screen();
                let base = s.sent.as_ref().filter(|(seq, _)| *seq == have).map(|(_, sc)| sc);
                if base == Some(&screen) {
                    let mark = s.changed.load(Ordering::Relaxed);
                    self.held.push(Held { socket, req, have, deadline: now + HOLD, mark });
                    return Vec::new();
                }
                vec![(socket, req, self.update(node, have))]
            }
            _ => vec![(socket, req, vec![ResponsePacket { user: 0, data: vec![cmd, 0xFF] }])],
        }
    }

    /// The rows that changed since the client's screen `have`, as the next
    /// screen number, or an empty update if nothing did.
    fn update(&mut self, node: u8, have: u16) -> Vec<ResponsePacket> {
        let Some(s) = self.sessions.get_mut(&node) else { return packets(encode(have, &[])) };
        let screen = s.screen();
        let base = s.sent.as_ref().filter(|(seq, _)| *seq == have).map(|(_, sc)| sc);
        let rows = changed_rows(base, &screen);
        if rows.is_empty() {
            return packets(encode(have, &[]));
        }
        let seq = have.wrapping_add(1).max(1);
        let data = encode(seq, &rows);
        s.sent = Some((seq, screen));
        packets(data)
    }

    /// Held POLLs whose screen changed or whose time is up; idle sessions
    /// are closed.
    pub fn due(&mut self, now: Instant) -> Vec<Answer> {
        self.sessions.retain(|node, s| {
            let keep = now.duration_since(s.last_seen) < IDLE_TIMEOUT;
            if !keep {
                eprintln!("teletekst: node {node}: idle, closing the session");
            }
            keep
        });
        let mut answers = Vec::new();
        for mut h in std::mem::take(&mut self.held) {
            let node = h.req.from.node;
            // Compare screens only when output arrived since last time.
            let changed = self.sessions.get(&node).is_none_or(|s| {
                let mark = s.changed.load(Ordering::Relaxed);
                if mark == h.mark {
                    return false;
                }
                h.mark = mark;
                let base = s.sent.as_ref().filter(|(seq, _)| *seq == h.have).map(|(_, sc)| sc);
                base != Some(&s.screen())
            });
            if changed || now >= h.deadline {
                let data = self.update(node, h.have);
                answers.push((h.socket, h.req, data));
            } else {
                self.held.push(h);
            }
        }
        answers
    }
}

/// Rows of `screen` that differ from `base` (all of them without a base).
fn changed_rows(base: Option<&Screen>, screen: &Screen) -> Vec<(u8, Row)> {
    screen
        .iter()
        .enumerate()
        .filter(|(i, row)| base.is_none_or(|b| b.get(*i) != Some(*row)))
        .map(|(i, row)| (i as u8, *row))
        .collect()
}

fn encode(seq: u16, rows: &[(u8, Row)]) -> Vec<u8> {
    let mut d = vec![POLL, OK];
    d.extend(seq.to_be_bytes());
    d.extend([rows.len() as u8, 0]);
    for (i, row) in rows {
        d.push(*i);
        for c in row {
            d.extend([c.ch, c.attr]);
        }
    }
    d
}

/// A response split into ATP packets: all full except the last, so the
/// Mac receives it as one contiguous byte stream.
fn packets(data: Vec<u8>) -> Vec<ResponsePacket> {
    data.chunks(MAX_DATA).map(|c| ResponsePacket { user: 0, data: c.to_vec() }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sextants_map_to_patterns() {
        assert_eq!(mosaic('\u{1FB00}'), Some(1));
        assert_eq!(mosaic('\u{1FB13}'), Some(20));
        assert_eq!(mosaic('\u{1FB14}'), Some(22)); // 21 is ▌
        assert_eq!(mosaic('\u{1FB27}'), Some(41));
        assert_eq!(mosaic('\u{1FB28}'), Some(43)); // 42 is ▐
        assert_eq!(mosaic('\u{1FB3B}'), Some(62));
        assert_eq!(mosaic('▌'), Some(21));
        assert_eq!(mosaic('a'), None);
    }

    #[test]
    fn colours() {
        let mut p = vt100::Parser::new(2, 10, 0);
        // White on blue, yellow on red, default, inverse default.
        p.process(b"\x1b[38;2;255;255;255;48;2;0;0;255mA\x1b[38;2;255;255;0;48;2;255;0;0mB\x1b[0mC\x1b[7mD");
        let s = extract(p.screen());
        assert_eq!((s[0][0].ch, fg(s[0][0].attr), bg(s[0][0].attr)), (b'A', WHITE, 4));
        assert_eq!((s[0][1].ch, fg(s[0][1].attr), bg(s[0][1].attr)), (b'B', 3, 1));
        assert_eq!((fg(s[0][2].attr), bg(s[0][2].attr)), (WHITE, BLACK));
        assert_eq!((fg(s[0][3].attr), bg(s[0][3].attr)), (BLACK, WHITE));
        // An empty cell is white on black, like an untouched terminal.
        assert_eq!(s[1][9], Cell { ch: b' ', attr: WHITE });
    }

    #[test]
    fn mac_characters() {
        assert_eq!(to_mac('ë'), 0x91);
        assert_eq!(to_mac('→'), b'>');
        assert_eq!(to_mac('✓'), b'?');
    }

    #[test]
    fn row_diff_and_encoding() {
        let a = message_screen(&["hallo"]);
        let mut b = a.clone();
        b[3][0].ch = b'x';
        assert_eq!(changed_rows(None, &a).len(), ROWS);
        let rows = changed_rows(Some(&a), &b);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 3);
        let d = encode(7, &rows);
        assert_eq!(&d[..6], &[POLL, OK, 0, 7, 1, 0]);
        assert_eq!(d.len(), 6 + 1 + 2 * COLS);
        // A full screen fits the packets a POLL asks for.
        assert!(encode(1, &changed_rows(None, &a)).len() <= POLL_PACKETS * MAX_DATA);
    }
}
