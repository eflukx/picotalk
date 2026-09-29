//! LLAP frame check sequence: CRC-CCITT, reflected (poly 0x8408), initial
//! value 0xFFFF, complemented on output and sent low byte first. This is the
//! same CRC as CRC-16/IBM-SDLC (a.k.a. X.25).

/// Register value left after running a whole frame, including its correct
/// FCS, through the CRC.
pub const RESIDUE: u16 = 0xF0B8;

const TABLE: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u16;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crc(u16);

impl Default for Crc {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc {
    pub const fn new() -> Self {
        Self(0xFFFF)
    }

    #[inline]
    pub fn update(&mut self, byte: u8) {
        self.0 = (self.0 >> 8) ^ TABLE[((self.0 ^ byte as u16) & 0xFF) as usize];
    }

    pub fn update_all(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.update(b);
        }
    }

    /// Raw register value.
    pub fn register(&self) -> u16 {
        self.0
    }

    /// True if the bytes fed so far were a frame followed by its correct FCS.
    pub fn residue_ok(&self) -> bool {
        self.0 == RESIDUE
    }

    /// The two FCS bytes to append, in transmission order.
    pub fn fcs(&self) -> [u8; 2] {
        (!self.0).to_le_bytes()
    }
}

/// FCS bytes for `data`, in transmission order.
pub fn fcs(data: &[u8]) -> [u8; 2] {
    let mut crc = Crc::new();
    crc.update_all(data);
    crc.fcs()
}

/// True if `frame` ends in a correct FCS.
pub fn check(frame: &[u8]) -> bool {
    let mut crc = Crc::new();
    crc.update_all(frame);
    crc.residue_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x25_check_value() {
        let mut crc = Crc::new();
        crc.update_all(b"123456789");
        assert_eq!(!crc.register(), 0x906E);
    }

    #[test]
    fn matches_tashtalk_lookup_tables() {
        // TashTalk splits the table into low (CrcLut1) and high (CrcLut2) bytes.
        assert_eq!(TABLE[1], 0x1189);
        assert_eq!(TABLE[0x10], 0x1081);
        assert_eq!(TABLE[0xFF], 0x0F78);
    }

    #[test]
    fn residue_after_fcs() {
        let mut frame = [0x02u8, 0x01, 0x84, 0, 0];
        let f = fcs(&frame[..3]);
        frame[3..].copy_from_slice(&f);
        assert!(check(&frame));
        frame[0] ^= 0x10;
        assert!(!check(&frame));
    }
}
