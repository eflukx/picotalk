//! A small AppleTalk node for a PC, reached over LToUDP.
//!
//! ```text
//!  LToUDP ─► ltoudp ─► stack ─┬─ LLAP address acquisition, ENQ/ACK
//!           (Pacer)   (DDP)   ├─ nbp: answer lookups, look names up
//!                             └─ atp: responders (XO cache), requester
//! ```
//!
//! [`node::Node`] runs a [`stack::Stack`] on an LToUDP socket. Everything
//! below it is transport free and unit-tested.
//!
//! Scope: one LocalTalk network without routers (net 0, short DDP headers,
//! long headers answered in kind), no DDP checksums, no ZIP/RTMP, no ADSP.

pub mod atp;
pub mod ddp;
pub mod ltoudp;
pub mod nbp;
pub mod node;
pub mod stack;

pub use ddp::Addr;
pub use nbp::Entity;
pub use node::{Config, Node};
pub use stack::{Event, Role, Stack};
