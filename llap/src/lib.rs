//! LocalTalk link layer, hardware independent.
//!
//! The pipeline mirrors the one TashTalk implements in PIC assembly:
//!
//! ```text
//!  receive:  line samples ─► fm0::Decoder ─► hdlc::Deframer ─► mac::Mac / host
//!  transmit: frame bytes  ─► hdlc::encode_frame ─► fm0::SymbolWriter ─► line symbols
//! ```
//!
//! Everything here is `no_std`, allocation free and free of timing
//! assumptions other than the sample rate, so it can be tested on a host.

#![cfg_attr(not(test), no_std)]

pub mod crc;
pub mod fm0;
pub mod frame;
pub mod hdlc;
pub mod ltoudp;
pub mod mac;
pub mod nodes;
pub mod tashtalk;

pub use frame::Frame;
