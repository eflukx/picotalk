# LocalFrame

A remote desktop for classic Macs over LocalTalk: a PC pushes its screen to a
Mac Plus (or any compact Mac) over AppleTalk, and the Mac sends back mouse and
keyboard. The design is in
[docs/remote-desktop-protocol.md](docs/remote-desktop-protocol.md).

The remote desktop itself is **not written yet**. This directory has what
comes first: a small AppleTalk stack for the PC, and a test program with a
PC side and a Mac side, to check each link of the chain before any pixels
move:

* **`lftest serve`** on the PC offers three services on one AppleTalk node:
  an echo service, a chat hub, and NOS Teletekst (bridged from
  `ssh teletekst.nl`).
* **LFTest** on the Mac has two windows that run at the same time: *Echo
  Test*, with buttons for the ping and bulk tests, and *Chat*.
* **Teletekst** on the Mac shows Teletekst pages, block graphics included.
* **Tanks** on the Mac shows a tank-level dashboard full screen: three
  tanks with their levels and totals, and the weather.
  There is an **Apple IIgs** version too, in 65816 assembly and in colour:
  [client_iigs/](client_iigs/README.md).

| Test | What it proves |
|---|---|
| Echo: ping and bulk | NBP lookup, ATP requests, 8-packet responses, round-trip time, throughput |
| Chat | long polling with asynchronous requests, which the remote desktop needs; running it during a bulk test shows how the two share the link |
| Teletekst | a screen kept on the server, of which the Mac receives only the rows that changed: the remote desktop's model in miniature |

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
│   └── lftest/           `lftest`: the test server (serve), PC clients
│                         (ping, chat, teletekst) and a frame monitor
├── client/               Mac side, C, built with Retro68
│   ├── build.sh          build with Docker or a local Retro68
│   ├── CMakeLists.txt
│   └── src/
│       ├── appletalk.[ch]  .MPP/.ATP glue: NBP lookup, ATP requests (async)
│       ├── textwin.[ch]    text windows with buttons and an input line
│       ├── echo.[ch]       the Echo Test window
│       ├── chat.[ch]       the Chat window
│       ├── lftest.c        LFTest: sets up both windows, runs the event loop
│       ├── teletekst.c     Teletekst
│       └── tanks.c         Tanks
└── client_iigs/          Apple IIgs Teletekst, 65816 assembly (Merlin 32)
    ├── build.sh          assembles, and makes an 800K ProDOS disk image
    ├── src/teletekst.s   the program; src/font.s is generated
    └── tools/mkfont.py   the font generator (from font8x8, public domain)
```

## Status

| Part | State |
|---|---|
| `appletalk` crate, `lftest` | 29 unit tests pass; `serve`, `ping` and `chat` tested against each other over LToUDP on one host: 10/10 pings, 92 KB bulk at 17.9 KB/s paced, chat both ways |
| Mac, in Snow (Mac Plus) with `lftest` on the same Windows PC | the earlier single-purpose version (ATPing) worked: 10/10 pings at about 70 ms, bulk 4.3 KB/s with 0 bad packets. The bulk rate is lower than expected and still being investigated |
| LFTest (both windows) | runs in Snow: echo tests and chat at the same time |
| Teletekst | the service tested with `lftest teletekst` (pages, colour keys, arrows); the Mac app runs in Snow |
| Tanks | the service tested with `lftest tanks` against the live dashboard; the Mac app compiles, **not yet run** |
| Teletekst for the IIgs | runs in GSplus up to the AppleTalk check (draws, detects, quits cleanly); the network part waits for a real IIgs on picotalk, as no IIgs emulator speaks LToUDP |
| Snow on one PC, `lftest` on another over Wi-Fi | not working yet: Windows sent Snow's multicast out of the wrong network adapter (see [Networking notes](#networking-notes)); still being checked |
| Through a picotalk bridge to a real Mac | not yet tried |
| Remote desktop | design only |

## How the pieces connect

Everything talks **LToUDP**: LocalTalk frames in UDP multicast packets
(239.192.76.84:1954). The PC tools, emulators and the picotalk bridge all join
that group, so they share one virtual AppleTalk network:

```
 1. PC only:       lftest ping / lftest chat ──┐
                                               ├── LToUDP (multicast on the LAN or localhost)
                   lftest serve ───────────────┘

 2. Emulator:      Snow / Mini vMac running LFTest ──┐
                                                     ├── LToUDP
                   lftest serve ─────────────────────┘

 3. Real Mac:      Mac ══ LocalTalk ══ picotalk (Pico 2 W, `ltoudp` build) ~~ Wi-Fi ~~┐
                                                                                        ├── LToUDP
                   lftest serve on a PC on the same LAN ───────────────────────────────┘
```

For stage 2, start with the emulator and `lftest serve` on the **same
computer**: that takes the network out of the picture. Move `lftest` to
another machine once that works.

Work through the stages in order. Each one adds a single new component, so
when something breaks you know where to look.

## Building the PC side

You need Rust (stable, 2024 edition). From `localframe/server`:

```sh
cargo build --release     # target/release/lftest (lftest.exe on Windows)
cargo test                # unit tests
```

The workspace uses the `llap` crate from the picotalk root for the LToUDP
constants, so build it inside this repository. It builds on Linux, macOS
and Windows.

## Building the Mac side

The Mac program is C, built with [Retro68](https://github.com/autc04/Retro68),
a GCC cross-compiler for classic Mac OS. It is written for System 6 and 7 on
any 68000 Mac with AppleTalk in ROM (Mac Plus onwards), using only calls
those systems have. The build needs Linux or macOS (or Docker).

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

4. Build the Mac program with it:

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

`build/demos.dsk` is **The One Disk**: an 800K HFS disk image named
"The One Disk" with all three applications on it. Mount it in an emulator
or write it to a floppy, and everything is there.

Each application has its own Finder icon. `client/tools/mkicons.py` draws
them and writes `src/*.r`. The Finder shows an application's icon only when
the file's bundle bit is set, which Retro68 does not do, so the build sets
it with `tools/setbundle.py` and makes the disk images with
`tools/mkdisk.sh`. A disk that showed the generic icons before may need its
desktop rebuilt (hold ⌘⌥ while inserting it).

`build/` also holds, for each of LFTest, Teletekst and Tanks:

| File | Use |
|---|---|
| `NAME.dsk` | an 800K HFS disk image with the application on it: mount it in an emulator, or write it to a floppy |
| `NAME.bin` | MacBinary: the application with its resource fork, packed into one file. A plain copy (for example through a BlueSCSI's Toolbox share) arrives as a document the Finder cannot open; unpack it on the Mac with BinUnpk or StuffIt Expander first |
| `NAME.APPL` | the application with its resource fork in `.rsrc/`, for Basilisk-style shared folders |
| `NAME.code.bin` | an intermediate build file (the code before Rez adds the other resources); ignore it |

## Setting up an emulator (stage 2)

### Snow (recommended)

[Snow](https://github.com/twvd/snow) 1.3.0 and later speak LToUDP out of the
box.

1. Start a Mac Plus (or SE, Classic, …) with System 6 or 7.
2. Enable the bridge: **Ports → Channel B (printer) → Enable LocalTalk
   (UDP)**, or start Snow with `--serial-bridge-b localtalk`. The setting is
   saved in the workspace.
3. Insert `client/build/LFTest.dsk` as a floppy.
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
* **Several network interfaces** (Wi-Fi and Ethernet, Docker bridges, VPNs,
  Tailscale, WSL): multicast may go out on the wrong one. The PC tools take
  `--iface <address of the right interface>`. Emulators have no such
  setting; the operating system picks.
* **Windows** picks the connected adapter with the lowest *interface
  metric*, which is often Tailscale, a VPN or `vEthernet (WSL)` rather than
  Wi-Fi. List them with
  `Get-NetIPInterface -AddressFamily IPv4 | Sort-Object InterfaceMetric`, and
  pin the LToUDP group to the right adapter (administrator PowerShell, then
  restart the emulator):

  ```powershell
  New-NetRoute -DestinationPrefix 239.192.76.84/32 -InterfaceAlias "Wi-Fi" -RouteMetric 1
  ```

  This changes the route for that one multicast address only, and survives
  a reboot. `Remove-NetRoute -DestinationPrefix 239.192.76.84/32` undoes it.
  A network classified as *Public* also blocks it; make it *Private*.
* **Wi-Fi**: multicast between two Wi-Fi clients goes through the access
  point. Client isolation, IGMP snooping without a querier, or
  multicast-to-unicast conversion can drop it even within one subnet.
* To see what actually arrives, run `lftest monitor` (below) on each
  machine.

## Connecting a real Mac (stage 3)

1. Build and flash picotalk in its standalone bridge mode (see the
   [picotalk README](../README.md); for a Pico W, use `rp2040` and the
   RP2040 target as described there):

   ```sh
   cd firmware
   WIFI_SSID=myssid WIFI_PASSWORD=secret \
     cargo build --release --no-default-features --features rp2350,ltoudp
   ```

2. Wire the Pico's transceiver to the Mac's printer port, or to a LocalTalk
   box on the network (see the picotalk README, "Wiring").
3. Put the PC on the same Wi-Fi network or LAN as the Pico.
4. On the Mac, set AppleTalk Active in the Chooser and run LFTest.

Keep `--rate` at its default of 30000 bytes/s or lower (see
[Pacing](#pacing)): the bridge has only a small queue. If bulk tests lose
packets through the bridge, try 20000.

## Using lftest

`lftest` is the PC side: one program with five subcommands. Run it with
`cargo run --release -p lftest -- <subcommand>` from `localframe/server`, or
as `target/release/lftest <subcommand>`.

### `lftest serve`: the server

```sh
lftest serve                  # NAME is the host name
lftest serve --name lab -v    # log every lookup and request
```

It registers `NAME:LFEcho` (socket 250), `NAME:RChat` (socket 251) and
`NAME:Teletekst` (socket 252) on one node, and answers all three. The terminal is part of the chat: type a line and
press Enter to send it as NAME. `/who` lists who is connected, and `/quit`
stops the server. Without a terminal (stdin closed) it keeps serving.

The node number is derived from NAME, so a restarted server comes back at
the same address and connected Macs carry on without looking it up again.

### LFTest on the Mac

Start it with AppleTalk active. It opens two windows, both working at once:

**Echo Test** finds the server by itself and shows its name and address.
Then use the buttons:

| Button | Does |
|---|---|
| Ping | 10 pings (32- and 566-byte payloads): checks the echo, shows round-trip times and the Mac's node number as the server sees it |
| Bulk | 20 transactions of 8 × 578 bytes: checks every byte, shows bytes/s |
| Find Server | look the server up again (after moving it to another machine) |
| Stop | stop a running test; Esc and ⌘. do the same |

If a request gets no answer (about 10 s), the test stops and LFTest looks
for the server again.

The Mac's clock ticks 60 times a second, so round-trip times are only
accurate to about 17 ms. On real LocalTalk, the bulk rate is the figure the
remote desktop design needs.

**Chat** joins the hub with the Chooser's user name as nickname. Type in
the line at the bottom of the window and press Return; typing always goes
there, whichever window is in front. Up and Down arrow step through the
lines you sent before (the last 16); on keyboards without arrow keys, such
as the original Macintosh keyboard, use ⌘P and ⌘N. If the hub goes away, the chat keeps
looking for it and rejoins when it is back. ⌘Q quits and tells the hub you
left.

Mac text uses the Mac Roman character set. `lftest` converts between it and
UTF-8; characters Mac Roman lacks become `?`.

### Teletekst on the Mac

The **Teletekst** app finds the server by itself and shows page 100. Type a
page number, or click one on the page. The four coloured keys (red, green,
yellow, blue) jump to the pages named in the bottom row of the page: press
⌘1–⌘4 (or Shift-1–4), or click that row, one quarter per key. Other keys
go to the service as typed; `?` shows its help. W, A, S and D work as the
arrow keys (the original Macintosh keyboard has none): A and D go to the
previous and next page; W and S send up and down, which the service uses
for subpages.

Behind it, `lftest serve` runs `ssh teletekst.nl` in a 40×26 terminal for
each Mac (a page is 25 rows; the service adds a status line) and keeps the
screen in a terminal emulator. The Mac receives only the rows that changed,
so the clock in the header costs one row a second. Each cell carries its
two Teletekst colours (of 8); the Mac draws them as dither patterns of 8
grey levels, ordered by brightness (what is bright on a TV is dark ink on
the Mac). Block graphics are sent as 2×3 patterns and filled with their
colour's pattern. Text is solid black or white for contrast, on a solid
cell where the background is dithered. `ssh` must be
installed on the PC; no login or key is needed, and `lftest` offers none.
A session closes a minute after its Mac stops asking.

`--teletekst HOST` points `serve` at another ssh server.

### Teletekst on the Apple IIgs

[client_iigs/](client_iigs/README.md) is the same app for the IIgs, in
65816 assembly: `./build.sh` there gives `build/Teletekst.po`, an 800K
ProDOS disk. It shows the page in the eight real Teletekst colours on the
Super Hi-Res screen and speaks the same protocol, through the IIgs's own
AppleTalk firmware. Its README covers building, the keys, and what is not
yet verified on hardware.

### Tanks on the Mac

**Tanks** shows a tank-level dashboard on the whole screen: the iotta logo
and the weather at the top, then a card per tank with its name, its status
(Niveau stabiel, or Laden ▲ / Lossen ▼), a glass cylinder filled to its
level with the percentage on it, and the litres loaded, unloaded and the
level for today and yesterday. The time of the data is at the bottom left;
connection problems show at the bottom right. **⌘D** (or **D**) switches
between dithered shading (a highlighted, rounded cylinder) and flat
patterns. Esc or ⌘Q quits.

`lftest serve` fetches the data from a web dashboard that serves
`api/levels` and `api/weather` below its address, every 5 seconds, and
offers the `Tanks` service only when that address is in the environment
variable **`BLD_URL`**:

```sh
export BLD_URL=...        # the dashboard's address; keep it out of scripts in git
lftest serve
```

The address contains an access token, so it is not in the code, the docs
or the logs: `serve` reports only that the service runs "from the
dashboard in $BLD_URL", and its error messages leave the address out. Times
are shown in Dutch time, as on the dashboard's own page. `lftest tanks`
shows the same data in a terminal.

### `lftest ping`, `lftest chat` and `lftest teletekst`: PC clients

The same tests and chat as LFTest, from a PC: stage 1, or a second chat
participant.

```
$ lftest ping
we are node 21
found testbox:LFEcho@* at 0.239:250
ping 0: 32 bytes, 14.0 ms, server sees us as 0.21
…
pings: 10/10 ok
bulk: 92480 bytes in 5.16 s = 17.9 KB/s, 0 bad packets, 0 failed transactions

$ lftest chat --name tester
$ lftest teletekst          # shows the page; type 101 and Enter, or !, @, #, $
$ lftest tanks              # prints the tank dashboard each time it changes
```

### `lftest monitor`: see the traffic

```sh
lftest monitor --iface 192.168.1.147
```

Prints every LToUDP frame the machine receives, with the sender's IP
address and the frame decoded:

```
  0.504 192.168.1.147:1954    id b2a24e94  LLAP ENQ  for node 59
  0.712 192.168.1.147:1954    id b2a24e94  DDP 0.59:2 → broadcast:2  NBP lookup =:LFEcho@* (id 163, reply to 0.59:2)
```

It never sends anything. When a lookup finds nothing, run it on both
machines: it shows whether the frames leave one and arrive at the other,
and from which address.

## Common options

All `lftest` subcommands take these options:

| Option | Default | Meaning |
|---|---|---|
| `--name NAME` | host name | server name (`serve`) or chat nickname (`chat`) |
| `--node N` | from NAME (`serve`), random (clients) | LLAP node ID to try first: 128–254 for `serve`, 1–127 for `ping` and `chat` |
| `--rate BPS` | 30000 | pace outgoing frames to BPS bytes/s. Not much above LocalTalk's 28800, and not 0 (unpaced): see [Pacing](#pacing) |
| `--iface IP` | OS choice | address of the interface to use for multicast |

## Pacing

A PC can send the 8 packets of an ATP response in microseconds; LocalTalk
needs about 21 ms for each. Frames that arrive faster queue up in the
emulator or the bridge. Once that queue is full they are lost, and ATP only
notices after a 2-second timeout. So sending too fast makes transfers
**slower**. Measured bulk throughput in Snow, Mac Plus at 1× speed:

| `--rate` | Bulk throughput |
|---|---|
| 15000 | 10.5 KB/s |
| 28000 | 18.4 KB/s |
| 38000 | 18.4 KB/s (identical: Snow buffered the burst) |
| 100000 | 2.7 KB/s (Snow's buffer overflowed) |

18.4 KB/s is the ceiling with one transaction at a time: each one takes
252 ms, of which about 167 ms is 8 packets on the (emulated) wire and about
85 ms is turnaround (request, release, the Mac's processing). Two
transactions in flight could hide the turnaround. The default, 30000, is
just above wire speed: the link runs flat out, and bursts queue only
slightly.

## Protocols and numbers

| Service | NBP type | Socket | Protocol description |
|---|---|---|---|
| Echo | `LFEcho` | 250 | [`server/lftest/src/echo.rs`](server/lftest/src/echo.rs) |
| Chat hub | `RChat` | 251 | [`server/lftest/src/chat.rs`](server/lftest/src/chat.rs) |
| Teletekst | `Teletekst` | 252 | [`server/lftest/src/teletekst.rs`](server/lftest/src/teletekst.rs) |
| Tanks (only with `BLD_URL` set) | `Tanks` | 253 | [`server/lftest/src/tanks.rs`](server/lftest/src/tanks.rs) |
| Requests from PC clients | – | 254 | – |

All services use exactly-once ATP transactions. The chat uses the pull
model the remote desktop will use: the client always has one request
outstanding, and the hub holds it (up to 2 s) until there is something to
send.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| LFTest: error −97 or −98 at start | AppleTalk is inactive: Chooser → AppleTalk Active |
| Nothing found in the lookup | `lftest serve` is not running; the emulator's LToUDP bridge is off; a firewall blocks UDP 1954; multicast leaves on the wrong adapter (see [Networking notes](#networking-notes)). `lftest monitor` shows which |
| `lftest ping` finds nothing, on one PC | another program may hold port 1954 exclusively: stop it, or run the tools on another PC |
| Pings work, bulk fails through picotalk | pacing too fast for the bridge: lower `--rate` |
| Error −1096 (reqFailed) | the server stopped answering: it quit, or packets are being lost |

## Documentation

* [docs/appletalk-primer.md](docs/appletalk-primer.md): AppleTalk 101 —
  AppleTalk versus LocalTalk, LLAP, DDP, NBP, ATP, LToUDP, a worked example
  byte by byte, and the Mac's AppleTalk Manager.
* [docs/remote-desktop-protocol.md](docs/remote-desktop-protocol.md): the
  remote desktop design.
