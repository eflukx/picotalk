/*
 * Tanks: a tank-level dashboard on a classic Mac, from `lftest serve`,
 * which fetches it from a web dashboard. The protocol is described in
 * localframe/server/lftest/src/tanks.rs:
 *   POLL 01 00 have(2) → 01 st version(2) snapshot (if newer than `have`)
 *
 * The dashboard fills the screen: three tanks, each a cylinder filled to
 * its level, with today's and yesterday's totals, and the weather. Cmd-D
 * (or D) switches between dithered shading and flat patterns.
 */
#include <string.h>

#include "appletalk.h"
#include "textwin.h"

#define TK_TYPE "Tanks"
enum { CMD_POLL = 1 };
#define ST_OK 0
#define POLL_PACKETS 2
#define POLL_TIMEOUT 4 /* s; the server holds a POLL for 2 s */

#define MAX_TANKS 3
#define GENEVA 3

enum { NET_OFF = -1, NET_LOOKUP, NET_FINDING, NET_RUNNING };
enum { STABLE, FILLING, EMPTYING, OTHER };

typedef struct {
    Str255 name;   /* Pascal strings, shortened to fit below */
    short status;
    Str255 status_text;
    short level;   /* 0.1 % */
    Str255 v[6];   /* today filled, emptied, level; yesterday the same */
} Tank;

static WindowPtr win;
static short width, height;

static Tank tanks[MAX_TANKS];
static short ntanks;
static Boolean have_weather;
static Str255 temperature, summary, feels, updated, problem, note;
static Boolean shaded = true;

static short state = NET_LOOKUP;
static long nextLookup;
static NBPLookup look;
static ATAddr server;
static unsigned short have;
static ATPRequest pollR;
static Boolean pollBusy;
static unsigned char pollReq[4];
static unsigned char pollBuf[POLL_PACKETS * ATP_MAX_DATA];

/* ---- strings ---- */

/* A Pascal string literal: P("\x05Hello"). */
#define P(s) ((const unsigned char *)(s))

static void cstr(Str255 to, const char *from)
{
    short n = (short)strlen(from);
    to[0] = (unsigned char)n;
    memcpy(to + 1, from, n);
}

/* ---- drawing ---- */

static const unsigned char bayer[4][4] = {{0, 8, 2, 10}, {12, 4, 14, 6}, {3, 11, 1, 9}, {15, 7, 13, 5}};

/* Pattern for grey level 0 (white) to 16 (black). Patterns line up with
 * the window, so neighbouring fills of one level join seamlessly. */
static void grey(short level, Pattern *p)
{
    short y, x;
    for (y = 0; y < 8; y++) {
        unsigned char bits = 0;
        for (x = 0; x < 8; x++)
            if (bayer[y & 3][x & 3] < level)
                bits |= 0x80 >> x;
        p->pat[y] = bits;
    }
}

static void draw_centred(const unsigned char *s, short x, short y)
{
    MoveTo(x - StringWidth(s) / 2, y);
    DrawString(s);
}

static void draw_right(const unsigned char *s, short x, short y)
{
    MoveTo(x - StringWidth(s), y);
    DrawString(s);
}

/* The iotta logo, drawn with the pen: about 104 × 22 pixels. */
static void draw_logo(short x, short top)
{
    Rect r;
    PenSize(3, 3);
    /* I */
    MoveTo(x, top);
    LineTo(x, top + 19);
    x += 9;
    /* O */
    SetRect(&r, x, top, x + 22, top + 22);
    FrameOval(&r);
    x += 28;
    /* T T */
    MoveTo(x, top);
    LineTo(x + 32, top);
    MoveTo(x + 8, top);
    LineTo(x + 8, top + 19);
    MoveTo(x + 24, top);
    LineTo(x + 24, top + 19);
    x += 38;
    /* a */
    SetRect(&r, x, top + 7, x + 16, top + 22);
    FrameOval(&r);
    MoveTo(x + 13, top + 7);
    LineTo(x + 13, top + 19);
    x += 21;
    /* . */
    SetRect(&r, x, top + 18, x + 4, top + 22);
    PaintOval(&r);
    PenNormal();
}

/* A small triangle for filling (up) or emptying (down). */
static void draw_arrow(short x, short y, Boolean up)
{
    short i;
    for (i = 0; i < 5; i++) {
        short row = up ? y - 4 + i : y - i;
        MoveTo(x - i, row);
        LineTo(x + i, row);
    }
}

/*
 * A tank, drawn pixel by pixel into an offscreen bitmap: a glass cylinder
 * (rim ellipse at the top, front edge at the bottom), filled to `level`
 * (0.1 %) with liquid whose surface is an ellipse. With shading, the
 * liquid gets a highlight left of centre that darkens towards the edges
 * and the bottom, and the glass a faint edge, all in ordered dither; flat,
 * the liquid is one grey and the glass white.
 */
#define TANK_W 80
#define TANK_ROWBYTES ((TANK_W + 15) / 16 * 2)
#define ELLIPSE 7 /* half-height of the ellipses */

static void draw_tank(short left, short top, short h, short level)
{
    static unsigned char bits[TANK_ROWBYTES * 200];
    short half[TANK_W];  /* ellipse half-height in each column */
    short side[TANK_W];  /* shading across the cylinder, 0..16 */
    BitMap bm;
    Rect src, dst;
    short x, y, yt = ELLIPSE, yb = h - 1 - ELLIPSE, ys;
    long rx2 = (long)(TANK_W - 1) * (TANK_W - 1);

    if (h > 200)
        h = 200;
    ys = yb - (short)((long)(yb - yt) * level / 1000);
    for (x = 0; x < TANK_W; x++) {
        long dx2 = (long)(2 * x - (TANK_W - 1)) * (2 * x - (TANK_W - 1)); /* (2·dx)² */
        long e = rx2 - dx2;                                            /* ∝ 1 - dx²/rx² */
        short hy = 0;
        /* hy = ELLIPSE·√(e/rx2), by counting */
        while ((long)(hy + 1) * (hy + 1) * rx2 <= e * ELLIPSE * ELLIPSE)
            hy++;
        half[x] = hy;
        {
            /* u from -1 (left) to 1 (right); the highlight sits at -0.3 */
            long u100 = (long)(200 * x) / (TANK_W - 1) - 100 + 30;
            side[x] = (short)(u100 * u100 * 5 / 10000);
        }
    }
    memset(bits, 0, sizeof bits);
    for (y = 0; y < h; y++) {
        unsigned char *row = bits + y * TANK_ROWBYTES;
        for (x = 0; x < TANK_W; x++) {
            short hy = half[x], g;
            Boolean ink;
            if (y < yt - hy || y > yb + hy)
                continue; /* outside */
            if (x == 0 || x == TANK_W - 1 || y == yt - hy || y == yt + hy || y == yb + hy)
                ink = true; /* outline: sides, rim, bottom edge */
            else if (level > 0 && (y == ys - hy || y == ys + hy))
                ink = true; /* the surface's outline */
            else {
                if (level > 0 && y > ys - hy) {
                    if (y < ys + hy) /* the surface, seen from above */
                        g = shaded ? 4 + side[x] / 2 : 6;
                    else
                        g = shaded ? 6 + side[x] + (short)((long)(y - ys) * 3 / (yb - yt + 1)) : 10;
                } else
                    g = shaded ? side[x] / 3 : 0; /* glass */
                if (g > 16)
                    g = 16;
                ink = bayer[(top + y) & 3][(left + x) & 3] < g;
            }
            if (ink)
                row[x >> 3] |= 0x80 >> (x & 7);
        }
    }
    bm.baseAddr = (Ptr)bits;
    bm.rowBytes = TANK_ROWBYTES;
    SetRect(&bm.bounds, 0, 0, TANK_W, h);
    SetRect(&src, 0, 0, TANK_W, h);
    SetRect(&dst, left, top, left + TANK_W, top + h);
    CopyBits(&bm, &win->portBits, &src, &dst, srcCopy, NULL);
}

/* Text in white with a black outline, readable on any shade. */
static void draw_outlined(const unsigned char *s, short x, short y)
{
    short dx, dy;
    x -= StringWidth(s) / 2;
    TextMode(srcOr);
    for (dy = -1; dy <= 1; dy++)
        for (dx = -1; dx <= 1; dx++)
            if (dx || dy) {
                MoveTo(x + dx, y + dy);
                DrawString(s);
            }
    TextMode(srcBic);
    MoveTo(x, y);
    DrawString(s);
    TextMode(srcOr);
}

static void draw_card(const Tank *t, const Rect *card)
{
    Rect r = *card;
    short cx = (card->left + card->right) / 2, y, tank_top, tank_h;
    short col[3];
    Str255 pct;
    static const char *rows[2] = {"Vandaag", "Gisteren"};
    short i;

    if (shaded) {
        Pattern p;
        Rect s = r;
        grey(8, &p);
        OffsetRect(&s, 3, 3);
        FillRoundRect(&s, 16, 16, &p);
    }
    EraseRoundRect(&r, 16, 16);
    FrameRoundRect(&r, 16, 16);

    TextFont(GENEVA);
    TextSize(12);
    TextFace(bold);
    y = r.top + 18;
    draw_centred(t->name, cx, y);
    TextFace(t->status == STABLE ? italic : bold);
    TextSize(10);
    y += 15;
    draw_centred(t->status_text, cx, y);
    if (t->status == FILLING || t->status == EMPTYING)
        draw_arrow(cx + StringWidth(t->status_text) / 2 + 10, y - 2, t->status == FILLING);

    tank_top = y + 9;
    tank_h = r.bottom - 62 - tank_top;
    draw_tank(cx - TANK_W / 2, tank_top, tank_h, t->level);
    {
        short l = t->level;
        char buf[8];
        short n = 0;
        if (l >= 1000)
            buf[n++] = '1';
        if (l >= 100)
            buf[n++] = (char)('0' + l / 100 % 10);
        buf[n++] = (char)('0' + l / 10 % 10);
        buf[n++] = '.';
        buf[n++] = (char)('0' + l % 10);
        buf[n++] = '%';
        buf[n] = 0;
        cstr(pct, buf);
    }
    TextSize(14);
    TextFace(bold);
    draw_outlined(pct, cx, tank_top + tank_h / 2 + 5);

    /* The totals: Geladen, Gelost, Stand, right-aligned. */
    col[0] = r.left + 82;
    col[1] = r.left + 118;
    col[2] = r.right - 8;
    TextSize(9);
    y = r.bottom - 44;
    TextFace(bold);
    MoveTo(r.left + 8, y);
    DrawString(P("\x06Liters"));
    draw_right(P("\x07Geladen"), col[0], y);
    draw_right(P("\x06Gelost"), col[1], y);
    draw_right(P("\x05Stand"), col[2], y);
    for (i = 0; i < 2; i++) {
        Str255 label;
        MoveTo(r.left + 8, y + 4);
        LineTo(r.right - 8, y + 4);
        y += 16;
        TextFace(0);
        cstr(label, rows[i]);
        MoveTo(r.left + 8, y);
        DrawString(label);
        draw_right(t->v[3 * i], col[0], y);
        draw_right(t->v[3 * i + 1], col[1], y);
        draw_right(t->v[3 * i + 2], col[2], y);
    }
}

static void draw_all(void)
{
    Rect card;
    short i, k, cw = (width - 4 * 10) / 3;

    SetPort(win);
    EraseRect(&win->portRect);
    PenNormal();
    TextMode(srcOr);

    draw_logo(14, 10);
    TextFont(GENEVA);
    TextSize(12);
    TextFace(bold);
    MoveTo(14, 50);
    DrawString(P("\x19Status Brandstoflaaddepot"));

    if (have_weather) {
        TextSize(14);
        draw_right(temperature, width - 12, 22);
        TextSize(10);
        TextFace(0);
        draw_right(summary, width - 12, 36);
        TextSize(9);
        draw_right(feels, width - 12, 49);
    }

    for (i = 0; i < MAX_TANKS; i++) {
        Tank empty;
        const Tank *t = &tanks[i];
        if (i >= ntanks) {
            memset(&empty, 0, sizeof empty);
            cstr(empty.name, i == 0 ? "Tank 1" : i == 1 ? "Tank 2" : "Tank 3");
            cstr(empty.status_text, "-");
            for (k = 0; k < 6; k++)
                cstr(empty.v[k], "-");
            t = &empty;
        }
        SetRect(&card, 10 + i * (cw + 10), 60, 10 + i * (cw + 10) + cw, height - 22);
        draw_card(t, &card);
    }

    TextFont(GENEVA);
    TextSize(9);
    TextFace(0);
    if (updated[0]) {
        Str255 s;
        cstr(s, "Laatste update: ");
        memcpy(s + 1 + s[0], updated + 1, updated[0]);
        s[0] += updated[0];
        MoveTo(12, height - 7);
        DrawString(s);
    }
    TextFace(bold);
    draw_right(note[0] ? note : problem, width - 12, height - 7);
    TextFace(0);
}

/* Shows `s` at the bottom right (a connection message), or clears it. */
static void set_note(const char *s)
{
    cstr(note, s);
    draw_all();
}

/* ---- the snapshot ---- */

/* Reads a Pascal string into `to` (at most `max` characters). */
static Boolean read_pstr(const unsigned char **p, const unsigned char *end, Str255 to, short max)
{
    short n;
    if (*p >= end || *p + 1 + **p > end)
        return false;
    n = **p;
    to[0] = (unsigned char)(n < max ? n : max);
    memcpy(to + 1, *p + 1, to[0]);
    *p += 1 + n;
    return true;
}

static Boolean parse(const unsigned char *p, const unsigned char *end)
{
    short n, i, k;
    if (p >= end)
        return false;
    n = *p++;
    if (n > MAX_TANKS)
        n = MAX_TANKS;
    for (i = 0; i < n; i++) {
        Tank *t = &tanks[i];
        if (!read_pstr(&p, end, t->name, 24) || p >= end)
            return false;
        t->status = *p++;
        if (!read_pstr(&p, end, t->status_text, 24) || p + 2 > end)
            return false;
        t->level = (short)((p[0] << 8) | p[1]);
        p += 2;
        if (t->level > 1000)
            t->level = 1000;
        for (k = 0; k < 6; k++)
            if (!read_pstr(&p, end, t->v[k], 12))
                return false;
    }
    ntanks = n;
    if (p >= end)
        return false;
    have_weather = *p++ == 1;
    if (have_weather && !(read_pstr(&p, end, temperature, 16) && read_pstr(&p, end, summary, 60) &&
                          read_pstr(&p, end, feels, 80)))
        return false;
    return read_pstr(&p, end, updated, 40) && read_pstr(&p, end, problem, 80);
}

/* ---- network ---- */

static void start_poll(void)
{
    pollReq[0] = CMD_POLL;
    pollReq[1] = 0;
    pollReq[2] = (unsigned char)(have >> 8);
    pollReq[3] = (unsigned char)have;
    pollBusy = at_request(&pollR, server, pollReq, 4, 0, pollBuf, POLL_PACKETS, POLL_TIMEOUT, true) == noErr;
}

static void on_poll(void)
{
    long len = at_response_length(&pollR);
    unsigned short version;

    if (pollR.h.ioResult != noErr) {
        set_note("De tankserver antwoordt niet. Opnieuw zoeken...");
        state = NET_LOOKUP;
        nextLookup = TickCount() + 60;
        have = 0;
        return;
    }
    if (len < 4 || pollBuf[1] != ST_OK)
        return;
    version = (unsigned short)((pollBuf[2] << 8) | pollBuf[3]);
    if (version == have || len == 4)
        return;
    if (parse(pollBuf + 4, pollBuf + len)) {
        have = version;
        note[0] = 0;
        draw_all();
    }
}

static void net_idle(void)
{
    if (pollBusy && at_done(&pollR)) {
        pollBusy = false;
        on_poll();
    }
    if (state == NET_FINDING && at_lookup_done(&look)) {
        NBPResult found;
        if (at_lookup_results(&look, &found, 1) > 0) {
            server = found.addr;
            have = 0;
            state = NET_RUNNING;
            set_note("Verbonden, wachten op data...");
        } else {
            set_note("Geen tankserver gevonden. Draait `lftest serve` met BLD_URL?");
            state = NET_LOOKUP;
            nextLookup = TickCount() + 2 * 60;
        }
    }
    if (state == NET_LOOKUP && !pollBusy && TickCount() >= nextLookup) {
        OSErr err = at_lookup_start(&look, "=", TK_TYPE, 1, true);
        if (err == noErr)
            state = NET_FINDING;
        else
            nextLookup = TickCount() + 60;
    }
    if (state == NET_RUNNING && !pollBusy)
        start_poll();
}

static void net_close(void)
{
    long give_up = TickCount() + 10 * 60;
    if (pollBusy)
        at_cancel(&pollR);
    while (((pollBusy && !at_done(&pollR)) || (state == NET_FINDING && !at_lookup_done(&look))) &&
           TickCount() < give_up)
        SystemTask();
}

int main(void)
{
    Rect r;
    Boolean quit = false;

    tw_init("Tanks", "Tanks: a tank-level dashboard via lftest serve.\nCmd-D switches the shading.");

    /* The whole screen below the menu bar. */
    r = qd.screenBits.bounds;
    r.top += 20;
    win = NewWindow(NULL, &r, (ConstStr255Param)P("\x05Tanks"), true, plainDBox, (WindowPtr)-1, false, 0);
    SetPort(win);
    width = win->portRect.right;
    height = win->portRect.bottom;

    cstr(note, "Zoeken naar de tankserver...");
    if (at_open() != noErr) {
        cstr(note, "AppleTalk staat uit. Zet AppleTalk aan in de Kiezer.");
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
        case TW_CANCEL:
            quit = true;
            break;
        case TW_KEY:
            if (e.key == 'd' || e.key == 'D') {
                shaded = !shaded;
                draw_all();
            }
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
