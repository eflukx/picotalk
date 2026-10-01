//! The `Tanks` service: a tank-level dashboard for classic Macs.
//!
//! A web dashboard serves two JSON documents, `api/levels` (the tanks) and
//! `api/weather`, below its address. That address carries an access token,
//! so it is never in the code, the logs or the docs: `serve` reads it from
//! the environment variable [`URL_VAR`] and offers this service only when
//! it is set. Error messages never include it.
//!
//! A thread fetches both documents every [`FETCH_EVERY`] and encodes them
//! into one snapshot, ready to draw. The Mac long-polls for it, as the chat
//! does for messages:
//!
//! ```text
//! POLL  01 00 have(2)  → 01 st version(2) snapshot…
//! ```
//!
//! `version` numbers the snapshots; a POLL for an older one is answered at
//! once, otherwise it is held until a new one arrives or for [`HOLD`] (then
//! answered with the same version and no snapshot). The snapshot, with
//! strings as Pascal strings in Mac Roman and numbers big-endian:
//!
//! ```text
//! ntanks
//! { name, status(1: 0 stable, 1 filling, 2 emptying, 3 other), status text,
//!   level (0.1 %, 2 bytes),
//!   today filled, today emptied, today level,          (strings: "9.688")
//!   yesterday filled, yesterday emptied, yesterday level } × ntanks
//! weather?(1) [ temperature, summary, feels-like line ]  (strings)
//! updated  ("1-10-2026, 12:40:09")
//! problem  (empty, or why the data is old or missing)
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use appletalk::atp::{MAX_DATA, Request, ResponsePacket};
use chrono::DateTime;
use chrono_tz::{Europe::Amsterdam, Tz};
use serde::Deserialize;

use crate::chat::{pstr, to_mac};

pub const TYPE: &str = "Tanks";
/// The environment variable holding the dashboard's address.
pub const URL_VAR: &str = "BLD_URL";

pub const POLL: u8 = 1;
pub const OK: u8 = 0;
/// Response packets a client should allow a POLL.
pub const POLL_PACKETS: usize = 2;

/// How often the dashboard is asked; its own page asks every 5 s.
const FETCH_EVERY: Duration = Duration::from_secs(5);
/// How long a POLL waits for new data; clients allow 4 s.
pub const HOLD: Duration = Duration::from_secs(2);
/// Failed fetches in a row before the Mac is told the data is old.
const FAILURES_SHOWN: u32 = 3;

#[derive(Deserialize)]
struct Totals {
    filled: f64,
    emptied: f64,
    level: f64,
}

#[derive(Deserialize)]
struct Tank {
    name: String,
    level_pct: f64,
    status: String,
    timestamp: Option<String>,
    today_totals: Totals,
    yesterday_totals: Totals,
}

#[derive(Deserialize)]
struct Weather {
    location: String,
    temperature: f64,
    feels_like: f64,
    summary: String,
    time: String,
}

/// The latest snapshot and its version.
#[derive(Default)]
struct Latest {
    version: u16,
    snapshot: Vec<u8>,
}

struct Held {
    socket: u8,
    req: Request,
    have: u16,
    deadline: Instant,
}

pub struct Tanks {
    latest: Arc<Mutex<Latest>>,
    held: Vec<Held>,
}

impl Tanks {
    /// Starts fetching from `url` (the dashboard's address) in the
    /// background.
    pub fn start(url: String) -> Self {
        let latest = Arc::new(Mutex::new(Latest::default()));
        let shared = latest.clone();
        std::thread::spawn(move || fetch_loop(&url, &shared));
        Self { latest, held: Vec::new() }
    }

    pub fn handle(&mut self, socket: u8, req: Request, now: Instant) -> Vec<(u8, Request, Vec<ResponsePacket>)> {
        let cmd = req.data.first().copied().unwrap_or(0);
        if cmd != POLL {
            return vec![(socket, req, vec![ResponsePacket { user: 0, data: vec![cmd, 0xFF] }])];
        }
        let have = u16::from_be_bytes([req.data.get(2).copied().unwrap_or(0), req.data.get(3).copied().unwrap_or(0)]);
        let latest = self.latest.lock().unwrap();
        if latest.version != have && latest.version != 0 {
            let answer = reply(&latest, true);
            return vec![(socket, req, answer)];
        }
        drop(latest);
        self.held.push(Held { socket, req, have, deadline: now + HOLD });
        Vec::new()
    }

    /// Held POLLs with new data, or whose time is up.
    pub fn due(&mut self, now: Instant) -> Vec<(u8, Request, Vec<ResponsePacket>)> {
        let latest = self.latest.lock().unwrap();
        let mut answers = Vec::new();
        for h in std::mem::take(&mut self.held) {
            let new = latest.version != h.have && latest.version != 0;
            if new || now >= h.deadline {
                answers.push((h.socket, h.req, reply(&latest, new)));
            } else {
                self.held.push(h);
            }
        }
        answers
    }
}

fn reply(latest: &Latest, with_snapshot: bool) -> Vec<ResponsePacket> {
    let mut d = vec![POLL, OK];
    d.extend(latest.version.to_be_bytes());
    if with_snapshot {
        d.extend(&latest.snapshot);
    }
    d.chunks(MAX_DATA).map(|c| ResponsePacket { user: 0, data: c.to_vec() }).collect()
}

fn fetch_loop(url: &str, latest: &Mutex<Latest>) {
    let base = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let mut tanks: Option<Vec<Tank>> = None;
    let mut weather: Option<Weather> = None;
    let mut failures = 0u32;
    let mut last_problem = String::new();
    loop {
        match get::<Vec<Tank>>(&agent, &base, "api/levels") {
            Ok(t) => {
                tanks = Some(t);
                failures = 0;
            }
            Err(e) => {
                failures += 1;
                if e != last_problem {
                    eprintln!("tanks: levels: {e}");
                }
                last_problem = e;
            }
        }
        if let Ok(w) = get::<Weather>(&agent, &base, "api/weather") {
            weather = Some(w);
        }
        let problem = match (&tanks, failures) {
            (None, _) => "Wachten op data...".to_string(),
            (Some(_), n) if n >= FAILURES_SHOWN => "Geen verbinding met de tankserver; gegevens zijn oud.".into(),
            _ => String::new(),
        };
        let snapshot = encode(tanks.as_deref().unwrap_or(&[]), weather.as_ref(), &problem);
        {
            let mut l = latest.lock().unwrap();
            if l.snapshot != snapshot || l.version == 0 {
                l.version = l.version.wrapping_add(1).max(1);
                l.snapshot = snapshot;
            }
        }
        std::thread::sleep(FETCH_EVERY);
    }
}

/// Fetches and parses `base` + `path`. The error says what went wrong
/// without the address, which holds the access token.
fn get<T: for<'de> Deserialize<'de>>(agent: &ureq::Agent, base: &str, path: &str) -> Result<T, String> {
    match agent.get(&format!("{base}{path}")).call() {
        Ok(r) => r
            .into_string()
            .ok()
            .and_then(|body| serde_json::from_str(&body).ok())
            .ok_or_else(|| "unexpected data from the dashboard".to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("dashboard answered HTTP {code}")),
        Err(ureq::Error::Transport(t)) => Err(format!("cannot reach the dashboard ({})", t.kind())),
    }
}

fn encode(tanks: &[Tank], weather: Option<&Weather>, problem: &str) -> Vec<u8> {
    let mut d = vec![tanks.len() as u8];
    for t in tanks {
        let (code, text) = status(&t.status);
        pstr(&mut d, &to_mac(&t.name));
        d.push(code);
        pstr(&mut d, &to_mac(text));
        d.extend(((t.level_pct.clamp(0.0, 100.0) * 10.0).round() as u16).to_be_bytes());
        for v in [
            t.today_totals.filled,
            t.today_totals.emptied,
            t.today_totals.level,
            t.yesterday_totals.filled,
            t.yesterday_totals.emptied,
            t.yesterday_totals.level,
        ] {
            pstr(&mut d, litres(v).as_bytes());
        }
    }
    match weather {
        Some(w) => {
            d.push(1);
            pstr(&mut d, &to_mac(&format!("{:.1}°C", w.temperature)));
            pstr(&mut d, &to_mac(&w.summary));
            let time = local(&w.time).map(|t| t.format(" (%H:%M)").to_string()).unwrap_or_default();
            pstr(&mut d, &to_mac(&format!("Gevoel: {:.1}°C | {}{time}", w.feels_like, w.location)));
        }
        None => d.push(0),
    }
    let updated = tanks
        .iter()
        .filter_map(|t| t.timestamp.as_deref().and_then(local))
        .max()
        .map(|t| t.format("%-d-%-m-%Y, %H:%M:%S").to_string())
        .unwrap_or_default();
    pstr(&mut d, updated.as_bytes());
    pstr(&mut d, &to_mac(problem));
    d
}

/// The dashboard's status, as a code for the Mac's arrows and as the text
/// its own page shows.
fn status(s: &str) -> (u8, &str) {
    match s {
        "Stable" | "Stabiel" | "Niveau stabiel" => (0, "Niveau stabiel"),
        "Filling" | "Vullen" | "Laden" => (1, "Laden"),
        "Emptying" | "Legen" | "Lossen" => (2, "Lossen"),
        other => (3, other),
    }
}

/// Litres, rounded, with a dot between thousands: 68312.5 → "68.313".
fn litres(v: f64) -> String {
    let n = v.round().max(0.0) as u64;
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// A dashboard time (UTC) in Dutch time, as the dashboard's page shows it.
fn local(ts: &str) -> Option<DateTime<Tz>> {
    DateTime::parse_from_rfc3339(ts).ok().map(|t| t.with_timezone(&Amsterdam))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn litres_have_thousands_dots() {
        assert_eq!(litres(0.0), "0");
        assert_eq!(litres(9687.5), "9.688");
        assert_eq!(litres(68312.5), "68.313");
        assert_eq!(litres(1234567.0), "1.234.567");
    }

    #[test]
    fn statuses() {
        assert_eq!(status("Filling"), (1, "Laden"));
        assert_eq!(status("Lossen"), (2, "Lossen"));
        assert_eq!(status("Stable"), (0, "Niveau stabiel"));
        assert_eq!(status("Onderhoud"), (3, "Onderhoud"));
    }
}
