//! Host side of the TashTalk protocol, on any async byte stream.

use defmt::warn;
use embedded_io_async::{Read, Write};
use llap::frame::MAX_FRAME;
use llap::tashtalk::{self, Command, CommandParser};

use crate::shared::{FROM_LINK, TO_LINK, ToLink};

/// Reads commands from the host and hands them to the link. Back-pressure
/// (the link queue being full) stops reading, which in turn deasserts flow
/// control towards the host.
pub async fn host_to_link<R: Read>(rx: &mut R) -> R::Error {
    let mut parser = CommandParser::new();
    let mut buf = [0u8; 64];
    loop {
        let n = match rx.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => return e,
        };
        for &b in &buf[..n] {
            let msg = match parser.feed(b) {
                None => continue,
                Some(Command::Transmit(frame)) => ToLink::Transmit(frame.clone()),
                Some(Command::SetNodeIds(ids)) => ToLink::SetNodeIds(ids),
                Some(Command::SetFeatures(f)) => ToLink::SetFeatures(f),
            };
            TO_LINK.send(msg).await;
        }
    }
}

/// Forwards received frames to the host.
pub async fn link_to_host<W: Write>(tx: &mut W) -> W::Error {
    let mut out = [0u8; tashtalk::max_encoded_len(MAX_FRAME)];
    loop {
        let msg = FROM_LINK.receive().await;
        let mut n = 0;
        tashtalk::encode_rx(msg.frame.as_slice(), msg.status, |b| {
            out[n] = b;
            n += 1;
        });
        if let Err(e) = tx.write_all(&out[..n]).await {
            warn!("host write failed");
            return e;
        }
    }
}
