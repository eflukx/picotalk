*
* Teletekst for the Apple IIgs: NOS Teletekst from `lftest serve`, which
* bridges to `ssh teletekst.nl`, over AppleTalk.
*
* The IIgs version of localframe/client/src/teletekst.c, in 65816
* assembly (Merlin 32). The protocol is described in
* localframe/server/lftest/src/teletekst.rs:
*
*   POLL 03 00 have(2) -> 03 st seq(2) nrows 00 { row 40 x (char attr) }
*   KEY  02 00 keys    -> 02 st
*
* The page fills the Super Hi-Res screen in 320 mode: 40 x 25 cells of
* 8 x 8 pixels, in the 8 Teletekst colours of palette 0. The service's
* 26th row (its status line) is not shown.
*
* AppleTalk is called as the IIgs AppleTalk firmware expects (see
* ../README.md): the parameter block's address in X (low word) and Y
* (bank), then JSL RamDispatch. The blocks have the same layout as the
* ProDOS 8 AppleTalk calls ($42): NBPLookup is $10, SendATPReq $12.
*
* Keys: type a page number; the arrow keys (and W A S D) are the arrow
* keys; Apple-1 to Apple-4 (or ! @ # $) are the red, green, yellow and
* blue keys; Esc or Apple-Q quits.
*

         mx    %00

* ---- constants ----

GSOS     equ   $E100A8          ; GS/OS inline call entry
TOOLS    equ   $E10000          ; Tool Locator dispatcher
RAMDISP  equ   $E11014          ; AppleTalk: RamDispatch

KBD      equ   $E0C000          ; keyboard data, bit 7 = a key is waiting
KBDSTRB  equ   $E0C010          ; clears the keyboard strobe
KEYMODS  equ   $E0C025          ; bit 7 = Open Apple
NEWVIDEO equ   $E0C029          ; bit 7 = Super Hi-Res, bit 6 = linear
CLOCKCTL equ   $E0C034          ; bits 0-3 = border colour

SHR      equ   $E12000          ; pixels: 200 lines of 160 bytes
SCBS     equ   $E19D00          ; one scan-line control byte per line
PALETTE  equ   $E19E00          ; palette 0: 16 words $0RGB

CMD_KEY  equ   2
CMD_POLL equ   3
ROWS     equ   25               ; page rows shown (the service sends 26)
ROWLEN   equ   81               ; row number + 40 x (char, attr)
POLLBUFS equ   4                ; response packets a POLL allows
ATPMAX   equ   578              ; data bytes in one ATP packet
MAXKEYS  equ   32
WHITE    equ   $0700            ; attr white on black, in the high byte

AT_NBPLOOKUP equ $10
AT_SENDATP   equ $12
AT_ASYNC equ   $80
ATP_XO   equ   $20
DEV_APPLETALK equ $1D           ; GS/OS deviceID of the AppleTalk driver

* states
ST_OFF   equ   0                ; no AppleTalk: only Esc works
ST_WAIT  equ   1                ; looking up the server at tick `next`
ST_FIND  equ   2                ; the lookup runs
ST_RUN   equ   3                ; polling the server

* ---- direct page ----

scr      equ   $00              ; long: the cell being drawn
diff     equ   $04              ; (ink EOR paper) in every nibble
paper    equ   $06              ; paper colour in every nibble
src      equ   $08              ; long: a row in the POLL response
tmp      equ   $0C
tmp2     equ   $0E
left     equ   $10              ; POLL response bytes not parsed yet
nrows    equ   $12
pbp      equ   $14              ; the parameter block being called
msgp     equ   $16              ; message(): the string table
mrow     equ   $18              ; message(): the row
txtp     equ   $1A              ; drawtext(): the string

* One line of a glyph, half a cell wide: ]1 = its offset in the glyph,
* ]2 = its offset on the screen.
line     mac
         lda   glyphs+]1,x
         and   diff
         eor   paper
         ldy   #]2
         sta   [scr],y
         <<<

* ---- start ----

start    phk
         plb
         sta   userid           ; the loader passes our user ID in A
         phk
         phk
         pla
         and   #$00FF
         sta   mybank

         ldx   #$0201           ; TLStartUp
         jsl   TOOLS
         pha
         ldx   #$0202           ; MMStartUp
         jsl   TOOLS
         pla
         sta   userid
         ldx   #$0203           ; MTStartUp
         jsl   TOOLS

* Claim the Super Hi-Res screen, $E1/2000-$9FFF, so nothing else is
* put there. If it is taken we draw anyway, as games do.
         pha
         pha
         pea   $0000
         pea   $8000            ; size
         lda   userid
         pha
         pea   $C003            ; locked, fixed, fixed bank and address
         pea   $00E1
         pea   $2000
         ldx   #$0902           ; NewHandle
         jsl   TOOLS
         pla
         sta   scrhand
         pla
         sta   scrhand+2
         bcc   :claimed
         stz   scrhand
         stz   scrhand+2
:claimed
         jsr   screenon
         jsr   findat
         bcc   :haveat
         jsr   noatscr
         lda   #ST_OFF
         sta   state
         bra   main
:haveat  ldx   #msg_find
         jsr   message
         lda   #ST_WAIT
         sta   state
         jsr   ticks
         sta   next

main     jsr   keyboard
         jsr   network
         lda   quitting
         beq   main
         lda   pollbusy
         ora   keybusy
         ora   lookbusy
         bne   main             ; wait for requests that write into our memory

* ---- quit ----

         jsr   screenoff
         lda   scrhand
         ora   scrhand+2
         beq   :nohand
         lda   scrhand+2
         pha
         lda   scrhand
         pha
         ldx   #$1002           ; DisposeHandle
         jsl   TOOLS
:nohand  ldx   #$0303           ; MTShutDown
         jsl   TOOLS
         lda   userid
         pha
         ldx   #$0302           ; MMShutDown
         jsl   TOOLS
         ldx   #$0301           ; TLShutDown
         jsl   TOOLS
         jsl   GSOS
         da    $2029            ; Quit
         adrl  quitparm
         brk   $00              ; not reached

quitparm dw    0                ; pCount

* ---- time ----

* Returns the low word of the tick counter (60 Hz) in A.
ticks    pha
         pha
         ldx   #$2503           ; GetTick
         jsl   TOOLS
         pla
         ply
         rts

* Carry set if the tick counter has reached `next`.
due      jsr   ticks
         sec
         sbc   next
         bmi   :no
         sec
         rts
:no      clc
         rts

* Sets `next` to A ticks from now.
later    sta   tmp
         jsr   ticks
         clc
         adc   tmp
         sta   next
         rts

* ---- AppleTalk ----

* Carry clear if the AppleTalk driver is present: GS/OS lists it as a
* device with deviceID $1D (Apple II AppleTalk Technical Note #1).
findat   lda   #1
         sta   dinum
:next    jsl   GSOS
         da    $202C            ; DInfo
         adrl  dinfo
         bcc   :check
         cmp   #$11             ; invalid device number: past the last
         beq   :none
         cmp   #$53             ; parameter out of range: likewise
         beq   :none
         bra   :skip            ; another error, e.g. a long name
:check   lda   diid
         cmp   #DEV_APPLETALK
         beq   :found
:skip    inc   dinum
         lda   dinum
         cmp   #64
         bcc   :next
:none    sec
         rts
:found   clc
         rts

dinfo    dw    8                ; pCount
dinum    dw    1                ; devNum
         adrl  diname           ; devName
         dw    0                ; characteristics
         adrl  0                ; totalBlocks
         dw    0                ; slotNum
         dw    0                ; unitNum
         dw    0                ; version
diid     dw    0                ; deviceID
diname   dw    36               ; result buffer: size, then the name
         ds    34

* Makes the AppleTalk call whose parameter block is at X (in our bank),
* asynchronously. Returns carry set if it failed at once; otherwise A is
* the block's result word while the call runs, which network compares
* against to see the call finish.
*
* How the firmware marks a running call in the result word is not
* documented here (ASP uses $07FF), so the value is taken as it stands
* right after the call and the call is done when it changes. The result
* is set to $FFFF first, in case the firmware writes it only at the end.
atasync  stx   pbp
         ldy   #2
         lda   #$FFFF
         sta   (pbp),y
         phb
         phd
         ldy   mybank
         jsl   RAMDISP
         rep   #$30
         pld
         plb
         bcs   :fail
         ldy   #2
         lda   (pbp),y
         clc
:fail    rts

* ---- network ----

network  lda   pollbusy
         beq   :key
         lda   pollres
         cmp   pollbusyv
         beq   :key
         stz   pollbusy
         lda   quitting
         bne   :key
         jsr   onpoll

:key     lda   keybusy
         beq   :look
         lda   keyres
         cmp   keybusyv
         beq   :look
         stz   keybusy

:look    lda   lookbusy
         beq   :quit
         lda   lookres
         cmp   lookbusyv
         beq   :quit
         stz   lookbusy
         lda   quitting
         bne   :quit
         jsr   onlookup

:quit    lda   quitting
         beq   :wait
         rts

:wait    lda   state
         cmp   #ST_WAIT
         bne   :run
         lda   pollbusy
         ora   keybusy
         bne   :run
         jsr   due
         bcc   :run
         jsr   lookup

:run     lda   state
         cmp   #ST_RUN
         bne   :keys
         lda   pollbusy
         bne   :keys
         jsr   poll
:keys    jmp   sendkeys

lookup   ldx   #lookpb
         jsr   atasync
         bcs   :fail
         sta   lookbusyv
         lda   #1
         sta   lookbusy
         lda   #ST_FIND
         sta   state
         rts
:fail    lda   #60
         jmp   later

onlookup lda   lookres
         bne   :none
         lda   looknum
         and   #$00FF
         beq   :none
         lda   lookbuf          ; network, node, socket
         sta   polladdr
         sta   keyaddr
         lda   lookbuf+2
         sta   polladdr+2
         sta   keyaddr+2
         stz   have
         lda   #ST_RUN
         sta   state
         rts
:none    ldx   #msg_none
         jsr   message
         lda   #ST_WAIT
         sta   state
         lda   #120
         jmp   later

poll     lda   have
         xba                    ; big-endian
         sta   pollreq+2
         stz   pollbds+10       ; actual lengths
         stz   pollbds+22
         stz   pollbds+34
         stz   pollbds+46
         ldx   #pollpb
         jsr   atasync
         bcs   lost
         sta   pollbusyv
         lda   #1
         sta   pollbusy
         rts

* The server stopped answering: look it up again.
lost     ldx   #msg_lost
         jsr   message
         stz   have
         lda   #ST_WAIT
         sta   state
         lda   #60
         jmp   later

onpoll   lda   pollres
         bne   lost
         lda   pollbds+10
         clc
         adc   pollbds+22
         adc   pollbds+34
         adc   pollbds+46
         sec
         sbc   #6               ; the header
         bmi   :done
         sta   left
         lda   pollbuf+1        ; status
         and   #$00FF
         bne   :done
         lda   pollbuf+2
         xba
         sta   have
         lda   pollbuf+4
         and   #$00FF
         sta   nrows
         lda   #pollbuf+6
         sta   src
         lda   mybank
         sta   src+2
:row     lda   nrows
         beq   :done
         dec   nrows
         lda   left
         sec
         sbc   #ROWLEN
         bmi   :done
         sta   left
         lda   [src]
         and   #$00FF
         cmp   #ROWS
         bcs   :skip
         jsr   drawrow
:skip    lda   src
         clc
         adc   #ROWLEN
         sta   src
         bra   :row
:done    rts

* ---- keyboard ----

keyboard sep   #$20
         mx    %10
         ldal  KBD
         bmi   :key
         rep   #$20
         mx    %00
         rts
         mx    %10
:key     and   #$7F
         xba
         ldal  KEYMODS
         stal  KBDSTRB
         rep   #$20
         mx    %00
         xba                    ; A = key, high byte = modifiers
         bit   #$8000
         bne   :apple
         and   #$00FF
         cmp   #$1B             ; Esc
         beq   :quit
         sta   tmp
         ldx   #0
:map     lda   keymap,x         ; translations, 0-terminated
         and   #$00FF
         beq   :plain0
         cmp   tmp
         beq   :mapped
         inx
         inx
         bra   :map
:mapped  lda   keymap+1,x
         and   #$00FF
         bra   :add
:plain0  lda   tmp
:plain   cmp   #$7F             ; Delete
         beq   :add
         cmp   #$0D             ; Return
         beq   :add
         cmp   #$20
         bcc   :done
         bra   :add

:apple   and   #$00FF
         ora   #$20             ; lower case
         cmp   #'q'
         beq   :quit
         sec
         sbc   #'1'
         cmp   #4
         bcs   :done
         tax
         lda   colkeys,x
         and   #$00FF
:add     ldx   state
         cpx   #ST_RUN
         bne   :done
         ldx   nkeys
         cpx   #MAXKEYS
         bcs   :done
         sep   #$20
         mx    %10
         sta   keys,x
         rep   #$20
         mx    %00
         inc   nkeys
:done    rts
:quit    lda   #1
         sta   quitting
         ldx   #msg_quit
         jmp   message

* key, what the server gets: the IIgs arrow keys and W A S D become the
* Mac's arrow keys (1C left, 1D right, 1E up, 1F down).
keymap   db    $08,$1C,$15,$1D,$0B,$1E,$0A,$1F
         db    'a',$1C,'d',$1D,'w',$1E,'s',$1F
         db    'A',$1C,'D',$1D,'W',$1E,'S',$1F
         db    0,0
colkeys  asc   '!@#$'           ; red, green, yellow, blue

sendkeys lda   keybusy
         bne   :done
         lda   nkeys
         beq   :done
         ldx   state
         cpx   #ST_RUN
         bne   :done
         clc
         adc   #2
         sta   keyreqln
         ldx   #0
:copy    lda   keys,x
         sta   keyreq+2,x
         inx
         inx
         cpx   nkeys
         bcc   :copy
         stz   nkeys
         ldx   #keypb
         jsr   atasync
         bcs   :done            ; the keys are lost; the next ones may go
         sta   keybusyv
         lda   #1
         sta   keybusy
:done    rts

* ---- screen ----

screenon sep   #$20
         mx    %10
         ldal  CLOCKCTL
         sta   oldclock
         and   #$F0             ; black border
         stal  CLOCKCTL
         rep   #$20
         mx    %00
         jsr   clear
         lda   #0
         ldx   #198
:scb     stal  SCBS,x           ; 320 mode, palette 0
         dex
         dex
         bpl   :scb
         ldx   #30
:pal     lda   colours,x
         stal  PALETTE,x
         dex
         dex
         bpl   :pal
         sep   #$20
         mx    %10
         ldal  NEWVIDEO
         sta   oldvideo
         ora   #$C0
         stal  NEWVIDEO
         rep   #$20
         mx    %00
         rts

screenoff sep  #$20
         mx    %10
         lda   oldvideo
         stal  NEWVIDEO
         ldal  CLOCKCTL
         and   #$F0
         sta   tmp
         lda   oldclock
         and   #$0F
         ora   tmp
         stal  CLOCKCTL
         rep   #$20
         mx    %00
         rts

clear    lda   #0
         ldx   #32000-2
:loop    stal  SHR,x
         dex
         dex
         bpl   :loop
         rts

* The Teletekst colours: bit 0 red, 1 green, 2 blue.
colours  dw    $0000,$0F00,$00F0,$0FF0,$000F,$0F0F,$00FF,$0FFF
         ds    16

* A colour in every nibble of a word.
nibbles  dw    $0000,$1111,$2222,$3333,$4444,$5555,$6666,$7777

* Points scr at row A, column 0.
rowaddr  xba                    ; row x 256
         sta   tmp
         asl
         asl                    ; row x 1024
         clc
         adc   tmp              ; row x 1280 = 8 lines of 160 bytes
         adc   #$2000           ; SHR
         sta   scr
         lda   #$00E1
         sta   scr+2
         rts

* Draws row [src] (its number, then 40 x (char, attr)).
drawrow  lda   [src]
         and   #$00FF
         jsr   rowaddr
         ldy   #1
:cell    lda   [src],y
         phy
         jsr   drawcell
         ply
         iny
         iny
         cpy   #ROWLEN
         bcc   :cell
         rts

* Draws the cell A (char in the low byte, attr in the high byte) at scr,
* and moves scr to the next cell.
drawcell sta   tmp2
         xba
         and   #$0007           ; ink
         asl
         tax
         lda   nibbles,x
         sta   diff
         lda   tmp2
         xba
         lsr
         lsr
         and   #$000E           ; paper x 2
         tax
         lda   nibbles,x
         sta   paper
         eor   diff
         sta   diff
         lda   tmp2
         bit   #$4000           ; MOSAIC
         beq   :char
         and   #$003F
         ora   #$0100
         bra   :glyph
:char    and   #$00FF
:glyph   asl
         asl
         asl
         asl
         asl                    ; x 32 bytes a glyph
         tax
         line  0;0
         line  2;2
         line  4;160
         line  6;162
         line  8;320
         line  10;322
         line  12;480
         line  14;482
         line  16;640
         line  18;642
         line  20;800
         line  22;802
         line  24;960
         line  26;962
         line  28;1120
         line  30;1122
         lda   scr
         clc
         adc   #4
         sta   scr
         rts

* Clears the screen and shows up to three centred lines: X points at
* three string addresses (0 for none).
message  stx   msgp
         jsr   clear
         lda   #10
         sta   mrow
         ldy   #0
:line    lda   (msgp),y
         beq   :next
         phy
         tax
         lda   mrow
         jsr   drawtext
         ply
:next    lda   mrow
         inc
         inc
         sta   mrow
         iny
         iny
         cpy   #6
         bcc   :line
         rts

* Draws the 0-terminated string X at row A, centred, white on black.
drawtext stx   txtp
         jsr   rowaddr
         ldy   #0
:len     lda   (txtp),y
         and   #$00FF
         beq   :centre
         iny
         bra   :len
:centre  sty   tmp2
         lda   #40
         sec
         sbc   tmp2
         and   #$FFFE           ; (40 - len) / 2 cells of 4 bytes
         asl
         clc
         adc   scr
         sta   scr
         ldy   #0
         bra   drawstr

* Draws the 0-terminated string X at row A, from column 1.
drawleft stx   txtp
         jsr   rowaddr
         lda   scr
         clc
         adc   #4
         sta   scr
         ldy   #0
drawstr
:char    lda   (txtp),y
         and   #$00FF
         beq   :done
         ora   #WHITE
         phy
         jsr   drawcell
         ply
         iny
         bra   :char
:done    rts

* No AppleTalk: says so, and lists the devices GS/OS knows (number,
* device ID, name), to show what is there instead.
noatscr  jsr   clear
         lda   #1
         ldx   #t_noat
         jsr   drawtext
         lda   #2
         ldx   #t_noat2
         jsr   drawtext
         lda   #4
         ldx   #t_devs
         jsr   drawtext
         lda   #24
         ldx   #t_esc
         jsr   drawtext
         lda   #1
         sta   dinum
         lda   #6
         sta   mrow
:next    jsl   GSOS
         da    $202C            ; DInfo
         adrl  dinfo
         sta   tmp2             ; the error, if any
         bcc   :ok
         cmp   #$11             ; past the last device
         beq   :done
         cmp   #$53
         beq   :done
         sec                    ; an error: cmp changed the carry
:ok      php
         ldx   #0
         lda   dinum
         jsr   hexbyte
         lda   #' '
         jsr   putch
         plp
         bcc   :dev
         ldy   #0               ; "fout" and the error code
:fout    lda   t_fout,y
         and   #$00FF
         beq   :code
         jsr   putch
         iny
         bra   :fout
:code    lda   tmp2
         jsr   hexword
         bra   :show
:dev     lda   #'$'
         jsr   putch
         lda   diid
         jsr   hexword
         lda   #' '
         jsr   putch
         lda   diname+2         ; the name's length
         cmp   #31
         bcc   :len
         lda   #30
:len     sta   tmp
         ldy   #0
:name    cpy   tmp
         bcs   :show
         lda   diname+4,y
         jsr   putch
         iny
         bra   :name
:show    lda   #0
         jsr   putch
         lda   mrow
         ldx   #linebuf
         jsr   drawleft
         inc   mrow
         inc   dinum
         lda   mrow
         cmp   #23
         bcs   :done
         jmp   :next
:done    rts

* Appends A as four hex digits to linebuf at X.
hexword  pha
         xba
         jsr   hexbyte
         pla
* Appends the low byte of A as two hex digits.
hexbyte  pha
         lsr
         lsr
         lsr
         lsr
         jsr   hexdig
         pla
hexdig   and   #$000F
         tay
         lda   hexchars,y
* Appends the low byte of A to linebuf at X, and advances X.
putch    sep   #$20
         mx    %10
         sta   linebuf,x
         rep   #$20
         mx    %00
         inx
         rts

hexchars asc   '0123456789ABCDEF'
linebuf  ds    40

* ---- messages ----

msg_find dw    t_title,t_find,0
msg_none dw    t_none,t_none2,0
msg_lost dw    t_lost,t_lost2,0
msg_quit dw    t_title,t_quit,0

t_title  asc   'Teletekst'
         db    0
t_find   asc   'Zoeken naar de server...'
         db    0
t_noat   asc   'AppleTalk staat uit.'
         db    0
t_noat2  asc   'Zet AppleTalk aan in het Control Panel.'
         db    0
t_esc    asc   'Esc: stoppen'
         db    0
t_devs   asc   'GS/OS-apparaten (AppleTalk = $001D):'
         db    0
t_fout   asc   'fout $'
         db    0
t_none   asc   'Geen Teletekst-server gevonden.'
         db    0
t_none2  asc   'Draait `lftest serve` op de PC?'
         db    0
t_lost   asc   'De Teletekst-server antwoordt niet.'
         db    0
t_lost2  asc   'Opnieuw zoeken...'
         db    0
t_quit   asc   'Stoppen...'
         db    0

* ---- variables ----

userid   dw    0
mybank   dw    0
scrhand  adrl  0
oldvideo db    0
oldclock db    0
state    dw    0
next     dw    0
quitting dw    0
have     dw    0                ; the last screen applied
pollbusy dw    0
pollbusyv dw   0
keybusy  dw    0
keybusyv dw    0
lookbusy dw    0
lookbusyv dw   0
nkeys    dw    0
keys     ds    MAXKEYS+2

* ---- AppleTalk parameter blocks ----

* NBPLookup =:Teletekst@*, asynchronous.
lookpb   db    AT_ASYNC,AT_NBPLOOKUP
lookres  dw    0                ; result
         adrl  0                ; completion routine
         adrl  entity           ; the name to look up
         db    4                ; retry interval, in 1/4 s
         db    4                ; retries
         dw    0                ; reserved
         dw    128              ; buffer size
         adrl  lookbuf
         db    1                ; matches wanted
looknum  db    0                ; matches found

entity   db    1
         asc   '='
         db    9
         asc   'Teletekst'
         db    1
         asc   '*'

* Each match: network (2), node, socket, enumerator, then the name.
lookbuf  ds    128

* SendATPReq: POLL, exactly once, up to 4 response packets.
pollpb   db    AT_ASYNC,AT_SENDATP
pollres  dw    0
         adrl  0                ; completion routine
         db    0                ; socket (0: any)
polladdr ds    4                ; network, node, socket
         dw    0                ; TID
         dw    4                ; request length
         adrl  pollreq
         adrl  0                ; user bytes
         db    POLLBUFS         ; response buffers
         adrl  pollbds
         db    ATP_XO
         db    16               ; retry interval, in 1/4 s: over the 2 s hold
         db    3                ; retries
         db    $0F              ; bitmap: packets 0-3
         db    0                ; responses received
         ds    6                ; reserved

pollreq  db    CMD_POLL,0
         dw    0                ; have, big-endian

* Response buffers: length, address, user bytes, actual length. They are
* back to back, and every packet but the last is full, so the response
* arrives as one run of bytes.
pollbds  dw    ATPMAX
         adrl  pollbuf
         adrl  0
         dw    0
         dw    ATPMAX
         adrl  pollbuf+ATPMAX
         adrl  0
         dw    0
         dw    ATPMAX
         adrl  pollbuf+ATPMAX+ATPMAX
         adrl  0
         dw    0
         dw    ATPMAX
         adrl  pollbuf+ATPMAX+ATPMAX+ATPMAX
         adrl  0
         dw    0

* SendATPReq: KEY.
keypb    db    AT_ASYNC,AT_SENDATP
keyres   dw    0
         adrl  0
         db    0
keyaddr  ds    4
         dw    0
keyreqln dw    0
         adrl  keyreq
         adrl  0
         db    1
         adrl  keybds
         db    ATP_XO
         db    8                ; retry interval: 2 s
         db    3
         db    $01
         db    0
         ds    6

keyreq   db    CMD_KEY,0
         ds    MAXKEYS

keybds   dw    16
         adrl  keybuf
         adrl  0
         dw    0
keybuf   ds    16

         put   font.s

pollbuf  ds    POLLBUFS*ATPMAX
