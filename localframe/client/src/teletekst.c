/*
 * Teletekst: NOS Teletekst on a classic Mac, from `lftest serve`, which
 * bridges to `ssh teletekst.nl`. The protocol is described in
 * localframe/server/lftest/src/teletekst.rs:
 *   POLL 03 00 have(2) → the rows that changed since screen `have`
 *   KEY  02 00 keys    → keys for the session
 *
 * Type a page number, or click one; Cmd-1 to Cmd-4 (and Shift-1 to
 * Shift-4, i.e. ! @ # $) are the red, green, yellow and blue keys, which
 * a click on the bottom page row gives too.
 */
#include <string.h>

#include "appletalk.h"
#include "textwin.h"

#define TT_TYPE "Teletekst"
enum { CMD_KEY = 2, CMD_POLL = 3 };
#define ST_OK 0

#define COLS 40
#define ROWS 26         /* 25 page rows and the service's status line */
#define FASTEXT_ROW 24  /* the row naming the colour keys' pages */
#define MOSAIC 0x40     /* attribute: the character is a 2×3 block pattern */
#define FG(a) ((a) & 7)  /* attribute: the 8 Teletekst colours, */
#define BG(a) (((a) >> 3) & 7) /* bit 0 red, 1 green, 2 blue */
#define WHITE_ON_BLACK 7
#define POLL_PACKETS 4  /* a full screen is about 2 KB */
#define POLL_TIMEOUT 4  /* s; the server holds a POLL for 2 s */
#define MAX_KEYS 32

enum { NET_OFF = -1, NET_LOOKUP, NET_FINDING, NET_RUNNING };

static WindowPtr win;
static short cw, ch, ascent; /* cell size */
static short left0, top0;    /* the page's top left corner */

/* The page as the server last sent it: character and attribute. */
static unsigned char cells[ROWS][COLS][2];

static short state = NET_LOOKUP;
static long nextLookup;
static NBPLookup look;
static ATAddr server;
static unsigned short have;

static ATPRequest pollR, keyR;
static Boolean pollBusy, keyBusy;
static unsigned char pollReq[4], pollBuf[POLL_PACKETS * ATP_MAX_DATA];
static unsigned char keyReq[2 + MAX_KEYS], keyBuf[ATP_MAX_DATA];
static char keys[MAX_KEYS];
static short nkeys;

/* ---- drawing ---- */

/* Each colour's grey level, 0 (white paper) to 7 (solid black): what is
 * bright on a TV becomes dark ink. Ordered by brightness: black, blue,
 * red, magenta, green, cyan, yellow, white. */
static const unsigned char level[8] = {0, 2, 4, 6, 1, 3, 5, 7};
static Pattern grey[8];

/* Ordered (Bayer) dither patterns for the 8 grey levels. They line up with
 * the window, so neighbouring cells of one colour form a smooth area. */
static void make_patterns(void)
{
    static const unsigned char bayer[4][4] = {{0, 8, 2, 10}, {12, 4, 14, 6}, {3, 11, 1, 9}, {15, 7, 13, 5}};
    short l, y, x;
    for (l = 0; l < 8; l++) {
        short threshold = (l * 16 + 3) / 7;
        for (y = 0; y < 8; y++) {
            unsigned char bits = 0;
            for (x = 0; x < 8; x++)
                if (bayer[y & 3][x & 3] < threshold)
                    bits |= 0x80 >> x;
            grey[l].pat[y] = bits;
        }
    }
}

static void cell_rect(short r, short c, Rect *rect)
{
    SetRect(rect, left0 + c * cw, top0 + r * ch, left0 + (c + 1) * cw, top0 + (r + 1) * ch);
}

/* One 2×3 block-graphics cell: bit 0 top left, 1 top right, … 5 bottom right. */
static void draw_mosaic(short r, short c, unsigned char pattern, const Pattern *ink)
{
    Rect cell, b;
    short i;
    cell_rect(r, c, &cell);
    for (i = 0; i < 6; i++) {
        short row = i >> 1;
        if (!(pattern & (1 << i)))
            continue;
        b.left = (i & 1) ? cell.left + cw / 2 : cell.left;
        b.right = (i & 1) ? cell.right : cell.left + cw / 2;
        b.top = cell.top + row * ch / 3;
        b.bottom = cell.top + (row + 1) * ch / 3;
        FillRect(&b, ink);
    }
}

/* Text on a background of grey level `bg`: black on light backgrounds,
 * white on dark ones. On a dithered background each letter first gets a
 * solid cell behind it (white behind black text, black behind white), so
 * it does not merge with the dots; the gaps between letters keep the
 * pattern. */
static void draw_text(char *run, short n, short x, short y, short bg)
{
    Boolean black = bg <= 3;
    if (bg != 0 && bg != 7) {
        short i;
        for (i = 0; i < n; i++) {
            Rect cell;
            if (run[i] == ' ')
                continue;
            SetRect(&cell, x + i * cw, y - ascent, x + (i + 1) * cw, y - ascent + ch);
            if (black)
                EraseRect(&cell);
            else
                PaintRect(&cell);
        }
    }
    TextMode(black ? srcOr : srcBic);
    MoveTo(x, y);
    DrawText(run, 0, n);
}

static void draw_row(short r)
{
    Rect rr;
    short c;
    GrafPtr old;

    GetPort(&old);
    SetPort(win);
    /* Backgrounds, in runs of one colour. */
    for (c = 0; c < COLS;) {
        short start = c;
        unsigned char b = BG(cells[r][c][1]);
        while (c < COLS && BG(cells[r][c][1]) == b)
            c++;
        SetRect(&rr, left0 + start * cw, top0 + r * ch, left0 + c * cw, top0 + (r + 1) * ch);
        FillRect(&rr, &grey[level[b]]);
    }
    /* Block graphics in their colour's pattern. Text is solid black or
     * white, whichever stands out against the background: dithered
     * letters of 6×11 pixels would be unreadable. */
    for (c = 0; c < COLS;) {
        unsigned char attr = cells[r][c][1];
        if (attr & MOSAIC) {
            draw_mosaic(r, c, cells[r][c][0], &grey[level[FG(attr)]]);
            c++;
        } else {
            char run[COLS];
            short n = 0, start = c;
            while (c < COLS && cells[r][c][1] == attr)
                run[n++] = (char)cells[r][c++][0];
            if (FG(attr) == BG(attr))
                continue; /* hidden text */
            draw_text(run, n, left0 + start * cw, top0 + r * ch + ascent, level[BG(attr)]);
        }
    }
    TextMode(srcOr);
    SetPort(old);
}

static void draw_all(void)
{
    short r;
    EraseRect(&win->portRect);
    for (r = 0; r < ROWS; r++)
        draw_row(r);
}

/* Replaces the page with a message, centred. */
static void message(const char *line1, const char *line2)
{
    const char *lines[2];
    short i;
    lines[0] = line1;
    lines[1] = line2;
    for (i = 0; i < ROWS * COLS; i++) {
        cells[i / COLS][i % COLS][0] = ' ';
        cells[i / COLS][i % COLS][1] = WHITE_ON_BLACK;
    }
    for (i = 0; i < 2; i++) {
        short n = lines[i] ? (short)strlen(lines[i]) : 0, k;
        if (n > COLS)
            n = COLS;
        for (k = 0; k < n; k++)
            cells[11 + i][(COLS - n) / 2 + k][0] = (unsigned char)lines[i][k];
    }
}

/* ---- network ---- */

static void send_keys(void)
{
    if (keyBusy || nkeys == 0 || state != NET_RUNNING)
        return;
    keyReq[0] = CMD_KEY;
    keyReq[1] = 0;
    memcpy(keyReq + 2, keys, nkeys);
    keyBusy = at_request(&keyR, server, keyReq, 2 + nkeys, 0, keyBuf, 1, 2, true) == noErr;
    if (keyBusy)
        nkeys = 0;
}

static void add_keys(const char *k, short n)
{
    if (nkeys + n > MAX_KEYS) {
        SysBeep(10);
        return;
    }
    memcpy(keys + nkeys, k, n);
    nkeys += n;
    send_keys();
}

static void start_poll(void)
{
    pollReq[0] = CMD_POLL;
    pollReq[1] = 0;
    pollReq[2] = (unsigned char)(have >> 8);
    pollReq[3] = (unsigned char)have;
    pollBusy = at_request(&pollR, server, pollReq, 4, 0, pollBuf, POLL_PACKETS, POLL_TIMEOUT, true) == noErr;
}

static void lost(void)
{
    state = NET_LOOKUP;
    nextLookup = TickCount() + 60;
    have = 0;
}

static void on_poll(void)
{
    long len = at_response_length(&pollR);
    const unsigned char *p = pollBuf + 6;
    short n, i;

    if (pollR.h.ioResult != noErr) {
        message("De Teletekst-server antwoordt niet.", "Opnieuw zoeken...");
        draw_all();
        lost();
        return;
    }
    if (len < 6 || pollBuf[1] != ST_OK)
        return;
    have = (unsigned short)((pollBuf[2] << 8) | pollBuf[3]);
    n = pollBuf[4];
    for (i = 0; i < n && p + 1 + 2 * COLS <= pollBuf + len; i++) {
        short r = p[0], c;
        if (r < ROWS) {
            for (c = 0; c < COLS; c++) {
                cells[r][c][0] = p[1 + 2 * c];
                cells[r][c][1] = p[2 + 2 * c];
            }
            draw_row(r);
        }
        p += 1 + 2 * COLS;
    }
}

static void net_idle(void)
{
    if (pollBusy && at_done(&pollR)) {
        pollBusy = false;
        on_poll();
    }
    if (keyBusy && at_done(&keyR)) {
        keyBusy = false;
        send_keys();
    }
    if (state == NET_FINDING && at_lookup_done(&look)) {
        NBPResult found;
        if (at_lookup_results(&look, &found, 1) > 0) {
            server = found.addr;
            have = 0;
            state = NET_RUNNING;
        } else {
            message("Geen Teletekst-server gevonden.", "Draait `lftest serve` op de PC?");
            draw_all();
            state = NET_LOOKUP;
            nextLookup = TickCount() + 2 * 60;
        }
    }
    if (state == NET_LOOKUP && !pollBusy && !keyBusy && TickCount() >= nextLookup) {
        OSErr err = at_lookup_start(&look, "=", TT_TYPE, 1, true);
        if (err == noErr)
            state = NET_FINDING;
        else
            nextLookup = TickCount() + 60;
    }
    if (state == NET_RUNNING && !pollBusy)
        start_poll();
    send_keys();
}

static void net_close(void)
{
    long give_up = TickCount() + 10 * 60;
    if (pollBusy)
        at_cancel(&pollR);
    if (keyBusy)
        at_cancel(&keyR);
    while (((pollBusy && !at_done(&pollR)) || (keyBusy && !at_done(&keyR)) ||
            (state == NET_FINDING && !at_lookup_done(&look))) &&
           TickCount() < give_up)
        SystemTask();
}

/* ---- input ---- */

static const char colour_keys[] = "!@#$";

/* A click: the bottom page row holds the colour keys' destinations, one
 * per quarter; elsewhere a click on a three-digit page number opens it. */
static void click(Point pt)
{
    short r = (pt.v - top0) / ch, c = (pt.h - left0) / cw, a, b;

    if (pt.v < top0 || pt.h < left0 || r >= ROWS || c >= COLS)
        return;
    if (r == FASTEXT_ROW) {
        add_keys(&colour_keys[c * 4 / COLS], 1);
        return;
    }
#define DIGIT(x) (!(cells[r][x][1] & MOSAIC) && cells[r][x][0] >= '0' && cells[r][x][0] <= '9')
    if (!DIGIT(c))
        return;
    for (a = c; a > 0 && DIGIT(a - 1); a--)
        ;
    for (b = c; b < COLS - 1 && DIGIT(b + 1); b++)
        ;
    if (b - a == 2) {
        char page[3];
        short i;
        for (i = 0; i < 3; i++)
            page[i] = (char)cells[r][a + i][0];
        add_keys(page, 3);
    }
#undef DIGIT
}

int main(void)
{
    Rect sb, r;
    FontInfo fi;
    Boolean quit = false;
    Str255 title = "\x09Teletekst";
    OSErr err;

    tw_init("Teletekst", "Teletekst via lftest serve and ssh teletekst.nl.");
    make_patterns();

    /* The window fits the page: 40 × 26 cells of Monaco 9. */
    sb = qd.screenBits.bounds;
    SetRect(&r, 0, 0, 1, 1);
    win = NewWindow(NULL, &r, title, false, noGrowDocProc, (WindowPtr)-1, false, 0);
    SetPort(win);
    TextFont(4);
    TextSize(9);
    GetFontInfo(&fi);
    cw = CharWidth('m');
    ch = fi.ascent + fi.descent + fi.leading;
    ascent = fi.ascent;
    left0 = top0 = 4;
    SizeWindow(win, COLS * cw + 8, ROWS * ch + 8, false);
    MoveWindow(win, (sb.right - (COLS * cw + 8)) / 2, 40, true);
    ShowWindow(win);

    message("Teletekst", "Zoeken naar de server...");
    err = at_open();
    if (err != noErr) {
        message("AppleTalk staat uit.", "Zet AppleTalk aan in de Kiezer.");
        state = NET_OFF;
    }
    draw_all();

    while (!quit) {
        TWEvent e;
        if (state != NET_OFF)
            net_idle();
        if (!tw_poll(&e))
            continue;
        switch (e.kind) {
        case TW_QUIT:
            quit = true;
            break;
        case TW_KEY:
            if (e.command && e.key >= '1' && e.key <= '4')
                add_keys(&colour_keys[e.key - '1'], 1);
            else if (e.command)
                break;
            else if (e.key == '\r' || e.key == 3)
                add_keys("\r", 1);
            else
                add_keys(&e.key, 1);
            break;
        case TW_CLICK:
            click(e.where);
            break;
        case TW_UPDATE:
            SetPort(win);
            BeginUpdate(win);
            draw_all();
            EndUpdate(win);
            break;
        default:
            break;
        }
    }
    if (state != NET_OFF)
        net_close();
    return 0;
}
