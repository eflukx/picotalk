//! A [`Stack`] wired to an LToUDP socket and a [`Pacer`]: the event loop
//! every tool in this workspace runs.

use std::io;
use std::net::Ipv4Addr;
use std::time::{Instant, SystemTime};

use crate::ltoudp::{LtoUdp, Pacer};
use crate::stack::{Event, Role, Stack};

#[derive(Clone, Debug)]
pub struct Config {
    pub role: Role,
    /// LLAP node to try first.
    pub node: Option<u8>,
    /// Outgoing bytes per second, 0 = unpaced.
    pub rate: u32,
    /// Interface address for multicast; `0.0.0.0` lets the OS choose.
    pub iface: Ipv4Addr,
}

impl Config {
    pub fn new(role: Role) -> Self {
        Self { role, node: None, rate: DEFAULT_RATE, iface: Ipv4Addr::UNSPECIFIED }
    }

    /// Handles the command-line options every tool shares (`--node`,
    /// `--rate`, `--iface`). Returns `Ok(false)` if `arg` is not one of them.
    pub fn parse_arg<'a>(&mut self, arg: &str, rest: &mut impl Iterator<Item = &'a String>) -> Result<bool, String> {
        let mut val = || rest.next().ok_or(format!("{arg} needs a value"));
        let bad = |e: &dyn std::fmt::Display| format!("{arg}: {e}");
        match arg {
            "--node" => self.node = Some(val()?.parse().map_err(|e| bad(&e))?),
            "--rate" => self.rate = val()?.parse().map_err(|e| bad(&e))?,
            "--iface" => self.iface = val()?.parse().map_err(|e| bad(&e))?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// Help text for the options [`Config::parse_arg`] handles.
pub const OPTIONS_HELP: &str = "\
  --node N        LLAP node to try first (default: random; servers 128-254,
                  workstations 1-127)
  --rate BPS      pace outgoing frames to BPS bytes/s, 0 = unpaced (default 20000)
  --iface IP      address of the network interface for LToUDP multicast
                  (default: chosen by the OS)";

/// A little below LocalTalk's 28.8 KB/s, so a picotalk bridge never has
/// to queue much.
pub const DEFAULT_RATE: u32 = 20_000;

pub struct Node {
    link: LtoUdp,
    pacer: Pacer,
    pub stack: Stack,
}

impl Node {
    pub fn open(cfg: &Config) -> io::Result<Self> {
        let seed = seed();
        let now = Instant::now();
        Ok(Self {
            link: LtoUdp::open(cfg.iface, seed)?,
            pacer: Pacer::new(cfg.rate, now),
            stack: Stack::new(cfg.role, cfg.node, seed, now),
        })
    }

    /// Receives what has arrived (waiting up to a few milliseconds), runs
    /// the stack's timers and sends whatever is due.
    pub fn step(&mut self) -> io::Result<Vec<Event>> {
        let mut events = Vec::new();
        while let Some(frame) = self.link.recv()? {
            events.extend(self.stack.receive(&frame, Instant::now()));
        }
        events.extend(self.stack.poll(Instant::now()));
        self.flush()?;
        Ok(events)
    }

    /// Sends what the stack has queued, as far as the pacer allows.
    pub fn flush(&mut self) -> io::Result<()> {
        while let Some(f) = self.stack.pop_frame() {
            self.pacer.push(f);
        }
        for f in self.pacer.ready(Instant::now()) {
            self.link.send(&f)?;
        }
        Ok(())
    }
}

/// Seed for node addresses, transaction IDs and the LToUDP sender ID.
fn seed() -> u32 {
    let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    (t.as_nanos() as u32) ^ std::process::id().rotate_left(16)
}
