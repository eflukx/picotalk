# AppleTalk 101

A primer on the protocols LocalFrame speaks. It covers what AppleTalk is, how
it differs from LocalTalk, what each layer does, and how LToUDP carries all of
it over a modern network. It explains only as much as you need to read the
code in [`server/appletalk`](../server/appletalk/src) and
[`client/src/appletalk.c`](../client/src/appletalk.c); the definitive
reference is *Inside AppleTalk*, 2nd edition (Addison-Wesley, 1990).

## AppleTalk is not LocalTalk

The two names are easy to mix up:

* **AppleTalk** is a *protocol suite*, like TCP/IP. It covers addressing,
  routing, naming, transactions, streams, file sharing and printing.
* **LocalTalk** is one *link* that AppleTalk runs on: the cabling, the
  electrical signalling and the frame format of the serial port in every Mac
  from 1985 onwards. Its link protocol is **LLAP**, the LocalTalk Link Access
  Protocol.

AppleTalk runs on other links too: **EtherTalk** (Ethernet, with the ELAP
link protocol) and **TokenTalk** (Token Ring). Today it also runs on
**LToUDP**, LocalTalk frames in UDP packets. Above the link layer the protocols
are the same, so a Mac that prints over LocalTalk and one that prints over
EtherTalk speak identical PAP to the printer.

```
          ┌─────────────────────────────────────────────────────┐
 apps     │  AFP (file sharing)   PAP (printing)   your program │
          ├──────────────┬──────────────┬───────────────────────┤
 session  │     ASP      │     PAP      │    ADSP (streams)     │
          ├──────────────┴──────────────┤                       │
transport │     ATP (transactions)       │  NBP  ZIP  RTMP  AEP │
          ├──────────────────────────────┴───────────────────────┤
 network  │                     DDP (datagrams)                  │
          ├──────────────┬───────────────┬──────────────────────┤
  link    │ LLAP         │ ELAP          │ LToUDP (LLAP in UDP) │
 physical │ LocalTalk    │ Ethernet      │ any IP network       │
          └──────────────┴───────────────┴──────────────────────┘
```

LocalFrame uses one path through this: **LLAP (via LToUDP) → DDP → NBP +
ATP**. That is enough to find a service by name and exchange reliable
request/response messages with it.

## The link: LLAP on LocalTalk

LocalTalk is a bus that runs at **230.4 kbit/s**. It uses FM0 line coding and
SDLC-style framing: flags, bit stuffing and a CRC-CCITT frame check sequence.
How picotalk produces those signals is described in the project
[README](../../README.md) and [overview](../../docs/overview.md). At the
protocol level, an LLAP frame looks like this:

```
 dest  src  type  │ payload (0–600 bytes) │ FCS(2)
  1     1    1    │                       │
```

* **Node IDs are one byte**, 1–254, and picked at random when the Mac starts.
  A node chooses a candidate and sends several **ENQ** (enquiry) frames
  addressed to it. If anyone answers with an **ACK**, the ID is taken and it
  tries another. There is no DHCP and no configuration. Workstations use
  1–127 and servers 128–254; 255 is broadcast.
* **Type** 1 and 2 carry DDP (short and long header, below). Types 0x80 and
  up are control frames: ENQ 0x81, ACK 0x82, RTS 0x84, CTS 0x85.
* **Media access is a handshake.** Before a directed data frame the sender
  sends **RTS** (request to send) and the receiver must answer **CTS** within
  the 200 µs inter-frame gap. Only then does the data frame follow. Broadcasts
  skip the handshake. Collisions are avoided with random back-off, not
  detected. The hard 200 µs deadline is why LocalTalk bridges need a
  dedicated chip or, in picotalk's case, the RP2350's PIO.

LLAP delivers on a best-effort basis. There is no retransmission; lost frames
are the business of the layers above.

## The network: DDP

The **Datagram Delivery Protocol** gives each packet a full address and a
*socket*, the AppleTalk equivalent of a UDP port.

An AppleTalk address is `network.node:socket`:

| Part | Size | Meaning |
|---|---|---|
| network | 16 bits | Which network. 0 means "this one" when there is no router. |
| node | 8 bits | The LLAP node ID. |
| socket | 8 bits | 1–127 statically assigned, 128–254 dynamic. 2 is NBP, 4 is echo. |

A DDP packet comes in two forms:

```
short header (LLAP type 1), only between nodes on the same network:
  len(2)  dst_socket  src_socket  ddp_type  │ data (0–586 bytes)

long header (LLAP type 2), whenever a router may be involved:
  hops+len(2)  checksum(2)  dst_net(2)  src_net(2)  dst_node  src_node
  dst_socket  src_socket  ddp_type  │ data
```

`ddp_type` names the protocol inside: 2 is NBP, 3 is ATP, 4 is AEP (echo),
7 is ADSP. A single LocalTalk cable without a router is a *non-extended
network*: its number is 0 until a router tells the nodes otherwise, and
everyone uses short headers. Our stack does the same: it sends short headers
and answers long ones with long ones. Routing (RTMP) and zones (ZIP) are left
out.

AppleTalk Phase 2 (1989) added *extended networks* for EtherTalk, with network
ranges and a different address acquisition. LocalTalk is always non-extended,
so none of that is needed here.

## Naming: NBP

The **Name Binding Protocol** maps names to addresses. Every service has an
*entity name* of the form

```
object:type@zone        e.g.  Rogier's PC:LFEcho@*
```

* **object** is the instance: a user or machine name.
* **type** is the kind of service: `LaserWriter`, `AFPServer`, `LFEcho`,
  `RChat`.
* **zone** groups networks behind routers; `*` means "my zone".

In a lookup, `=` matches anything and `≈` matches any run of characters.
There is **no name server**. Every node keeps its own registered names and
answers for them itself:

```
Mac                                          every node, socket 2
 │── LkUp "=:LFEcho@*", reply to 0.21:2 ─────► (LLAP broadcast)
 │                                           the PC: "that's me"
 │◄── LkUp-Reply "pc:LFEcho@*" is 0.200:250 ──│
```

This is how the Chooser finds printers and file servers. With a router on the
network, the Mac sends a broadcast request (BrRq) to the router instead, and
the router forwards lookups into each network of the zone.

## Transactions: ATP

DDP datagrams can be lost. The **AppleTalk Transaction Protocol** adds
reliability in the simplest useful form: a request and its response.

```
requester                                   responder
    │── TReq  TID=3, bitmap 0b11111111 ─────►│   "I have buffers for 8 packets"
    │◄── TResp TID=3, seq 0 ─────────────────│
    │◄── TResp TID=3, seq 1, EOM ────────────│   "that's all"
    │── TRel  TID=3 ────────────────────────►│   (exactly-once only)
```

* A request carries a **transaction ID** and a **bitmap** of the response
  packets it has room for: up to **8 packets of 578 bytes** each, 4,624 bytes
  per transaction.
* If response packets go missing, the requester **retransmits the request**
  with only the missing bits set. The responder sends just those.
* **At-least-once (ALO)**: a retried request may run twice. That is fine for
  idempotent requests such as "read block 7".
* **Exactly-once (XO)**: the responder remembers each response until the
  requester sends **TRel** (transaction release) or a timeout passes. A
  duplicate request then gets the stored response again and is not run a
  second time. LocalFrame uses XO for everything.
* Every packet also carries 4 **user bytes**, which higher protocols use as
  a small header.

The packet header is 8 bytes:

```
control  bitmap/seq  TID(2)  user bytes(4)  │ data (0–578)
control: 01 TReq / 10 TResp / 11 TRel in bits 7–6; XO bit 5; EOM bit 4
```

ATP gives no ordering between transactions and no streams; ASP and ADSP
build those. For a pull protocol such as RChat's long poll, or the planned
remote desktop, one transaction per exchange is exactly right.

## The rest of the suite, briefly

| Protocol | What it does | Here? |
|---|---|---|
| RTMP | Routing tables between networks | no (no routers) |
| ZIP | Zone names and network ranges | no (zone is always `*`) |
| AEP | Echo on socket 4, the AppleTalk "ping" | no (we have `LFEcho`) |
| ADSP | Reliable byte streams, like TCP | no |
| ASP | Sessions on top of ATP, used by AFP | no |
| AFP | File sharing (AppleShare) | no |
| PAP | Printer access (LaserWriter) | no |

## LToUDP: AppleTalk over a modern network

LToUDP puts an LLAP frame, **without its FCS**, into a UDP datagram and sends
it to the multicast group **239.192.76.84, port 1954**. A 4-byte sender ID in
front lets each program ignore its own packets:

```
 UDP payload:  sender ID(4)  │  dest  src  type  payload…
                             │  └──── LLAP frame, no FCS ────┘
```

Everything on the LAN that joins the group sees every frame, just as every
node on a LocalTalk cable does. Emulators (Mini vMac, Snow), bridges
(AirTalk, the TashTalk-based TashRouter, picotalk's `ltoudp` mode) and our
PC tools all speak it, so they share one virtual LocalTalk network.

What changes compared to a real cable:

* **No RTS/CTS and no FCS.** UDP has its own checksum and there is no
  collision to avoid. A bridge to real LocalTalk does the handshake itself.
  It also **proxies**: it answers RTS and ENQ on the cable for nodes it has
  heard over UDP, because they could never answer within 200 µs.
* **Filtering at the bridge.** picotalk (following AirTalk) forwards a UDP
  frame onto the cable only if it is a broadcast or for a node recently heard
  there, so a busy LAN does not swamp the 230.4 kbit/s wire.
* **Speed mismatch.** A PC can send 8 × 578 bytes in microseconds; LocalTalk
  needs about 0.2 seconds for them. Our tools pace outgoing frames (`--rate`,
  20,000 bytes/s by default) so the bridge's queue never overflows.
* **Loss is still possible** (Wi-Fi, full queues). ATP retries cover it.
* **Multicast stays on the local network.** For remote networks, routers
  such as TashRouter bridge LToUDP to EtherTalk or tunnel it further.

The chain LocalFrame is built to test:

```
 Mac ══ LocalTalk ══ picotalk (Pico 2 W) ~~ Wi-Fi ~~ LAN ── PC
       230.4 kbit/s   LLAP ⇄ LToUDP bridge    UDP multicast  lftest / rchat
                      answers RTS for the PC

 or, all on one computer:

 emulator (Snow, Mini vMac) ── LToUDP on localhost ── lftest / rchat
```

## One exchange, byte by byte

These frames come from our own stack (sender ID shown as `12 34 56 78`). A PC
client is node 21 (`0x15`) and the echo server is node 200 (`0xc8`).

The server claims node 200. It sends this ENQ four times and hears no ACK:

```
12 34 56 78 │ c8 c8 81
            │ dest=200 src=200 type=ENQ
```

The client looks for `=:LFEcho@*`:

```
12 34 56 78 │ ff 15 01 │ 00 17 02 02 02 │ 21 01 00 00 15 02 00 │ 01 3d 06 4c 46 45 63 68 6f 01 2a
            │ LLAP     │ short DDP      │ NBP LkUp, 1 tuple,   │ "="  "LFEcho"  "*"
            │ broadcast│ len 23, 2→2,   │ ID 1, reply to       │
            │ from 21  │ type NBP       │ 0.21:2               │
```

The server answers that it is `pc:LFEcho` at 0.200:250:

```
12 34 56 78 │ 15 c8 01 │ 00 18 02 02 02 │ 31 01 00 00 c8 fa 00 │ 02 70 63 06 4c 46 45 63 68 6f 01 2a
            │ to 21    │ len 24         │ LkUp-Reply, ID 1,    │ "pc"  "LFEcho"  "*"
            │          │                │ 0.200:250            │
```

An exactly-once request with room for one packet: PING, sequence 7, payload "hi":

```
12 34 56 78 │ c8 15 01 │ 00 13 fa fe 03 │ 60 01 00 03 00 00 00 00 │ 01 00 00 07 68 69
            │ to 200   │ 254→250, ATP   │ TReq+XO, bitmap 1,      │ PING seq 7 "hi"
            │          │                │ TID 3, user bytes 0     │
```

The response, marked end-of-message:

```
12 34 56 78 │ 15 c8 01 │ 00 1b fe fa 03 │ 90 00 00 03 00 00 00 00 │ 01 00 00 07 00 00 15 fe c8 00 00 01 68 69
            │          │ 250→254        │ TResp+EOM, seq 0, TID 3 │ echo reply: "you are 0.21:254",
            │          │                │                         │ server 200, count 1, "hi"
```

The client releases the transaction, so the server can forget the response:

```
12 34 56 78 │ c8 15 01 │ 00 0d fa fe 03 │ c0 00 00 03 00 00 00 00
            │          │                │ TRel, TID 3
```

## On the Mac: the AppleTalk Manager

Classic Mac OS has AppleTalk in ROM (from the Mac Plus) and in the System
file. Programs reach it through two drivers:

* **`.MPP`**, reference number −10: LLAP, DDP and NBP.
* **`.ATP`**, reference number −11: ATP.

Every call is a parameter block passed to `_Control` with a `csCode`, for
example `lookupName` (251) on `.MPP` or `sendRequest` (255) on `.ATP`. Apple's
"preferred interface" (`PLookupName`, `PSendRequest`, …) is a thin layer that
fills in these blocks. Retro68's default interfaces do not include it, so
[`appletalk.c`](../client/src/appletalk.c) declares the blocks itself, with
their offsets checked at compile time.

Calls can run **asynchronously**: the driver sets `ioResult` to 1 while it
works and to the final result when done, and the program polls it from its
event loop. RChat keeps a long poll and a send running this way while the user
types.

AppleTalk must be switched on in the **Chooser** ("AppleTalk: Active"). It
then owns the printer port; opening `.MPP` otherwise fails with −97 (port in
use) or −98 (port not configured).

## Further reading

* Gursharan S. Sidhu, Richard F. Andrews, Alan B. Oppenheimer: *Inside
  AppleTalk*, 2nd edition, 1990. Every protocol here, byte by byte.
* *Inside Macintosh: Networking*, 1994, and *Inside Macintosh* volume II,
  "The AppleTalk Manager": the Mac programming interface.
* [TashTalk](https://github.com/lampmerchant/tashtalk) documentation: LLAP
  timing on real hardware.
* [AirTalk](https://github.com/cheesestraws/airtalk): LToUDP bridging rules.
* [Snow's LToUDP page](https://docs.snowemu.com/manual/network/ltoudp.html).
