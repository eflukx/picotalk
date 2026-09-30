//! `lftest ping`: from a PC, the same tests the Mac's LFTest runs in its
//! Echo Test mode.

use std::time::{Duration, Instant};

use appletalk::atp::ResponsePacket;
use appletalk::{Addr, Event, Node};

use crate::Options;
use crate::echo;

/// Runs the node until `until` is true of an event, or `timeout` passes.
pub fn wait_for<T>(
    node: &mut Node,
    timeout: Duration,
    mut until: impl FnMut(Event) -> Option<T>,
) -> std::io::Result<Option<T>> {
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

pub fn run(o: Options) -> std::io::Result<()> {
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
