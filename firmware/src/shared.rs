//! Queues between the host side (core 0) and the link (core 1).
//!
//! Core 1 only ever uses the non-blocking `try_*` calls, so it never waits
//! on core 0 for longer than a critical section.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use llap::Frame;
use llap::tashtalk::RxStatus;

// Frames are passed by value on purpose: the queues are the only buffers.
#[allow(clippy::large_enum_variant)]
pub enum ToLink {
    Transmit(Frame),
    SetNodeIds([u8; 32]),
    SetFeatures(u8),
}

pub struct FromLink {
    /// Empty for framing errors and aborts.
    pub frame: Frame,
    pub status: RxStatus,
}

pub static TO_LINK: Channel<CriticalSectionRawMutex, ToLink, 4> = Channel::new();
pub static FROM_LINK: Channel<CriticalSectionRawMutex, FromLink, 8> = Channel::new();
