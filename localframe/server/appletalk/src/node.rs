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
            "--rate" => {
                self.rate = val()?.parse().map_err(|e| bad(&e))?;
                if self.rate == 0 || self.rate > MAX_USEFUL_RATE {
                    eprintln!(
                        "warning: --rate {} is well above LocalTalk's {WIRE_RATE} bytes/s. It gains nothing, \
                         and once the receiving queue fills, frames are lost and each loss costs a \
                         2-second ATP retry: throughput goes down, not up.",
                        self.rate
                    );
                }
            }
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
  --rate BPS      pace outgoing frames to BPS bytes/s (default 30000); much
                  above LocalTalk's 28800, or 0 (unpaced), gains nothing and
                  risks lost frames
  --iface IP      address of the network interface for LToUDP multicast
                  (default: chosen by the OS)";

/// LocalTalk's raw speed: 230.4 kbit/s. Sending faster gains nothing:
/// frames queue up (Snow emulates the serial chip at this rate and buffers
/// a few), and once a queue is full they are lost, each loss costing an
/// ATP retry timeout.
pub const WIRE_RATE: u32 = 28_800;

/// Just above [`WIRE_RATE`]: the link runs flat out, and a burst queues by
/// only a few percent. Measured in Snow (Mac Plus at 1×): 15000 gives
/// 10.5 KB/s of bulk data, 28000 and 38000 both give 18.4 KB/s, 100000
/// collapses to 2.7 KB/s.
pub const DEFAULT_RATE: u32 = 30_000;

/// Above this `--rate` warns: Snow already lost frames at 100000.
const MAX_USEFUL_RATE: u32 = 40_000;

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
