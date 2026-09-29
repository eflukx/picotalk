//! LLAP frame layout.
//!
//! ```text
//! control frame: dest src type(0x80..) fcs fcs                      (5 bytes)
//! data frame:    dest src type(<0x80) len_hi len_lo payload… fcs fcs
//! ```
//!
//! The 10-bit length field counts itself plus the payload, but not the
//! three header bytes or the FCS.

pub const LLAP_ENQ: u8 = 0x81;
pub const LLAP_ACK: u8 = 0x82;
pub const LLAP_RTS: u8 = 0x84;
pub const LLAP_CTS: u8 = 0x85;

pub const BROADCAST: u8 = 0xFF;

pub const CONTROL_FRAME_LEN: usize = 5;

/// Largest frame the 10-bit length field can describe (LLAP itself caps the
/// length at 600, but TashTalk does not enforce that and neither do we).
pub const MAX_FRAME: usize = 3 + 0x3FF + 2;

pub fn is_control(llap_type: u8) -> bool {
    llap_type & 0x80 != 0
}

/// Total frame length (including FCS) given the first five bytes.
pub fn total_len(first5: &[u8]) -> usize {
    if is_control(first5[2]) {
        CONTROL_FRAME_LEN
    } else {
        let len = ((first5[3] as usize & 0x03) << 8) | first5[4] as usize;
        // Bytes following the first five are len - 2 payload bytes plus the
        // two FCS bytes.
        CONTROL_FRAME_LEN + len
    }
}

/// A frame including its two FCS bytes.
#[derive(Clone)]
pub struct Frame {
    len: u16,
    data: [u8; MAX_FRAME],
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for Frame {}

impl core::fmt::Debug for Frame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Frame({:02x?})", self.as_slice())
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for Frame {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(f, "Frame({=[u8]:02x})", self.as_slice())
    }
}

impl Frame {
    pub const fn new() -> Self {
        Self { len: 0, data: [0; MAX_FRAME] }
    }

    /// Copies `bytes` into a new frame, or `None` if they do not fit.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        let mut f = Self::new();
        f.set(bytes).then_some(f)
    }

    pub fn set(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > MAX_FRAME {
            return false;
        }
        self.data[..bytes.len()].copy_from_slice(bytes);
        self.len = bytes.len() as u16;
        true
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn push(&mut self, byte: u8) -> bool {
        if self.len as usize >= MAX_FRAME {
            return false;
        }
        self.data[self.len as usize] = byte;
        self.len += 1;
        true
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[..self.len as usize]
    }

    pub fn dest(&self) -> u8 {
        self.data[0]
    }

    pub fn src(&self) -> u8 {
        self.data[1]
    }

    pub fn llap_type(&self) -> u8 {
        self.data[2]
    }

    pub fn is_control(&self) -> bool {
        is_control(self.llap_type())
    }

    /// Overwrites the last two bytes with the correct FCS.
    pub fn fill_fcs(&mut self) {
        let n = self.len();
        if n >= 2 {
            let fcs = crate::crc::fcs(&self.data[..n - 2]);
            self.data[n - 2..n].copy_from_slice(&fcs);
        }
    }
}

/// Builds a control frame (with FCS).
pub fn control_frame(dest: u8, src: u8, llap_type: u8) -> [u8; CONTROL_FRAME_LEN] {
    let fcs = crate::crc::fcs(&[dest, src, llap_type]);
    [dest, src, llap_type, fcs[0], fcs[1]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(total_len(&[1, 2, LLAP_RTS, 0, 0]), 5);
        // Data frame with 10 payload bytes: length field = 12.
        assert_eq!(total_len(&[1, 2, 0x01, 0x00, 12]), 17);
        assert_eq!(total_len(&[1, 2, 0x01, 0xFE, 0x58]), 5 + 600);
    }
}
