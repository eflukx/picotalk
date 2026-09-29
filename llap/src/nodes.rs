//! Node freshness table, as used by AirTalk's LToUDP bridge.
//!
//! A bridge keeps two of these:
//! * nodes seen *on LocalTalk*: only LToUDP frames addressed to one of these
//!   (or broadcast) are worth putting on the wire;
//! * nodes seen *on LToUDP*: the link must answer RTS/ENQ on their behalf,
//!   because their own replies could never arrive within the 200 µs
//!   inter-frame gap.

#[derive(Clone)]
pub struct NodeTable {
    last_seen: [Option<u32>; 256],
}

impl Default for NodeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeTable {
    pub const fn new() -> Self {
        Self { last_seen: [None; 256] }
    }

    /// Marks `node` as alive at `now` (seconds). 0 and 255 are not node IDs.
    pub fn touch(&mut self, node: u8, now: u32) {
        if node != 0 && node != 255 {
            self.last_seen[node as usize] = Some(now);
        }
    }

    /// True if `node` was seen less than `max_age` seconds ago.
    pub fn fresh(&self, node: u8, now: u32, max_age: u32) -> bool {
        self.last_seen[node as usize].is_some_and(|t| now.saturating_sub(t) < max_age)
    }

    /// Bitmap of fresh nodes in TashTalk's "set node IDs" layout.
    pub fn bitmap(&self, now: u32, max_age: u32) -> [u8; 32] {
        let mut bits = [0u8; 32];
        for node in 0..=255u8 {
            if self.fresh(node, now, max_age) {
                bits[node as usize / 8] |= 1 << (node % 8);
            }
        }
        bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_and_bitmap() {
        let mut t = NodeTable::new();
        t.touch(9, 100);
        t.touch(255, 100);
        assert!(t.fresh(9, 150, 60));
        assert!(!t.fresh(9, 160, 60));
        assert!(!t.fresh(255, 100, 60));
        let bits = t.bitmap(120, 60);
        assert_eq!(bits[1], 0b10);
        assert_eq!(bits.iter().map(|b| b.count_ones()).sum::<u32>(), 1);
    }
}
