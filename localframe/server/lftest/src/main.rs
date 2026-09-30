//! `lftest echo`: an `LFEcho` server. `lftest ping`: the same tests the
//! Mac's ATPing runs, from a PC.

mod echo;
mod monitor;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use appletalk::atp::ResponsePacket;
use appletalk::node::OPTIONS_HELP;
use appletalk::{Addr, Config, Entity, Event, Node, Role};

use echo::Echo;

const ECHO_SOCKET: u8 = 250;

fn usage() -> String {
    format!(
        "usage: lftest echo [options]    answer lookups for NAME:LFEcho and echo ATP requests
       lftest ping [options]    find LFEcho servers and test the first one
       lftest monitor [--iface IP]
                                print every LToUDP frame on the network, decoded

options:
  --name NAME     NBP object name (echo; default: host name)
  -v              log every request (echo)
{OPTIONS_HELP}"
    )
}

struct Options {
    cfg: Config,
    name: String,
    verbose: bool,
}

fn parse_args(role: Role, args: &[String]) -> Result<Options, String> {
    let mut o = Options { cfg: Config::new(role), name: hostname(), verbose: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" => o.name = it.next().ok_or("--name needs a value")?.clone(),
            "-v" => o.verbose = true,
            _ if o.cfg.parse_arg(a, &mut it)? => {}
            _ => return Err(format!("unknown option {a}\n\n{}", usage())),
        }
    }
    if o.name.is_empty() || o.name.len() > 32 {
        return Err("--name must be 1 to 32 characters".into());
    }
    Ok(o)
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().chars().take(32).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "LocalFrame".into())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("echo") => parse_args(Role::Server, &args[1..]).and_then(|o| serve(o).map_err(|e| e.to_string())),
        Some("monitor") => parse_args(Role::Workstation, &args[1..])
            .and_then(|o| monitor::run(o.cfg.iface).map_err(|e| e.to_string())),
        Some("ping") => parse_args(Role::Workstation, &args[1..]).and_then(|o| ping(o).map_err(|e| e.to_string())),
        _ => Err(usage()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn serve(o: Options) -> std::io::Result<()> {
    let mut node = Node::open(&o.cfg)?;
    let entity = Entity::new(&o.name, echo::TYPE);
    node.stack.register(entity.clone(), ECHO_SOCKET);
    let mut echo = Echo::default();
    eprintln!("joined LToUDP, acquiring a node address…");
    loop {
        for ev in node.step()? {
            match ev {
                Event::Ready(n) => eprintln!("node {n}: serving {entity} on socket {ECHO_SOCKET}"),
                Event::LookedUp { from, pattern } => eprintln!("lookup for {pattern} from {from}"),
                Event::Request { socket, req } => {
                    let response = echo.handle(&req, node.stack.node().unwrap_or(0));
                    if o.verbose || req.data.first() != Some(&echo::PING) {
                        eprintln!(
                            "{}: cmd {:02x}, {} bytes in, {} packets out",
                            req.from,
                            req.data.first().copied().unwrap_or(0),
                            req.data.len(),
                            response.len()
                        );
                    }
                    node.stack.respond(socket, &req, response, Instant::now());
                    node.flush()?;
                }
                _ => {}
            }
        }
    }
}

/// Runs the node until `until` is true of an event, or `timeout` passes.
fn wait_for<T>(node: &mut Node, timeout: Duration, mut until: impl FnMut(Event) -> Option<T>) -> std::io::Result<Option<T>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        for ev in node.step()? {
            if let Some(t) = until(ev) {
                return Ok(Some(t));
            }
        }
    }
    Ok(None)
}

/// One exactly-once transaction; `None` if it failed.
fn transact(node: &mut Node, to: Addr, data: &[u8], packets: usize) -> std::io::Result<Option<Vec<ResponsePacket>>> {
    let tid = node.stack.request(to, data, 0x5EED, packets, Instant::now()).expect("node is ready");
    wait_for(node, Duration::from_secs(30), |ev| match ev {
        Event::Response { tid: t, packets } if t == tid => Some(Some(packets)),
        Event::RequestFailed { tid: t } if t == tid => Some(None),
        _ => None,
    })
    .map(Option::flatten)
}

fn ping(o: Options) -> std::io::Result<()> {
    let mut node = Node::open(&o.cfg)?;
    let Some(me) = wait_for(&mut node, Duration::from_secs(2), |ev| match ev {
        Event::Ready(n) => Some(n),
        _ => None,
    })?
    else {
        println!("could not get a node address");
        return Ok(());
    };
    println!("we are node {me}");

    let mut server = None;
    for _ in 0..3 {
        let id = node.stack.lookup("=", echo::TYPE).expect("node is ready");
        server = wait_for(&mut node, Duration::from_millis(700), |ev| match ev {
            Event::Found { id: i, addr, entity } if i == id => Some((addr, entity)),
            _ => None,
        })?;
        if server.is_some() {
            break;
        }
    }
    let Some((server, entity)) = server else {
        println!("no {} server found", echo::TYPE);
        return Ok(());
    };
    println!("found {entity} at {server}");

    let mut ok = 0;
    for seq in 0..10u16 {
        let len = if seq % 3 == 2 { 566 } else { 32 };
        let payload: Vec<u8> = (0..len).map(|i| (i ^ seq as usize) as u8).collect();
        let mut req = vec![echo::PING, 0];
        req.extend(seq.to_be_bytes());
        req.extend(&payload);
        let t = Instant::now();
        match transact(&mut node, server, &req, 1)? {
            Some(p) if p[0].data.get(12..) == Some(&payload[..]) && p[0].data[2..4] == seq.to_be_bytes() => {
                ok += 1;
                let d = &p[0].data;
                println!(
                    "ping {seq}: {len} bytes, {:.1} ms, server sees us as {}.{}",
                    t.elapsed().as_secs_f64() * 1e3,
                    u16::from_be_bytes([d[4], d[5]]),
                    d[6]
                );
            }
            Some(_) => println!("ping {seq}: WRONG REPLY"),
            None => println!("ping {seq}: no answer"),
        }
    }

    let t = Instant::now();
    let (mut bytes, mut bad, mut failed) = (0, 0, 0);
    for seq in 0..20u16 {
        match transact(&mut node, server, &[echo::BULK, 0, (seq >> 8) as u8, seq as u8], 8)? {
            Some(packets) => {
                for (i, p) in packets.iter().enumerate() {
                    bytes += p.data.len();
                    bad += (p.data != echo::bulk_packet(seq, i as u8)) as u32;
                }
            }
            None => failed += 1,
        }
    }
    let secs = t.elapsed().as_secs_f64();
    println!("pings: {ok}/10 ok");
    println!(
        "bulk: {bytes} bytes in {secs:.2} s = {:.1} KB/s, {bad} bad packets, {failed} failed transactions",
        bytes as f64 / secs / 1000.0
    );
    Ok(())
}
