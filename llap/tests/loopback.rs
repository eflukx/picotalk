//! End-to-end: frame -> symbols -> simulated line samples -> decoder ->
//! deframer, with clock mismatch and sampling phase offsets.

use llap::crc;
use llap::fm0::{Decoder, SAMPLES_PER_BIT, SYMBOLS_PER_WORD};
use llap::frame::{self, LLAP_RTS};
use llap::hdlc::{Deframer, RxEvent, encode_frame, max_words};

/// Renders transmit words into line samples. `ratio` is transmitter bit time
/// divided by receiver bit time (1.0 = perfect clocks); `phase` shifts the
/// sampling instants by a fraction of a sample.
fn render(words: &[u32], ratio: f64, phase: f64, idle_level: bool) -> Vec<bool> {
    let mut levels = Vec::new(); // one entry per half bit
    for w in words {
        for i in 0..SYMBOLS_PER_WORD {
            let sym = (w >> (2 * i)) & 3;
            // Released: the bias network pulls the line to its idle level.
            let line = if sym & 2 != 0 { sym & 1 != 0 } else { idle_level };
            levels.push(line);
        }
    }
    let half = SAMPLES_PER_BIT as f64 / 2.0 * ratio; // samples per half bit
    let total = (levels.len() as f64 * half) as usize;
    (0..total)
        .map(|s| {
            let t = (s as f64 + phase) / half;
            levels[(t as usize).min(levels.len() - 1)]
        })
        .collect()
}

fn pack(samples: &[bool]) -> Vec<u32> {
    samples
        .chunks(32)
        .map(|c| {
            let mut w = 0;
            for i in 0..32 {
                let s = *c.get(i).unwrap_or(c.last().unwrap());
                w |= (s as u32) << (31 - i);
            }
            w
        })
        .collect()
}

fn receive(samples: &[bool]) -> Vec<(Vec<u8>, bool)> {
    let mut dec = Decoder::new();
    let mut def = Deframer::new();
    let mut frames = Vec::new();
    for w in pack(samples) {
        dec.push_word(w, |ev| {
            def.line(ev, |rx| match rx {
                RxEvent::Frame { data, crc_ok } => frames.push((data.to_vec(), crc_ok)),
                other => panic!("unexpected {other:?}"),
            })
        });
    }
    frames
}

fn data_frame(dest: u8, src: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() + 2;
    let mut f = vec![dest, src, 0x01, (len >> 8) as u8, len as u8];
    f.extend_from_slice(payload);
    f.extend_from_slice(&crc::fcs(&f));
    f
}

fn transmit(frames: &[(Vec<u8>, bool)], ratio: f64, phase: f64, idle: bool) -> Vec<bool> {
    let mut samples = vec![idle; 100];
    for (f, sync) in frames {
        let mut buf = vec![0u32; max_words(f.len())];
        let n = encode_frame(&mut buf, f, *sync).unwrap();
        samples.extend(render(&buf[..n], ratio, phase, idle));
        samples.extend(std::iter::repeat_n(idle, 200));
    }
    samples
}

#[test]
fn loopback_across_clock_skew_and_phase() {
    let payload: Vec<u8> = (0..=255u8).chain([0xFF; 40]).chain([0x00; 40]).collect();
    let frames = vec![
        (frame::control_frame(8, 3, LLAP_RTS).to_vec(), true),
        (data_frame(8, 3, &payload), false),
        (data_frame(0xFF, 3, &[0x7E; 20]), false),
    ];
    for ratio in [0.97, 0.99, 1.0, 1.008, 1.03] {
        for phase in [0.0, 0.25, 0.5, 0.75] {
            for idle in [false, true] {
                let got = receive(&transmit(&frames, ratio, phase, idle));
                assert_eq!(got.len(), frames.len(), "ratio {ratio} phase {phase} idle {idle}");
                for ((data, ok), (sent, _)) in got.iter().zip(&frames) {
                    assert!(ok, "crc ratio {ratio} phase {phase}");
                    assert_eq!(data, sent);
                }
            }
        }
    }
}

#[test]
fn max_size_frame_fits_buffer() {
    let f = data_frame(1, 2, &[0xFF; frame::MAX_FRAME - 7]);
    assert_eq!(f.len(), frame::MAX_FRAME);
    let mut buf = vec![0u32; max_words(f.len())];
    assert!(encode_frame(&mut buf, &f, true).is_some());
}
