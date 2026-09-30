//! Name Binding Protocol: answering lookups for our registered names.
//!
//! ```text
//! packet: function(4 bits) | tuple count(4 bits), NBP ID, tuples…
//! tuple:  net(2) node socket enumerator, object, type, zone (Pascal strings)
//! ```
//!
//! On a network without a router a Mac broadcasts `LkUp` to socket 2 of
//! every node; the tuple is the address to send the `LkUp-Reply` to.

use crate::ddp::Addr;

pub const BRRQ: u8 = 1;
pub const LKUP: u8 = 2;
pub const LKUP_REPLY: u8 = 3;

/// "≈" in Mac Roman: matches any run of characters (AppleTalk Phase 2).
const WILD: u8 = 0xC5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    pub object: Vec<u8>,
    pub kind: Vec<u8>,
    pub zone: Vec<u8>,
}

impl Entity {
    pub fn new(object: &str, kind: &str) -> Self {
        Self { object: object.as_bytes().to_vec(), kind: kind.as_bytes().to_vec(), zone: b"*".to_vec() }
    }
}

impl core::fmt::Display for Entity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        write!(f, "{}:{}@{}", s(&self.object), s(&self.kind), s(&self.zone))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Lookup {
    pub id: u8,
    /// Where to send the reply.
    pub reply_to: Addr,
    pub pattern: Entity,
}

/// Parses a `LkUp` or `BrRq` packet.
pub fn parse_lookup(data: &[u8]) -> Option<Lookup> {
    let function = data.first()? >> 4;
    if function != LKUP && function != BRRQ || data[0] & 0x0F != 1 {
        return None;
    }
    let id = *data.get(1)?;
    let t = data.get(2..7)?;
    let reply_to = Addr { net: u16::from_be_bytes([t[0], t[1]]), node: t[2], socket: t[3] };
    let mut rest = &data[7..];
    let mut take = || {
        let (&n, tail) = rest.split_first()?;
        let s = tail.get(..n as usize)?.to_vec();
        rest = &tail[n as usize..];
        Some(s)
    };
    let pattern = Entity { object: take()?, kind: take()?, zone: take()? };
    Some(Lookup { id, reply_to, pattern })
}

/// Builds a one-tuple `LkUp-Reply`.
pub fn reply(id: u8, addr: Addr, entity: &Entity) -> Vec<u8> {
    let mut p = vec![(LKUP_REPLY << 4) | 1, id];
    p.extend(addr.net.to_be_bytes());
    p.extend([addr.node, addr.socket, 0]);
    for s in [&entity.object, &entity.kind, &entity.zone] {
        p.push(s.len() as u8);
        p.extend(s);
    }
    p
}

/// Builds a `LkUp` for `pattern`, asking for replies at `reply_to`.
pub fn lookup(id: u8, reply_to: Addr, pattern: &Entity) -> Vec<u8> {
    let mut p = vec![(LKUP << 4) | 1, id];
    p.extend(reply_to.net.to_be_bytes());
    p.extend([reply_to.node, reply_to.socket, 0]);
    for s in [&pattern.object, &pattern.kind, &pattern.zone] {
        p.push(s.len() as u8);
        p.extend(s);
    }
    p
}

/// Parses a `LkUp-Reply`: the NBP ID and the (address, entity) tuples.
pub fn parse_reply(data: &[u8]) -> Option<(u8, Vec<(Addr, Entity)>)> {
    if data.first()? >> 4 != LKUP_REPLY {
        return None;
    }
    let (count, id) = (data[0] & 0x0F, *data.get(1)?);
    let mut rest = &data[2..];
    let mut tuples = Vec::new();
    for _ in 0..count {
        let t = rest.get(..5)?;
        let addr = Addr { net: u16::from_be_bytes([t[0], t[1]]), node: t[2], socket: t[3] };
        rest = &rest[5..];
        let mut take = || {
            let (&n, tail) = rest.split_first()?;
            let s = tail.get(..n as usize)?.to_vec();
            rest = &tail[n as usize..];
            Some(s)
        };
        tuples.push((addr, Entity { object: take()?, kind: take()?, zone: take()? }));
    }
    Some((id, tuples))
}

/// Does `entity` match the lookup `pattern`? Zones are not checked: this
/// node answers for whatever zone the lookup was sent in.
pub fn matches(pattern: &Entity, entity: &Entity) -> bool {
    field_matches(&pattern.object, &entity.object) && field_matches(&pattern.kind, &entity.kind)
}

fn field_matches(pattern: &[u8], name: &[u8]) -> bool {
    if pattern == b"=" {
        return true;
    }
    let eq = |a: &[u8], b: &[u8]| a.eq_ignore_ascii_case(b);
    match pattern.iter().position(|&c| c == WILD) {
        None => eq(pattern, name),
        Some(i) => {
            let (prefix, suffix) = (&pattern[..i], &pattern[i + 1..]);
            name.len() >= prefix.len() + suffix.len()
                && eq(prefix, &name[..prefix.len()])
                && eq(suffix, &name[name.len() - suffix.len()..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(obj: &[u8], kind: &[u8]) -> Vec<u8> {
        let mut p = vec![0x21, 9, 0, 0, 12, 0xFD, 0];
        for s in [obj, kind, b"*"] {
            p.push(s.len() as u8);
            p.extend(s);
        }
        p
    }

    #[test]
    fn parse_and_match() {
        let l = parse_lookup(&lookup(b"=", b"lfecho")).unwrap();
        assert_eq!(l.id, 9);
        assert_eq!(l.reply_to, Addr { net: 0, node: 12, socket: 0xFD });
        let me = Entity::new("pc", "LFEcho");
        assert!(matches(&l.pattern, &me));
        assert!(!matches(&parse_lookup(&lookup(b"=", b"LaserWriter")).unwrap().pattern, &me));
        assert!(matches(&parse_lookup(&lookup(b"p\xC5", b"LF\xC5o")).unwrap().pattern, &me));
        assert!(!matches(&parse_lookup(&lookup(b"x\xC5", b"=")).unwrap().pattern, &me));
    }

    #[test]
    fn rejects_other_functions_and_truncation() {
        let mut p = lookup(b"=", b"=");
        p[0] = 0x31;
        assert_eq!(parse_lookup(&p), None);
        let p = lookup(b"=", b"=");
        assert_eq!(parse_lookup(&p[..p.len() - 1]), None);
    }

    #[test]
    fn reply_layout() {
        let r = reply(9, Addr { net: 0, node: 200, socket: 250 }, &Entity::new("pc", "LFEcho"));
        assert_eq!(r, b"\x31\x09\x00\x00\xC8\xFA\x00\x02pc\x06LFEcho\x01*");
        let (id, tuples) = parse_reply(&r).unwrap();
        assert_eq!(id, 9);
        assert_eq!(tuples, vec![(Addr { net: 0, node: 200, socket: 250 }, Entity::new("pc", "LFEcho"))]);
        assert_eq!(parse_reply(&r[..r.len() - 1]), None);
    }

    #[test]
    fn lookup_roundtrip() {
        let me = Addr { net: 0, node: 5, socket: 2 };
        let l = parse_lookup(&super::lookup(4, me, &Entity::new("=", "RChat"))).unwrap();
        assert_eq!((l.id, l.reply_to), (4, me));
        assert!(matches(&l.pattern, &Entity::new("anything", "rchat")));
    }
}
