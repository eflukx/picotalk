# Remote desktop over LocalTalk: design

**Status: draft design; the remote desktop itself is not implemented yet.**
The AppleTalk groundwork is in place (see [Plan](#plan)). The numbers marked
*estimate* must be measured with the simulator (phase 0) before the format is
frozen. For background on the protocols, see the
[AppleTalk primer](appletalk-primer.md).

## Goal

Show a modern computer's desktop on a compact Macintosh (512 × 342, 1-bit) and
send the Mac's mouse and keyboard back, over LocalTalk. It should feel usable
for terminal work, editing and clicking around. Video is out of scope.

```
modern desktop ─► encoder ─► AppleTalk (ATP) ─► LocalTalk ─► Mac client ─► screen
                  (server)                                   (68000)
                     ▲                                          │
                     └────────── mouse, keyboard ◄────────────────┘
```

## Constraints

### Screen sizes

| Mac | Resolution | Depth |
|---|---|---|
| 128K, 512K, Plus, SE, SE/30, Classic, Classic II | **512 × 342** | 1-bit |
| Color Classic, LC with 12" display | 512 × 384 | up to 8-bit |
| Mac II / LC with 13" display | 640 × 480 | 1–8-bit |
| Portable, PowerBook 100–170 | 640 × 400 | 1-bit |

Version 1 targets 512 × 342 at 1 bit. The geometry is negotiated at session
start, so other 1-bit sizes cost nothing extra later. Colour is a separate
future format.

### The link is the bottleneck, not the 68000

* A full frame is 512 × 342 / 8 = **21,888 bytes**.
* LocalTalk is 230.4 kbit/s = 28.8 KB/s raw. After LLAP/DDP/ATP headers, an
  RTS/CTS handshake per packet, gaps and the Mac's own processing, expect about
  **10–20 KB/s** (*estimate*).
* So an uncompressed full frame takes 1–2 seconds.
* An 8 MHz 68000 copies that same frame with `move.l (a0)+,(a1)+` in about
  110,000 cycles, roughly **14 ms**.

Any decoder faster than about 100 KB/s is effectively free. **Optimise for
bytes on the wire; keep the Mac side simple.** The encoder runs on fast
hardware and can try every option.

### 68000 alignment

The 68000 raises an address error on word or long access at an odd address.
So **every field in the format is 16-bit aligned and every payload has even
length**, and the client receives into an even-aligned buffer. The decoder then
uses only long moves.

## Principles

1. **Send only what changed**, in tiles.
2. **XOR against what the client already has.** Unchanged pixels become zero,
   and zero needs no write at all.
3. **Send the newest state, never a backlog.** The server diffs the current
   screen against what the client has confirmed. If the link falls behind,
   intermediate states are simply never sent.
4. **The client pulls.** It asks for an update when it is ready for one. That
   gives flow control and acknowledgement for free.
5. **The cursor is local.** The Mac draws its own cursor and only reports the
   position, so pointing feels instant.

## Tiles

* Tile size: **32 × 32 pixels**. One tile row is exactly one 32-bit long.
* 512 × 342 gives 16 columns × 11 rows = **176 tiles**, indexed
  `row * 16 + column`, so an index fits one byte.
* The bottom tile row is clipped to 22 lines (342 = 10 × 32 + 22). Its tiles
  carry 22 rows; row-mask bits beyond that must be zero.
* The screen is 64 bytes per line, so the next row of a tile is 64 bytes on.

The tile size is a parameter of the session, not baked into the client, so the
simulator can compare 32 × 32 with 32 × 16 and 64 × 32.

## Update format

An update is a byte stream, carried in one ATP response (up to 8 packets).

```
update header (6 bytes)
  u16  seq        update number, increments by 1
  u16  event_ack  highest input event_seq the server has processed
  u8   flags      bit 0 MORE      more dirty tiles are waiting on the server
                  bit 1 KEYFRAME  every tile in this update is absolute
  u8   ntiles     number of tile records that follow

tile record
  u8   tile       tile index, 0..175
  u8   mode       see below
  ...  payload    even length, depends on mode
```

Tiles are applied in the order given.

An earlier sketch used a 176-bit dirty bitmap instead of a tile index per
record. The index costs one byte per tile but keeps records aligned and lets an
update carry any subset of the dirty tiles, which the pull model needs.

### Tile modes

| Mode | Name | Payload | Meant for |
|---|---|---|---|
| 0 | `FILL0` | none | all white: cleared areas |
| 1 | `FILL1` | none | all black |
| 2 | `RAW` | 4 bytes per row, absolute | dither, photos: the fallback |
| 3 | `ROWXOR` | `u32` row mask + 4 bytes per set bit | typing, cursor blink, small edits |
| 4 | `XRLE` | 16-bit ops, see below | bigger changes with structure |
| 5 | `COPY` | `i16 dx_tiles`, `i16 dy_rows` | scrolling, window moves |

The encoder tries every mode for each dirty tile and sends the smallest.
`FILL0`, `FILL1` and `RAW` are *absolute*: they do not depend on what the
client has. Only these are allowed in a `KEYFRAME` update.

**`ROWXOR`** is the workhorse. Bit 31 of the mask is row 0. For each set bit
one long follows, to be XORed into that row. One typed character touches about
12 rows of one or two tiles: roughly 50 bytes on the wire.

```asm
; a0 = payload (even address), a1 = tile's top-left in the frame buffer
        move.l  (a0)+,d1        ; row mask, bit 31 = row 0
        moveq   #31,d2
.row:   add.l   d1,d1           ; top bit into carry
        bcc.s   .skip
        move.l  (a0)+,d0
        eor.l   d0,(a1)
.skip:  lea     64(a1),a1
        dbra    d2,.row
```

**`XRLE`** is run-length coding over the tile's rows, still as XOR deltas. Each
op is one 16-bit word: the top two bits are the kind, the low six bits are
`n − 1`.

| Bits 15–14 | Op | Followed by | Effect |
|---|---|---|---|
| 00 | `SKIP n` | nothing | next n rows unchanged |
| 01 | `LIT n` | n longs | XOR one long into each of the next n rows |
| 10 | `REP n` | 1 long | XOR the same long into the next n rows |
| 11 | `END` | nothing | remaining rows unchanged |

The ops end at `END` or when all rows are covered.

**`COPY`** takes the tile's content from elsewhere in the client's frame
buffer: `dx_tiles` tiles to the side (so the source is long-aligned) and
`dy_rows` rows up or down. Sources are read from the buffer as it is when the
record is applied. The encoder must order `COPY` records so that no source has
been overwritten yet, as `memmove` does, and must fall back to another mode
where it cannot. Arbitrary horizontal offsets are left out of version 1 because
they need bit shifting on the 68000.

## Transport: the client pulls with ATP

AppleTalk Transaction Protocol is in ROM from the Mac Plus on, is reliable, and
returns up to 8 packets of 578 bytes (4,624 bytes) per request. That fits the
pull model exactly:

```
client                                   server
  │ TReq UPDATE (have_seq = 41, input) ──►│  41 confirmed: commit it to the model
  │                                       │  diff screen against the model
  │◄── TResp ×1..8: update seq 42 ────────│  (waits up to ~100 ms if nothing changed)
  │ apply, blit                           │
  │ TReq UPDATE (have_seq = 42, input) ──►│
```

* **Acknowledgement is implicit.** `have_seq` in the next request confirms the
  previous update.
* **The server keeps a model of the client's frame buffer.** It applies an
  update to the model only once it is confirmed.
* **Loss is handled by ATP.** With exactly-once transactions a retried request
  gets the same response again.
* **Resync.** If `have_seq` is not what the server expects (restart, bug), it
  answers with `KEYFRAME` updates until the whole screen has been sent.
* **Big changes are spread over several updates.** The server fills one
  response with the most useful tiles, sets `MORE`, and the rest stay dirty.
  Tiles near the cursor go first.
* **Response size adapts.** The client's ATP bitmap says how many packets it
  wants. Asking for fewer keeps each round trip short while typing; asking for
  eight gives the best throughput during a big redraw.
* **Idle.** With nothing dirty the server holds the request for about 100 ms
  before answering with an empty update, so an idle session costs a few
  packets per second.

ADSP (a reliable byte stream) is the alternative. It was not chosen because it
is not present on older systems without an extra driver, and a stream
encourages queuing stale frames.

### Request format (client → server)

```
u8   type        1 = HELLO, 2 = UPDATE
u8   flags
u16  have_seq    last update applied (UPDATE only)
u16  mouse_x
u16  mouse_y
u8   buttons
u8   nevents
     events      nevents × { u16 event_seq, u8 kind, u8 code }
```

* `kind` is key down, key up, or mouse button change; `code` is the Mac
  virtual key code or button state.
* **Input must not wait for a screen update.** If an event happens while a
  request is outstanding, the client sends it at once in a plain DDP datagram
  with the same layout (`type` 3 = INPUT). Note: sending raw DDP from the Mac
  needs a DDP socket, and opening one requires a socket listener written in
  assembly. A second, small ATP request is the simpler alternative; LFTest
  already runs several ATP requests side by side this way.
* Events are repeated in following requests until the server's `event_ack`
  covers them. The server drops duplicates by `event_seq`. Mouse
  position needs no retry: the next packet supersedes it.

`HELLO` opens a session: the client sends its screen width, height, depth and
protocol version; the server answers with the geometry and tile size it will
use. The server is found by an NBP lookup (type `LocalFrame`, name to be
confirmed).

## Encoder (server side)

* **Get 1-bit pixels without scaling if possible.** Run the source desktop or
  VNC session at 512 × 342 so text stays sharp.
* **Dither with a fixed threshold pattern** (ordered / Bayer or a static
  blue-noise mask), never error diffusion. Error diffusion makes the pattern
  shimmer between frames, which ruins XOR deltas.
* **Hide the remote cursor** (VNC's client-side cursor option), because the Mac
  draws its own.
* **Detect scrolling.** Look for a vertical shift that makes most of a changed
  region match, and emit `COPY` tiles. This turns a scrolled text window from
  kilobytes into tens of bytes.
* **Pick the cheapest mode per tile** by encoding all of them.

Where it runs:

| Option | Pros | Cons |
|---|---|---|
| **On a PC**, speaking AppleTalk over LToUDP through a picotalk bridge | fast to develop, easy to debug | the PC needs a small DDP/NBP/ATP stack |
| **On the Pico 2 W**, which is then a VNC client on Wi-Fi and an AppleTalk node on LocalTalk | standalone gadget | needs DDP/NBP/ATP and a VNC client in the firmware; 520 KB RAM is ample for a few 22 KB buffers |

The wire format is the same either way. Start on the PC.

## Client (Mac side)

* Build with **Retro68** (GCC cross-compiler for 68k Macs); C with the tile
  loops in assembly.
* **Decode into an off-screen copy of the frame buffer** (22 KB), then copy the
  touched tiles to the screen. XOR deltas only work if the buffer holds exactly
  what the server thinks it does, and the real screen also contains the cursor
  and anything the system draws. Copying a tile is 32 long moves.
* The screen address comes from the `ScrnBase` low-memory global (`$0824`).
  Wrap screen writes in `ShieldCursor` / `ShowCursor`.
* Use ATP through the `.ATP` driver, asynchronously, so the event loop keeps
  reading the mouse and keyboard.
* One binary serves every compact Mac. A Plus needs nothing beyond its ROM.

## Expected cost (*estimates, to be measured*)

| Action | Bytes on the wire | Feel |
|---|---|---|
| Mouse move | 0 downstream (cursor is local) | instant |
| One typed character | ~50 (1–2 `ROWXOR` tiles) | one packet round trip |
| Blinking text cursor | ~20 | negligible |
| Scroll a text window one line | tens to a few hundred (`COPY` + a new line) | a few packets |
| Open a window / menu | 1–4 KB | well under a second |
| Full-screen redraw, worst case (`RAW`) | 22.5 KB | 1–2 s |

## Future enhancements: compression

Version 1 compresses only through the tile modes: dirty tiles, XOR deltas,
run-length and `COPY`. Each tile is coded on its own, so nothing is shared
between tiles: a line of text sends the same glyph again and again, a newly
opened window pays full price per tile, and the 50 % grey desktop pattern is
re-sent in every tile.

Four enhancements address that, in order of effort. None is committed; the
phase 0 simulator decides which are worth it.

### 1. Pattern-fill mode

A new tile mode `FILLPAT` with an 8-byte payload: an 8 × 8 pattern, as in
QuickDraw. It covers the grey desktop, scroll-bar tracks and title-bar stripes
in 10 bytes per tile. It is absolute, so it is allowed in `KEYFRAME` updates.
Cost: a few lines on each side.

### 2. Byte-oriented LZ over the whole update

Compress the update's tile records with an LZ4- or LZSA-style coder: literals
plus copy-from-earlier, in whole bytes.

* It catches repetition across tiles: repeated glyphs, identical rows,
  patterns.
* Decoding on a 68000 is essentially a copy loop, far faster than the link
  delivers data (to be confirmed on a Plus).
* It sits under the tile format. The client decompresses into an even-aligned
  buffer and then decodes tiles as before, so the alignment rules are
  untouched.
* One flag bit in the update header says whether the payload is compressed, so
  small updates can skip it.

Expected effect: a lot for big redraws, nothing for small updates, which are
already tiny.

### 3. The client's frame buffer as the dictionary

Let LZ matches point into the frame buffer the Mac already holds, not only at
earlier bytes of the same update. This generalises `COPY`: scrolling, window
moves and any glyph already visible somewhere become short references.

* The work is on the encoder, which has CPU to spare. On the Mac it is one
  more copy source.
* It needs the same ordering rule as `COPY`: a source must not have been
  overwritten yet.
* Matches are byte-aligned, so they find content that moved by multiples of
  8 pixels. That covers vertical scrolling at any offset, and text in a
  fixed-width font on an 8-pixel grid.

Expected effect: the biggest win for scrolling and text.

### 4. Tile cache

The client keeps the last N distinct tiles; the server sends "tile number k
again" instead of the pixels, as RDP-style protocols do. It needs memory and
bookkeeping on the Mac, and both sides must evict in exactly the same order.
It overlaps heavily with enhancement 3, so it is only worth doing if 3 proves
impractical.

### Choosing the LZ format

| Candidate | Output | Notes |
|---|---|---|
| LZ4 | bytes | simplest and fastest decoder; weakest ratio of the group |
| LZSA2 | bytes/nibbles | made for 8- and 16-bit machines; better ratio than LZ4, still fast |
| ZX0 | bits and bytes | better ratio again; slower decoder |
| [heatshrink](https://github.com/atomicobject/heatshrink) | bit-packed LZSS | built for tiny RAM, which is not our constraint; the stock decoder reads bit by bit and would need a 68k assembly rewrite; its own window makes enhancement 3 awkward |

The decoder only has to be comfortably faster than the link (about 15 KB/s),
and it must not stall the Mac's event loop. Compare all four in the simulator
on recorded sessions: compressed size, and decode time on an emulated Plus.

Ruled out:

* **Deflate / zlib** and **Shrinkler-style range coders:** better ratios, but
  bit-by-bit decoding on a 68000 costs more time than the smaller size saves,
  and freezes the mouse while unpacking.
* **Fax-style coders (CCITT G4, JBIG):** designed for scanned pages, complex,
  and poor at small deltas.

## Developing against an emulator

Two emulators speak LToUDP, so the whole stack can be built and tested on one
PC, with no Pico and no real Mac:

```
PC: encoder + small AppleTalk stack ──LToUDP (UDP multicast)──► emulator: Mac client
```

| Emulator | LToUDP | Notes |
|---|---|---|
| [Snow](https://github.com/twvd/snow) | from v1.3.0 | no custom build; aims for hardware accuracy, so client speed should be close to a real Mac. **Start here.** |
| [Mini vMac](https://www.gryphel.com/c/minivmac/) | 37.03 beta, build option | build a variant with `-lt -lto udp`; the standard 36.04 download lacks it |
| Basilisk II, SheepShaver | no | Ethernet (EtherTalk) only |

What this covers: NBP lookup, the ATP pull loop, the tile decoder, the screen
copy and input events. The same Retro68-built client runs unchanged on a real
Mac later.

What it does not cover:

* **Link speed.** LToUDP on a LAN is far faster than LocalTalk: no
  230.4 kbit/s limit and no RTS/CTS per packet. The server must throttle
  itself (about 10–20 KB/s, plus a few milliseconds per packet) to behave
  realistically. Confirm that figure on real hardware.
* **The Pico's line timing** (FM0, the 200 µs CTS deadline, the transceiver).
  That still needs a logic analyser and a real Mac.
* **Exact CPU timing.** Treat decode-speed measurements as approximate until
  checked on a real Plus.

## Plan

Done before phase 0, to prove the toolchain and the network chain:

* **AppleTalk stack for the PC** (`server/appletalk`): LLAP node acquisition,
  DDP, NBP (answering and looking up), ATP responder with the exactly-once
  cache and ATP requester, LToUDP transport with pacing.
* **Test program with a Mac client**: `lftest serve` / LFTest, with echo
  tests (lookup, ATP, 8-packet responses, throughput) and a chat (the
  long-poll pull model with asynchronous ATP on the Mac). See the
  [README](../README.md).

Still to do:

0. **Simulator on a PC.** Capture a real desktop, encode, decode, and compare;
   print bytes per update for typing, scrolling, window moves. Tune tile size
   and modes here. No Mac involved.
1. **Server on a PC over LToUDP**, on the `appletalk` crate (which already
   has the DDP/NBP/ATP implementation and the bandwidth pacing). Test against
   an emulator.
2. **Mac client** with Retro68, developed in the emulator: `RAW` and `FILL`
   first, then `ROWXOR`, `XRLE`, `COPY`, then input.
3. **Real hardware:** the same server and client, with a real Mac behind a
   picotalk bridge in `ltoudp` mode. Measure throughput and round-trip time,
   and tune the response-size policy.
4. **Optionally move the server into the Pico 2 W.**

## Open questions

* **Is 32 × 32 the right tile?** It ignores horizontal repetition across tiles.
  Measure against wider tiles, with and without the compression enhancements
  above.
* **Real throughput and round-trip time** of ATP on a Plus, which set the
  response-size policy.
* **Horizontal scrolling and sub-tile `COPY`**: worth the 68000 bit shifting?
* **Keyboard mapping** from Mac virtual key codes (and the missing modifier
  keys) to the remote system.
* **Multiple monitors / larger remote desktops**: pan, or scale down?
* **Colour Macs**: a separate tile format at 2, 4 or 8 bits per pixel.
* **Security**: none in version 1. Anyone on the AppleTalk network could
  connect. At least add a shared secret to `HELLO`.
