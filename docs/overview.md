# picotalk: background, decisions and status

This document records where the project came from and why it is built the way
it is. The [README](../README.md) covers building, wiring and the architecture
diagram; this file is the context you need to continue the work.

## Where it started

The starting point was [AirTalk](https://github.com/cheesestraws/airtalk) by
Rob Mitchelmore: a dongle that puts a classic Mac's LocalTalk port on Wi-Fi.

```
Mac ──LocalTalk──► TashTalk (PIC12F1840) ──UART 1 Mbaud──► ESP32 ──Wi-Fi──► LToUDP
```

* **TashTalk** (a PIC running Tashtari's firmware) does the timing-critical
  LocalTalk signalling.
* **The ESP32** bridges LLAP frames to **LToUDP**: UDP multicast to
  `239.192.76.84:1954`, the protocol Mini vMac, TashRouter and others speak.
* AirTalk also lets the Mac configure Wi-Fi in-band, through a Chooser
  extension:
  * an NBP lookup for type `AirTalkAP` returns one fake entry per visible SSID;
  * an ATP request to socket 4 carrying `"airtalk setap" <ssid> <pass>` stores
    the credentials and reboots.

picotalk replaces both chips with one RP2350.

## Why a second chip was needed, and why the RP2350 doesn't need one

LocalTalk is not async serial. It is synchronous SDLC-style framing at
230.4 kbit/s:

* **FM0 coding:** a transition at every bit-cell boundary (about 4.34 µs), plus
  one mid-cell for a `0`.
* **HDLC framing:** `0x7E` flags, zero-bit stuffing, CRC-CCITT.
* **Hard deadlines:** a node must answer an RTS with a CTS within the 200 µs
  inter-frame gap.

An ESP32 has no peripheral for FM0/SDLC, and it cannot guarantee microsecond
timing while running Wi-Fi: the radio stack takes interrupts and flash
operations stall both cores. Hence the PIC, which runs cycle-counted assembly
and does nothing else.

The RP2350 removes the problem:

* **PIO** does the bit timing in hardware-like state machines.
* **Two cores:** core 1 runs only the link layer; core 0 runs Wi-Fi, USB and
  everything else.

A search (GitHub, 68kMLA, TinkerDifferent) found no existing project that
implements LocalTalk natively on an RP2040/RP2350; every open LocalTalk bridge
found uses the TashTalk PIC. 68kMLA could not be searched fully, so ask there
before assuming this is the first.

## What TashTalk does (the behaviour picotalk reproduces)

From reading `firmware/one-chip.asm` (about 3,100 lines) and its protocol docs:

| Aspect | Behaviour |
|---|---|
| Bit time | 35 instruction cycles at 8 MIPS = 4.375 µs (228.6 kbit/s) |
| Preamble | 4 one-bits, then **3** flag bytes (the Apple IIgs needs three) |
| Postamble | closing flag, 13 one-bits, line left driven low, then released |
| Sync pulse | before an RTS/ENQ/control frame: drive ~1.5 bit, release ~3.5 bits |
| CRC | CRC-CCITT reflected (0x8408), init 0xFFFF, residue 0xF0B8 |
| Inter-dialog gap | 400 µs of idle line + random backoff × 100 µs |
| Inter-frame gap | 200 µs: CTS must start within it; broadcasts wait it out in silence |
| Backoff | local mask widens on deferral/collision; global mask adapts to the history of the last 8 frames |
| Attempts | 32, then the frame is dropped |
| Proxying | answers ENQ (with ACK) and RTS (with CTS) for every node ID in a 256-bit bitmap |
| Receive | waits for the line to go idle after the closing flag before acting on a frame |
| Host protocol | `0x01` transmit, `0x02` node IDs, `0x03` features; received frames escaped with `0x00` |

The proxying matters for any bridge: a node on the far side of UDP can never
answer an RTS within 200 µs, so the bridge answers for it.

## Design decisions

* **Receive by oversampling.** PIO samples the pin at 8× the bit rate and
  software classifies the gaps between edges (half bit, full bit, idle). This
  keeps the PIO program to one instruction and makes the decoder testable on a
  PC. The loopback test passes at ±3 % clock mismatch.
* **Transmit from pre-encoded symbols.** A frame is turned into half-bit
  symbols (line level + driver enable) before sending, so the sync pulse,
  stuffing and driver release are all exact. A data frame is encoded before
  its RTS goes out, so it can start the moment the CTS arrives.
* **Core 1 is a busy loop with no interrupts.** It polls both PIO FIFOs. The
  RX FIFO buffers about 139 µs and the TX FIFO about 278 µs, which is the time
  budget for one loop iteration.
* **The MAC is pure logic.** `llap::mac::Mac` gets events and timestamps and
  returns actions, so RTS/CTS, backoff and give-up are unit-tested.
* **TashTalk's host protocol is kept**, so AirTalk, tashtalkd, TashRouter and
  MultiTalk work unchanged with the `host-uart` and `host-usb` builds.
* **LToUDP bridging follows AirTalk's rules:** RTS/CTS stay local; UDP frames
  go to LocalTalk only if broadcast or for a node seen locally in the last
  hour; nodes heard over UDP in the last 30 minutes are proxied.
* **License: GPL-3.0**, because the link behaviour is derived from TashTalk
  (GPL-3.0). AirTalk's firmware is BSD and its hardware CERN-OHL-S-2.0.

## Hardware notes

* **Direct connection to one Mac needs no LocalTalk box.** The Mac's
  mini-DIN-8 has separate transmit and receive pairs; AirTalk wires them
  straight to two RS-485 transceivers. Boxes are only needed for a network of
  several devices, and then the Pico plugs into a box exactly like a Mac.
* **Transceiver:** 3.3 V, fail-safe receiver, ≥ 500 kbit/s. Recommended: one
  full-duplex TI THVD2442. Proven alternative: two GM3085E as in AirTalk.
  RS-485 parts are fine for the Mac's RS-422 port; the Mac tri-states its
  driver in LocalTalk mode anyway.
* **Power:** USB, or 5 V from the Mac's ADB port (pin 3) through a Schottky
  diode into VSYS, as AirTalk does with its two pass-through ADB sockets.
* **Apple IIgs:** same link layer and connector; the three-flag preamble it
  needs is already sent.
* Pinout diagram: [mac-serial-port.svg](mac-serial-port.svg).

## Status

| Part | State |
|---|---|
| `llap` core (CRC, FM0, HDLC, MAC, host protocol, bridge rules) | written, 21 host tests pass |
| Firmware, `host-uart` / `host-usb` / `ltoudp` | compiles for the RP2350 and the RP2040; **never run on hardware** |
| RP2040 (Pico / Pico W) | same code, `rp2040` feature; whether its Cortex-M0+ keeps up with the line in real time is unmeasured |
| Behaviour against a real Mac or IIgs | untested |

## Next steps

1. **Bring-up with a logic analyser** on GP6/GP7/GP8: check the transmit
   waveform, then receive from a real Mac, then the RTS → CTS turnaround.
2. **If timing jitters**, move core 1's hot path from flash into RAM.
3. **Wi-Fi setup without rebuilding:** port AirTalk's Chooser-based
   configuration (needs a small NBP/ATP responder on the Pico) or add a setup
   access point. Credentials are currently fixed at build time.
4. **Status LEDs.**
5. **Remote desktop:** see [localframe/](../localframe/README.md) and its
   [design](../localframe/docs/remote-desktop-protocol.md).

## Loose ends noticed in AirTalk (not fixed, upstream's code)

* `main/apps.c:67-70`: `my_crc32 & 0xFF000000 >> 24` parses as
  `my_crc32 & (0xFF000000 >> 24)`, so the fake NBP address bytes are not what
  the code intends.
* `apps.c` logs the Wi-Fi password in plain text on the serial console.

## References

* TashTalk: <https://github.com/lampmerchant/tashtalk> (firmware, UART
  protocol, transceiver list)
* AirTalk: <https://github.com/cheesestraws/airtalk>
* *Inside AppleTalk*, 2nd edition: LLAP, DDP, NBP, ATP, ADSP
* Mac serial pinout: <https://allpinouts.org/pinouts/connectors/serial/apple-macintosh-rs-422-serial/>
* TI THVD2442 datasheet (SLLSFR1)
