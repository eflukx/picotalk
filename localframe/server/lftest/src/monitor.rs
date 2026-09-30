//! `lftest monitor`: prints every LToUDP datagram on the network, decoded.
//! Joins the group like any node but never sends.

use std::net::Ipv4Addr;
use std::time::Instant;

use appletalk::atp;
use appletalk::ddp::{self, Llap};
use appletalk::ltoudp::{HEADER_LEN, LtoUdp};
use appletalk::nbp;

pub fn run(iface: Ipv4Addr) -> std::io::Result<()> {
    // Sender ID 0 is never used by a real node, so we see everything.
    let link = LtoUdp::open(iface, 0)?;
    let start = Instant::now();
    eprintln!("listening on 239.192.76.84:1954; Ctrl-C to stop");
    loop {
        let Some((d, from)) = link.recv_raw()? else { continue };
        let t = start.elapsed().as_secs_f64();
        if d.len() <= HEADER_LEN {
            println!("{t:8.3} {from:<21} runt datagram, {} bytes", d.len());
            continue;
        }
        let id = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
        println!("{t:8.3} {from:<21} id {id:08x}  {}", describe(&d[HEADER_LEN..]));
    }
}

fn describe(frame: &[u8]) -> String {
    let Some(llap) = ddp::parse(frame) else {
        return format!("malformed LLAP frame: {frame:02x?}");
    };
    let (dst, src) = (frame[0], frame[1]);
    match llap {
        Llap::Enq { node } => format!("LLAP ENQ  for node {node}"),
        Llap::Ack { node } => format!("LLAP ACK  node {node} is taken"),
        Llap::OtherControl => format!("LLAP control {:#04x}  {src} → {dst}", frame[2]),
        Llap::Ddp(d) => {
            let hdr = format!(
                "DDP{} {} → {}",
                if d.long { "(long)" } else { "" },
                d.src,
                if d.dst.node == 0xFF { format!("broadcast:{}", d.dst.socket) } else { d.dst.to_string() }
            );
            let body = match d.ddp_type {
                ddp::TYPE_NBP => describe_nbp(&d.data),
                ddp::TYPE_ATP => describe_atp(&d.data),
                t => format!("DDP type {t}, {} bytes", d.data.len()),
            };
            format!("{hdr}  {body}")
        }
    }
}

fn describe_nbp(data: &[u8]) -> String {
    if let Some(l) = nbp::parse_lookup(data) {
        return format!("NBP lookup {} (id {}, reply to {})", l.pattern, l.id, l.reply_to);
    }
    if let Some((id, tuples)) = nbp::parse_reply(data) {
        let list: Vec<String> = tuples.iter().map(|(a, e)| format!("{e} at {a}")).collect();
        return format!("NBP reply (id {id}): {}", list.join(", "));
    }
    format!("NBP function {}, {} bytes", data.first().map_or(0, |b| b >> 4), data.len())
}

fn describe_atp(data: &[u8]) -> String {
    let Some(p) = atp::Packet::parse(data) else { return "ATP, truncated".into() };
    let xo = if p.control & 0x20 != 0 { " XO" } else { "" };
    let eom = if p.control & 0x10 != 0 { " EOM" } else { "" };
    match p.control >> 6 {
        1 => format!("ATP TReq{xo}  tid {:#06x} bitmap {:08b}, {} bytes", p.tid, p.bitmap, p.data.len()),
        2 => format!("ATP TResp{eom} tid {:#06x} seq {}, {} bytes", p.tid, p.bitmap, p.data.len()),
        3 => format!("ATP TRel  tid {:#06x}", p.tid),
        _ => format!("ATP control {:#04x}", p.control),
    }
}
