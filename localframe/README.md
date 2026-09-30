# LocalFrame

A remote desktop for classic Macs over LocalTalk: a PC pushes its screen to a
Mac Plus (or any compact Mac) over AppleTalk, and the Mac sends back mouse and
keyboard. The design is in
[docs/remote-desktop-protocol.md](docs/remote-desktop-protocol.md).

The remote desktop itself is **not written yet**. This directory has what
comes first: a small AppleTalk stack for the PC, and two test programs with
both a PC side and a Mac side. They check each link of the chain before any
pixels move:

| Test | PC side | Mac side | What it proves |
|---|---|---|---|
| Echo | `lftest echo` | **ATPing** | NBP lookup, ATP requests, 8-packet responses, throughput |
| Chat | `rchat` | **RChat** | long-polling with asynchronous requests, which the remote desktop needs |

New to AppleTalk? Read [docs/appletalk-primer.md](docs/appletalk-primer.md)
first. It explains AppleTalk versus LocalTalk, the protocols used here, and
LToUDP.

```
localframe/
├── README.md             this file
├── docs/
│   ├── appletalk-primer.md         AppleTalk 101, LocalTalk, LToUDP
│   └── remote-desktop-protocol.md  the remote desktop design (draft)
├── server/               PC side, Rust (Cargo workspace)
│   ├── appletalk/        library: LLAP addressing, DDP, NBP, ATP, LToUDP
│   ├── lftest/           `lftest echo` server and `lftest ping` client
│   └── rchat/            `rchat` chat hub and PC client
└── client/               Mac side, C, built with Retro68
    ├── build.sh          build with Docker or a local Retro68
    ├── CMakeLists.txt
    └── src/
        ├── appletalk.[ch]  .MPP/.ATP glue: NBP lookup, ATP requests
        ├── textwin.[ch]    window with transcript and input line
        ├── atping.c        ATPing
        └── rchat.c         RChat
```

## Status

| Part | State |
|---|---|
| `appletalk` crate | 18 unit tests pass |
| `lftest`, `rchat` on the PC | tested against each other over LToUDP multicast on one host: lookup, 10/10 pings, 92 KB bulk at 17.9 KB/s paced, chat both ways |
| ATPing, RChat on the Mac | compile for the 68000 with Retro68 (about 64 KB each); **not yet run** in an emulator or on a Mac |
| Through a picotalk bridge to a real Mac | not yet tried |
| Remote desktop | design only |

## How the pieces connect

Everything talks **LToUDP**: LocalTalk frames in UDP multicast packets
(239.192.76.84:1954). The PC tools, emulators and the picotalk bridge all join
that group, so they share one virtual AppleTalk network:

```
 1. PC only:       lftest ping ──┐
                                 ├── LToUDP (multicast on the LAN or localhost)
                   lftest echo ──┘

 2. Emulator:      Snow / Mini vMac running ATPing or RChat ──┐
                                                              ├── LToUDP
                   lftest echo / rchat ───────────────────────┘

 3. Real Mac:      Mac ══ LocalTalk ══ picotalk (Pico 2 W, `ltoudp` build) ~~ Wi-Fi ~~┐
                                                                                        ├── LToUDP
                   lftest echo / rchat on a PC on the same LAN ────────────────────────┘
```

Work through the stages in order. Each one adds a single new component, so
when something breaks you know where to look.

## Building the PC side

You need Rust (stable, 2024 edition). From `localframe/server`:

```sh
cargo build --release     # target/release/lftest and target/release/rchat
cargo test                # unit tests
```

The workspace uses the `llap` crate from the picotalk root for the LToUDP
constants, so build it inside this repository.

## Building the Mac side

The Mac programs are C, built with [Retro68](https://github.com/autc04/Retro68),
a GCC cross-compiler for classic Mac OS. They are written for System 6 and 7
on any 68000 Mac with AppleTalk in ROM (Mac Plus onwards), using only calls
those systems have.

### Option A: Docker (easiest)

The Retro68 project publishes a Docker image with the toolchain already
built. With Docker installed, from `localframe/client`:

```sh
./build.sh
```

The first run downloads the image (about 5.6 GB unpacked). The results land in
`client/build/`.

### Option B: install Retro68 natively

Building the toolchain takes a while (half an hour to a couple of hours,
depending on the machine) and several GB of disk.

1. Install the build dependencies.

   Debian/Ubuntu:

   ```sh
   sudo apt-get install cmake libgmp-dev libmpfr-dev libmpc-dev \
       libboost-all-dev bison flex texinfo ruby
   ```

   macOS with Homebrew:

   ```sh
   brew install boost cmake gmp mpfr libmpc bison texinfo
   ```

2. Get the source, submodules included:

   ```sh
   git clone --recursive https://github.com/autc04/Retro68.git
   ```

3. Build it in a directory next to the source. `--no-ppc` skips the PowerPC
   and Carbon toolchains, which LocalFrame does not need:

   ```sh
   mkdir Retro68-build && cd Retro68-build
   ../Retro68/build-toolchain.bash --no-ppc
   ```

   The toolchain ends up in `Retro68-build/toolchain/`.

4. Build the Mac programs with it:

   ```sh
   cd picotalk/localframe/client
   RETRO68=/path/to/Retro68-build ./build.sh
   ```

   or by hand:

   ```sh
   mkdir build && cd build
   cmake .. -DCMAKE_TOOLCHAIN_FILE=/path/to/Retro68-build/toolchain/m68k-apple-macos/cmake/retro68.toolchain.cmake
   make
   ```

Retro68 uses its own free "Multiversal" Mac headers by default, and the code
is written for them. Apple's Universal Interfaces (see Retro68's README)
work too.

### What you get

For each program, `build/` holds:

| File | Use |
|---|---|
| `ATPing.dsk`, `RChat.dsk` | an 800K HFS disk image with the application on it: mount it in an emulator, or write it to a floppy |
| `ATPing.bin`, `RChat.bin` | MacBinary: transfer to a real Mac and unpack with StuffIt Expander or BinUnpk |
| `ATPing.APPL`, `RChat.APPL` | the application with its resource fork in `.rsrc/`, for Basilisk-style shared folders |

## Setting up an emulator (stage 2)

### Snow (recommended)

[Snow](https://github.com/twvd/snow) 1.3.0 and later speak LToUDP out of the
box.

1. Start a Mac Plus (or SE, Classic, …) with System 6 or 7.
2. Enable the bridge: **Ports → Channel B (printer) → Enable LocalTalk
   (UDP)**, or start Snow with `--serial-bridge-b localtalk`. The setting is
   saved in the workspace.
3. Insert `client/build/ATPing.dsk` or `RChat.dsk` as a floppy.
4. In the emulated Mac, open the **Chooser** and set **AppleTalk: Active**.
   Restart if it asks.

On macOS, allow Snow to use the local network when asked.

### Mini vMac

Mini vMac supports LToUDP only in 37.x builds made with the LocalTalk option
(`-lt -lto udp` in its build system); the standard 36.04 download lacks it.
Once built, turn AppleTalk on in the Chooser as above. Drag a `.dsk` onto the
window to mount it.

### Networking notes

* The emulator and the PC tools can run on the same computer; multicast
  loopback is enabled.
* On different computers they must be on the same LAN segment, and the
  firewall must allow **UDP port 1954** and multicast to 239.192.76.84.
* If the PC has several network interfaces (Wi-Fi and Ethernet, Docker
  bridges, VPN), multicast may go out on the wrong one. Pass
  `--iface <address of the right interface>` to the PC tools.

## Connecting a real Mac (stage 3)

1. Build and flash picotalk in its standalone bridge mode (see the
   [picotalk README](../README.md)):

   ```sh
   cd firmware
   WIFI_SSID=myssid WIFI_PASSWORD=secret \
     cargo build --release --no-default-features --features ltoudp
   ```

2. Wire the Pico's transceiver to the Mac's printer port, or to a LocalTalk
   box on the network (see the picotalk README, "Wiring").
3. Put the PC on the same Wi-Fi network or LAN as the Pico.
4. On the Mac, set AppleTalk Active in the Chooser and run ATPing.

Keep `--rate` at or below its default of 20000 bytes/s. LocalTalk carries at
most 28.8 KB/s, and the bridge has only a small queue.

## Using lftest and ATPing

`lftest echo` answers NBP lookups for `NAME:LFEcho` and echoes ATP requests:

```sh
lftest echo                  # NAME is the host name
lftest echo --name lab -v    # log every request
```

`lftest ping` runs, from a PC, the same tests the Mac's ATPing runs: stage 1.
Run it in a second terminal, on the same PC or on another one on the LAN:

```
$ lftest ping
we are node 21
found testbox:LFEcho@* at 0.239:250
ping 0: 32 bytes, 14.0 ms, server sees us as 0.21
…
pings: 10/10 ok
bulk: 92480 bytes in 5.16 s = 17.9 KB/s, 0 bad packets, 0 failed transactions
```

**ATPing** on the Mac looks up LFEcho servers at start and lists them. Type
a command in the input line at the bottom and press Return:

| Command | Does |
|---|---|
| `p` | 10 pings (32- and 566-byte payloads): checks the echo, shows round-trip times and the Mac's node number as the server sees it |
| `b` | 20 bulk transactions of 8 × 578 bytes: checks every byte, shows bytes/s |
| `l` | look the servers up again |
| `1`–`8` | use that server from the list |
| ⌘Q | quit |

The Mac's clock ticks 60 times a second, so round-trip times are only
accurate to about 17 ms. On an emulator the bulk rate shows the pacing (about
18 KB/s). On real LocalTalk expect somewhat less; that is the figure the
remote desktop design needs.

## Using rchat and RChat

`rchat` on the PC runs a chat **hub** and registers `NICK:RChat`. Macs (and
other PCs) join it; whatever anyone types goes to everyone.

```sh
rchat                  # hub; NICK is the host name
rchat --nick Rogier
```

Type a line and press Enter to send it. `/who` lists who is connected, and
`/quit` (or Ctrl-D) stops the hub.

**RChat** on the Mac finds the hub with NBP and joins it, using the
Chooser's user name as nickname. The window shows the conversation; type
in the bottom line and press Return to send. ⌘Q quits and tells the hub you
left. If the hub goes away, RChat keeps looking for it and rejoins when it
is back.

To try the chat without a Mac, join from a second terminal:

```sh
rchat --join --nick tester
```

Mac text uses the Mac Roman character set. rchat converts between it and
UTF-8; characters Mac Roman lacks become `?`.

## Common options

All PC tools take these options:

| Option | Default | Meaning |
|---|---|---|
| `--node N` | random | LLAP node ID to try first: 128–254 for servers (`lftest echo`, `rchat`), 1–127 for clients (`lftest ping`, `rchat --join`) |
| `--rate BPS` | 20000 | pace outgoing frames to BPS bytes/s; 0 sends at once (fine for emulators, not for a bridge) |
| `--iface IP` | OS choice | address of the interface to use for multicast |

## Protocols and numbers

| Service | NBP type | Socket | Protocol description |
|---|---|---|---|
| Echo | `LFEcho` | 250 | [`server/lftest/src/echo.rs`](server/lftest/src/echo.rs) |
| Chat hub | `RChat` | 251 | [`server/rchat/src/proto.rs`](server/rchat/src/proto.rs) |
| Requests from PC tools | – | 254 | – |

Both services use exactly-once ATP transactions. The chat uses the pull
model the remote desktop will use: the client always has one request
outstanding, and the hub holds it (up to 2 s) until there is something to
send.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| ATPing/RChat: error −97 or −98 at start | AppleTalk is inactive: Chooser → AppleTalk Active |
| Nothing found in the lookup | the PC tool is not running; the emulator's LToUDP bridge is off; a firewall blocks UDP 1954; the wrong interface (`--iface`) |
| `lftest ping` finds nothing, on one PC | another program may hold port 1954 exclusively: stop it, or run the tools on another PC |
| Pings work, bulk fails through picotalk | pacing too fast for the bridge: lower `--rate` |
| Error −1096 (reqFailed) | the server stopped answering: it quit, or packets are being lost |

## Documentation

* [docs/appletalk-primer.md](docs/appletalk-primer.md): AppleTalk 101 —
  AppleTalk versus LocalTalk, LLAP, DDP, NBP, ATP, LToUDP, a worked example
  byte by byte, and the Mac's AppleTalk Manager.
* [docs/remote-desktop-protocol.md](docs/remote-desktop-protocol.md): the
  remote desktop design.
